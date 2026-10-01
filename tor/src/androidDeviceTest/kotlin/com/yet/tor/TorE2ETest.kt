package com.yet.tor

import android.util.Log
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import okhttp3.OkHttpClient
import okhttp3.ConnectionPool
import okhttp3.Request
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File
import java.net.InetSocketAddress
import java.net.Proxy
import kotlin.time.Duration.Companion.seconds
import kotlin.time.TimeSource
import java.util.concurrent.TimeUnit

/**
 * On-device end-to-end proof: bootstrap, SOCKS fetch via Tor, pause/resume
 * without a second full bootstrap. Run with `:tor:connectedAndroidDeviceTest`.
 */
@RunWith(AndroidJUnit4::class)
class TorE2ETest {

    private val tag = "TorE2E"

    @Test
    fun bootstrapFetchPauseResume() = runBlocking {
        val ctx = InstrumentationRegistry.getInstrumentation().targetContext
        val dataDir = File(ctx.filesDir, "arti-e2e").apply { mkdirs() }.absolutePath

        val client = ArtiTorClient()
        Log.i(tag, "arti version = ${client.version}")

        val logJob = launch { client.logs.collect { Log.i(tag, "arti-log: $it") } }
        val statusJob = launch { client.status.collect { Log.i(tag, "status = $it") } }

        val directIp = runCatching {
            OkHttpClient().newCall(
                Request.Builder().url("https://check.torproject.org/api/ip").build()
            ).execute().use { it.body?.string().orEmpty() }
        }.getOrElse { "direct-failed: ${it.message}" }
        Log.i(tag, "DIRECT response = $directIp")

        // Ephemeral port; cold bootstrap can take tens of seconds.
        withTimeout(300_000) {
            client.start(ArtiConfig(dataDir = dataDir, socksPort = 0), timeout = 180.seconds)
                .getOrThrow()
        }
        val port = requireNotNull(client.status.value.socksPort) { "SOCKS port not set" }
        assertTrue("bootstrap not 100%", client.status.value.bootstrapPercent == 100)
        assertTrue(client.isReady)
        assertTrue(client.hasClient)
        Log.i(tag, "BOOTSTRAP complete, SOCKS on 127.0.0.1:$port")

        assertTorExit(port)

        // Soft stop: client retained.
        client.pause()
        delay(500)
        assertFalse("should not be ready after pause", client.isReady)
        assertTrue("client should be retained after pause", client.hasClient)
        assertEquals(TorState.PAUSED, client.status.value.state)
        Log.i(tag, "PAUSED ok")

        // Resume must not re-do multi-minute bootstrap from scratch.
        val resumeStart = System.currentTimeMillis()
        withTimeout(60_000) {
            client.resume(timeout = 45.seconds).getOrThrow()
        }
        val resumeMs = System.currentTimeMillis() - resumeStart
        Log.i(tag, "RESUME took ${resumeMs}ms")
        assertTrue(client.isReady)
        val port2 = requireNotNull(client.status.value.socksPort)
        assertTorExit(port2)
        // Record timing as evidence; it is not a frozen performance promise.

        client.shutdown()
        delay(500)
        assertFalse(client.hasClient)
        assertEquals(TorState.OFF, client.status.value.state)

        // Cold start again after shutdown (fresh bootstrap, new ephemeral port).
        withTimeout(300_000) {
            client.start(ArtiConfig(dataDir = dataDir, socksPort = 0), timeout = 180.seconds)
                .getOrThrow()
        }
        val port3 = requireNotNull(client.status.value.socksPort) { "SOCKS port not set" }
        assertTrue(client.isReady)
        assertTorExit(port3)
        Log.i(tag, "SECOND COLD START ok on 127.0.0.1:$port3")

        client.shutdown()
        delay(500)
        assertFalse(client.hasClient)
        assertEquals(TorState.OFF, client.status.value.state)

        logJob.cancel()
        statusJob.cancel()
    }

