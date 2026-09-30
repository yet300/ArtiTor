package com.yet.tor

import kotlinx.cinterop.ExperimentalForeignApi
import kotlinx.cinterop.addressOf
import kotlinx.cinterop.alloc
import kotlinx.cinterop.convert
import kotlinx.cinterop.memScoped
import kotlinx.cinterop.ptr
import kotlinx.cinterop.reinterpret
import kotlinx.cinterop.usePinned
import kotlinx.cinterop.sizeOf
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.collect
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import platform.Foundation.NSTemporaryDirectory
import platform.posix.AF_INET
import platform.posix.SOCK_STREAM
import platform.posix.close
import platform.posix.connect
import platform.posix.recv
import platform.posix.send
import platform.posix.sockaddr
import platform.posix.sockaddr_in
import platform.posix.socket
import platform.posix.setsockopt
import platform.posix.SOL_SOCKET
import platform.posix.SO_RCVTIMEO
import platform.posix.SO_SNDTIMEO
import platform.posix.timeval
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertTrue
import kotlin.test.assertIs
import kotlin.time.TimeSource
import kotlin.time.Duration.Companion.seconds

/**
 * iOS-simulator end-to-end: bootstrap, SOCKS HTTP via Tor, pause/resume.
 */
class TorIosE2ETest {

    @Test
    fun liveTwoSessionsLifecycle() = runBlocking {
        val dataDir = NSTemporaryDirectory() + "arti-ios-session-e2e"
        withLiveClient { client ->
            val config = ArtiConfig(dataDir = dataDir)
            val cold = TimeSource.Monotonic.markNow()
            client.start(config, timeout = 180.seconds).getOrThrow()
            println("LIVE_TIMING,cold_ms=${cold.elapsedNow().inWholeMilliseconds}")
            assertEquals(100, client.status.value.bootstrapPercent)
            assertHttpSuccess(socksHttpGet(requireNotNull(client.socksEndpoint).port, "api.ipify.org"))
            measureLiveSessionResources(client)
            val a = client.createIsolationSession().getOrThrow()
            val b = client.createIsolationSession().getOrThrow()
            val rootPort = requireNotNull(client.socksEndpoint).port
            val aPort = requireNotNull(a.status.value.socksEndpoint).port
            val bPort = requireNotNull(b.status.value.socksEndpoint).port
            assertEquals(3, setOf(rootPort, aPort, bPort).size)
            assertEquals(setOf(a, b), client.sessions.toSet())
            assertTrue(socksHttpGet(aPort, "api.ipify.org").startsWith("HTTP/1"))
            assertTrue(socksHttpGet(bPort, "api.ipify.org").startsWith("HTTP/1"))

            a.close()
            assertEquals(TorIsolationSessionState.CLOSED, a.status.value.state)
            assertEquals(null, a.status.value.socksEndpoint)
            assertEquals(listOf(b), client.sessions)
            assertTrue(socksHttpGet(bPort, "api.ipify.org").startsWith("HTTP/1"))
            assertTrue(socksHttpGet(rootPort, "api.ipify.org").startsWith("HTTP/1"))

            client.pause()
            assertEquals(TorIsolationSessionState.PAUSED, b.status.value.state)
            assertEquals(null, b.status.value.socksEndpoint)
            assertTrue(client.hasClient)
            assertEquals(null, client.socksEndpoint)
            val warm = TimeSource.Monotonic.markNow()
            client.resume(timeout = 45.seconds).getOrThrow()
            println("LIVE_TIMING,resume_ms=${warm.elapsedNow().inWholeMilliseconds}")
            assertEquals(100, client.status.value.bootstrapPercent)
            val resumed = withTimeout(30_000) {
                b.status.first { it.state == TorIsolationSessionState.ACTIVE }
            }
            assertTrue(socksHttpGet(requireNotNull(resumed.socksEndpoint).port, "api.ipify.org").startsWith("HTTP/1"))

            // Reputable public v3 fixture published at https://onion.torproject.org/.
            // SOCKS remote-name routing lets Arti resolve onion names internally.
            assertHttpSuccess(socksHttpGet(requireNotNull(resumed.socksEndpoint).port,
                "hjirlp6fu47kox4cnede4zlvaeq672bibss3oxgmsnsc5mdxygqshbqd.onion"))
            println("LIVE_ONION,public_v3_http_success=true")
            assertEquals(5, socksReplyCode(requireNotNull(resumed.socksEndpoint).port, "malformed.onion"))
            println("LIVE_ONION,malformed_target_code=5")
            val beforeRoot = client.status.value
            val beforeB = b.status.value
            for (mode in BridgesEnabled.entries) {
                val rejected = client.start(config.copy(bridges = listOf("malformed synthetic bridge"), bridgesEnabled = mode))
                assertIs<ArtiException.Config>(rejected.exceptionOrNull())
                assertEquals(beforeRoot, client.status.value)
                assertEquals(beforeB, b.status.value)
                assertTrue(client.hasClient)
            }
            assertHttpSuccess(socksHttpGet(requireNotNull(client.socksEndpoint).port, "api.ipify.org"))
            assertHttpSuccess(socksHttpGet(requireNotNull(b.status.value.socksEndpoint).port, "api.ipify.org"))
            println("LIVE_TRANSACTION,rejected_all_bridge_modes_preserved_root_and_b=true")

            client.shutdown()
            assertEquals(TorIsolationSessionState.INVALIDATED, b.status.value.state)
            assertEquals(TorIsolationSessionState.CLOSED, a.status.value.state)
            assertTrue(client.sessions.isEmpty())
            client.start(ArtiConfig(dataDir = dataDir), timeout = 180.seconds).getOrThrow()
            assertEquals(TorIsolationSessionState.INVALIDATED, b.status.value.state)
            val fresh = client.createIsolationSession().getOrThrow()
            assertTrue(fresh.id != a.id && fresh.id != b.id)
            b.close()
            assertEquals(TorIsolationSessionState.ACTIVE, fresh.status.value.state)
            assertHttpSuccess(socksHttpGet(requireNotNull(fresh.status.value.socksEndpoint).port, "api.ipify.org"))
            client.start(config.copy(allowOnionAddrs = false), timeout = 180.seconds).getOrThrow()
            assertEquals(TorIsolationSessionState.INVALIDATED, fresh.status.value.state)
            val policyPort = requireNotNull(client.socksEndpoint).port
            assertEquals(5, socksReplyCode(policyPort,
                "hjirlp6fu47kox4cnede4zlvaeq672bibss3oxgmsnsc5mdxygqshbqd.onion"))
            assertHttpSuccess(socksHttpGet(policyPort, "api.ipify.org"))
            assertTrue(client.isReady)
            println("LIVE_ONION,disabled_policy_code=5,root_still_ready=true")
            println("LIVE SESSION LIFECYCLE passed: two session paths, independent close, root survival, pause/resume, stale handles")
        }
    }

