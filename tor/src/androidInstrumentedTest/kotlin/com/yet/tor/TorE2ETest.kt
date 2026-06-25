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
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File
import java.net.InetSocketAddress
import java.net.Proxy

/**
 * On-device end-to-end proof: bootstrap to 100% (from the first-class status
 * API, not log scraping), then fetch through the local SOCKS proxy and confirm
 * the traffic exits via Tor. Run with `:tor:connectedDebugAndroidTest`.
 */
@RunWith(AndroidJUnit4::class)
class TorE2ETest {

    private val tag = "TorE2E"

    @Test
    fun bootstrapAndFetchThroughTor() = runBlocking {
        val ctx = InstrumentationRegistry.getInstrumentation().targetContext
        val dataDir = File(ctx.filesDir, "arti-e2e").apply { mkdirs() }.absolutePath

        val client = ArtiTorClient()
        Log.i(tag, "arti version = ${client.version}")

        val logJob = launch { client.logs.collect { Log.i(tag, "arti-log: $it") } }
        val statusJob = launch { client.status.collect { Log.i(tag, "status = $it") } }

        // Direct (no Tor) IP for contrast.
        val directIp = runCatching {
            OkHttpClient().newCall(
                Request.Builder().url("https://check.torproject.org/api/ip").build()
            ).execute().use { it.body?.string().orEmpty() }
        }.getOrElse { "direct-failed: ${it.message}" }
        Log.i(tag, "DIRECT response = $directIp")

        // Bootstrap can take tens of seconds on a fresh data dir.
        withTimeout(300_000) {
            client.start(ArtiConfig(dataDir = dataDir, socksPort = 19050)).getOrThrow()
        }
        val port = requireNotNull(client.status.value.socksPort) { "SOCKS port not set" }
        assertTrue("bootstrap not 100%", client.status.value.bootstrapPercent == 100)
        Log.i(tag, "BOOTSTRAP complete, SOCKS on 127.0.0.1:$port")

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

        client.stop()
        delay(500)
        logJob.cancel()
        statusJob.cancel()
    }
}