    @Test
    fun integratedIsolationOnionAndTransactionalLifecycle() = runBlocking {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val config = ArtiConfig(File(context.filesDir, "arti-03-integrated").apply { mkdirs() }.absolutePath)
        val tor = ArtiTorClient()
        val logs = launch { tor.logs.collect { line ->
            Regex("SOCKS CONNECT failed \\(code=5, arti_kind=[A-Za-z0-9_]+\\)").find(line)?.value?.let { Log.i(tag, it) }
        } }
        try {
            val cold = TimeSource.Monotonic.markNow()
            tor.start(config, timeout = 180.seconds).getOrThrow()
            Log.i(tag, "LIVE_TIMING,cold_ms=${cold.elapsedNow().inWholeMilliseconds}")
            assertEquals(100, tor.status.value.bootstrapPercent)
            httpSuccess(requireNotNull(tor.socksEndpoint).port, "http://api.ipify.org")
            val measured = mutableListOf<TorIsolationSession>()
            try {
                recordResources(0)
                for (count in listOf(1, 8, 16, 32)) {
                    while (measured.size < count) measured += tor.createIsolationSession().getOrThrow()
                    assertEquals(count, measured.map { it.id }.toSet().size)
                    assertEquals(count + 1, (measured.map { requireNotNull(it.status.value.socksEndpoint).port } +
                        requireNotNull(tor.socksEndpoint).port).toSet().size)
                    delay(3000)
                    repeat(3) { recordResources(count); delay(500) }
                }
                assertTrue(tor.createIsolationSession().isFailure)
            } finally { measured.forEach { it.close() }; recordResources(0) }
            val a = tor.createIsolationSession().getOrThrow()
            val b = tor.createIsolationSession().getOrThrow()
            assertEquals(3, setOf(tor.socksEndpoint, a.status.value.socksEndpoint, b.status.value.socksEndpoint).size)
            httpSuccess(requireNotNull(a.status.value.socksEndpoint).port, "http://api.ipify.org")
            httpSuccess(requireNotNull(b.status.value.socksEndpoint).port, "http://api.ipify.org")
            a.close()
            assertEquals(TorIsolationSessionState.CLOSED, a.status.value.state)
            httpSuccess(requireNotNull(tor.socksEndpoint).port, "http://api.ipify.org")
            httpSuccess(requireNotNull(b.status.value.socksEndpoint).port, "http://api.ipify.org")
            tor.pause()
            assertEquals(null, tor.socksEndpoint)
            assertEquals(TorIsolationSessionState.PAUSED, b.status.value.state)
            assertEquals(null, b.status.value.socksEndpoint)
            val warm = TimeSource.Monotonic.markNow()
            tor.resume(timeout = 45.seconds).getOrThrow()
            Log.i(tag, "LIVE_TIMING,resume_ms=${warm.elapsedNow().inWholeMilliseconds}")
            assertEquals(TorState.RUNNING, tor.status.value.state)
            assertTrue(tor.isReady)
            // Root readiness completes resume; each session publishes its own rebind.
            val resumedB = withTimeout(30_000) {
                b.status.first { it.state == TorIsolationSessionState.ACTIVE && it.socksEndpoint != null }
            }
            httpSuccess(requireNotNull(resumedB.socksEndpoint).port, "http://api.ipify.org")
            val onion = "http://hjirlp6fu47kox4cnede4zlvaeq672bibss3oxgmsnsc5mdxygqshbqd.onion/"
            httpSuccess(requireNotNull(b.status.value.socksEndpoint).port, onion)
            val beforeRoot = tor.status.value
            val beforeB = b.status.value
            for (mode in BridgesEnabled.entries) {
                assertTrue(tor.start(config.copy(bridges = listOf("malformed synthetic bridge"), bridgesEnabled = mode))
                    .exceptionOrNull() is ArtiException.Config)
                assertEquals(beforeRoot, tor.status.value)
                assertEquals(beforeB, b.status.value)
            }
            tor.shutdown()
            assertEquals(TorIsolationSessionState.INVALIDATED, b.status.value.state)
            tor.start(config, timeout = 180.seconds).getOrThrow()
            val fresh = tor.createIsolationSession().getOrThrow()
            b.close()
            assertEquals(TorIsolationSessionState.INVALIDATED, b.status.value.state)
            httpSuccess(requireNotNull(fresh.status.value.socksEndpoint).port, "http://api.ipify.org")
            tor.start(config.copy(allowOnionAddrs = false), timeout = 180.seconds).getOrThrow()
            assertEquals(TorIsolationSessionState.INVALIDATED, fresh.status.value.state)
            assertTrue(runCatching { httpSuccess(requireNotNull(tor.socksEndpoint).port, onion) }.isFailure)
            httpSuccess(requireNotNull(tor.socksEndpoint).port, "http://api.ipify.org")
        } finally { tor.shutdown(); logs.cancel() }
    }

    private fun recordResources(sessions: Int) {
        val rss = File("/proc/self/status").readLines().firstOrNull { it.startsWith("VmRSS:") }
            ?.substringAfter(':')?.trim().orEmpty()
        Log.i(tag, "ANDROID_RESOURCE,sessions=$sessions,rss=$rss,fd=${File("/proc/self/fd").list()?.size},threads=${File("/proc/self/task").list()?.size}")
    }

    /** A fresh SOCKS-only pool per call; no pool is shared across identities. */
    private fun httpSuccess(port: Int, url: String) {
        val http = OkHttpClient.Builder()
            .proxy(Proxy(Proxy.Type.SOCKS, InetSocketAddress("127.0.0.1", port)))
            .connectionPool(ConnectionPool())
            .followRedirects(false).callTimeout(90, TimeUnit.SECONDS).build()
        try {
            http.newCall(Request.Builder().url(url).build()).execute().use {
                assertTrue("unexpected HTTP status ${it.code}", it.code == 200 || it.code in listOf(301, 302, 307, 308))
            }
        } finally { http.connectionPool.evictAll(); http.dispatcher.executorService.shutdown() }
    }

    private fun assertTorExit(port: Int) {
        val proxy = Proxy(Proxy.Type.SOCKS, InetSocketAddress("127.0.0.1", port))
        val torClient = OkHttpClient.Builder().proxy(proxy).build()
        val torResp = torClient.newCall(
            Request.Builder().url("https://check.torproject.org/api/ip").build()
        ).execute().use { it.body?.string().orEmpty() }
        Log.i(tag, "TOR response = $torResp")
        assertTrue(
            "expected IsTor:true in $torResp",
            torResp.replace(" ", "").contains("\"IsTor\":true")
        )
        assertNotNull(torResp)
    }
}