    private suspend fun CoroutineScope.withLiveClient(block: suspend (ArtiTorClient) -> Unit) {
        val client = ArtiTorClient()
        // POSIX SOCKS operations block the test thread. Collect on another
        // dispatcher, subscribing synchronously before bootstrap can emit logs.
        val diagnostics = launch(Dispatchers.Default, start = CoroutineStart.UNDISPATCHED) {
            client.logs.collect { line ->
                safeSocksFailure.find(line)?.value?.let { println(it) }
            }
        }
        try {
            block(client)
        } catch (failure: Throwable) {
            // Give the independent diagnostics collector a bounded opportunity
            // to print the safe category already emitted before SOCKS 0x05.
            delay(100)
            throw failure
        } finally {
            client.shutdown()
            diagnostics.cancelAndJoin()
        }
    }

    companion object {
        private val safeSocksFailure = Regex("SOCKS CONNECT failed \\(code=5, arti_kind=[A-Za-z0-9_]+\\)")
    }

    @Test
    fun bootstrapFetchPauseResume() = runBlocking {
        val dataDir = NSTemporaryDirectory() + "arti-ios-e2e"
        withLiveClient { client ->
            println("arti version = ${client.version}")

            withTimeout(300_000) {
                client.start(ArtiConfig(dataDir = dataDir, socksPort = 0), timeout = 180.seconds)
                    .getOrThrow()
            }
            val port = requireNotNull(client.status.value.socksPort) { "SOCKS port not set" }
            assertEquals(100, client.status.value.bootstrapPercent, "bootstrap not 100%")
            assertTrue(client.isReady)
            println("BOOTSTRAP complete, SOCKS on 127.0.0.1:$port")

            val response = socksHttpGet(port, "api.ipify.org")
            assertTrue(response.startsWith("HTTP/1"), "no HTTP response through SOCKS")

            client.pause()
            assertFalse(client.isReady)
            assertTrue(client.hasClient)
            assertEquals(TorState.PAUSED, client.status.value.state)

            withTimeout(60_000) {
                client.resume(timeout = 45.seconds).getOrThrow()
            }
            assertTrue(client.isReady)
            val port2 = requireNotNull(client.status.value.socksPort)
            val response2 = socksHttpGet(port2, "api.ipify.org")
            assertTrue(response2.startsWith("HTTP/1"), "no HTTP after resume")

            client.shutdown()
            assertFalse(client.hasClient)
            assertEquals(TorState.OFF, client.status.value.state)

            // Cold start again after shutdown (fresh bootstrap, new ephemeral port).
            withTimeout(300_000) {
                client.start(ArtiConfig(dataDir = dataDir, socksPort = 0), timeout = 180.seconds)
                    .getOrThrow()
            }
            assertTrue(client.isReady)
            val port3 = requireNotNull(client.status.value.socksPort)
            val response3 = socksHttpGet(port3, "api.ipify.org")
            assertTrue(response3.startsWith("HTTP/1"), "no HTTP after second cold start")
            println("SECOND COLD START ok, SOCKS on 127.0.0.1:$port3")

            client.shutdown()
            assertFalse(client.hasClient)
            assertEquals(TorState.OFF, client.status.value.state)
        }
    }
}

