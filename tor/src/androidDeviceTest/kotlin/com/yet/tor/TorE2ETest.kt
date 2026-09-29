package com.yet.tor

import android.util.Log
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import okhttp3.OkHttpClient
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
        // Resume should be far cheaper than cold start (heuristic: under 30s).
        assertTrue("resume too slow (${resumeMs}ms) — client may have re-bootstrapped", resumeMs < 30_000)

        client.shutdown()
        delay(500)
        assertFalse(client.hasClient)
        assertEquals(TorState.OFF, client.status.value.state)

        logJob.cancel()
        statusJob.cancel()
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