@OptIn(ExperimentalForeignApi::class)
private fun socksHttpGet(socksPort: Int, host: String): String {
    val fd = socket(AF_INET, SOCK_STREAM, 0)
    check(fd >= 0) { "socket() failed" }
    try {
        memScoped {
            val deadline = alloc<timeval>()
            deadline.tv_sec = 60
            deadline.tv_usec = 0
            check(setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, deadline.ptr, sizeOf<timeval>().convert()) == 0)
            check(setsockopt(fd, SOL_SOCKET, SO_SNDTIMEO, deadline.ptr, sizeOf<timeval>().convert()) == 0)
            val addr = alloc<sockaddr_in>()
            addr.sin_family = AF_INET.convert()
            addr.sin_port = (((socksPort and 0xFF) shl 8) or ((socksPort shr 8) and 0xFF)).toUShort()
            addr.sin_addr.s_addr = 0x0100007Fu
            check(connect(fd, addr.ptr.reinterpret<sockaddr>(), sockaddr_in.size.convert()) == 0) {
                "connect to local SOCKS failed"
            }
        }

        writeAll(fd, byteArrayOf(0x05, 0x01, 0x00))
        val greeting = readN(fd, 2)
        check(greeting[1].toInt() == 0x00) { "SOCKS no-auth rejected" }

        val hb = host.encodeToByteArray()
        val req = byteArrayOf(0x05, 0x01, 0x00, 0x03, hb.size.toByte()) +
            hb + byteArrayOf(0x00, 80.toByte())
        writeAll(fd, req)
        val reply = readN(fd, 10)
        check(reply[1].toInt() == 0x00) { "SOCKS CONNECT failed (code=${reply[1]})" }

        writeAll(fd, "GET / HTTP/1.0\r\nHost: $host\r\nConnection: close\r\n\r\n".encodeToByteArray())

        val sb = StringBuilder()
        val buf = ByteArray(2048)
        while (true) {
            val n = buf.usePinned { recv(fd, it.addressOf(0), buf.size.convert(), 0) }
            check(n >= 0L) { "HTTP receive timed out or failed" }
            if (n == 0L) break
            sb.append(buf.decodeToString(0, n.toInt()))
            if (sb.length >= 65536) break
        }
        return sb.toString()
    } finally {
        close(fd)
    }
}

@OptIn(ExperimentalForeignApi::class)
private fun writeAll(fd: Int, bytes: ByteArray) {
    var off = 0
    bytes.usePinned { pinned ->
        while (off < bytes.size) {
            val n = send(fd, pinned.addressOf(off), (bytes.size - off).convert(), 0)
            check(n > 0L) { "send failed" }
            off += n.toInt()
        }
    }
}

@OptIn(ExperimentalForeignApi::class)
private fun readN(fd: Int, count: Int): ByteArray {
    val out = ByteArray(count)
    var off = 0
    out.usePinned { pinned ->
        while (off < count) {
            val n = recv(fd, pinned.addressOf(off), (count - off).convert(), 0)
            check(n > 0L) { "recv failed (eof)" }
            off += n.toInt()
        }
    }
    return out
}

private fun assertHttpSuccess(response: String) {
    val status = response.lineSequence().firstOrNull().orEmpty()
    assertTrue(status.matches(Regex("HTTP/1\\.[01] (200|301|302|307|308).*")), "unexpected HTTP status: $status")
}

@OptIn(ExperimentalForeignApi::class)
private fun socksReplyCode(socksPort: Int, host: String): Int {
    val fd = socket(AF_INET, SOCK_STREAM, 0)
    check(fd >= 0) { "socket() failed" }
    try {
        memScoped {
            val deadline = alloc<timeval>()
            deadline.tv_sec = 60
            deadline.tv_usec = 0
            check(setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, deadline.ptr, sizeOf<timeval>().convert()) == 0)
            check(setsockopt(fd, SOL_SOCKET, SO_SNDTIMEO, deadline.ptr, sizeOf<timeval>().convert()) == 0)
            val addr = alloc<sockaddr_in>()
            addr.sin_family = AF_INET.convert()
            addr.sin_port = (((socksPort and 0xFF) shl 8) or ((socksPort shr 8) and 0xFF)).toUShort()
            addr.sin_addr.s_addr = 0x0100007Fu
            check(connect(fd, addr.ptr.reinterpret<sockaddr>(), sockaddr_in.size.convert()) == 0) {
                "connect to local SOCKS failed"
            }
        }

        writeAll(fd, byteArrayOf(0x05, 0x01, 0x00))
        val greeting = readN(fd, 2)
        check(greeting[1].toInt() == 0x00) { "SOCKS no-auth rejected" }

        val hb = host.encodeToByteArray()
        val req = byteArrayOf(0x05, 0x01, 0x00, 0x03, hb.size.toByte()) +
            hb + byteArrayOf(0x00, 80.toByte())
        writeAll(fd, req)
        val reply = readN(fd, 10)
        return reply[1].toInt() and 0xff
    } finally { close(fd) }
}
