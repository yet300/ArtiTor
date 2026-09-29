package com.yet.tor

import kotlinx.cinterop.ExperimentalForeignApi
import kotlinx.cinterop.addressOf
import kotlinx.cinterop.alloc
import kotlinx.cinterop.convert
import kotlinx.cinterop.memScoped
import kotlinx.cinterop.ptr
import kotlinx.cinterop.reinterpret
import kotlinx.cinterop.usePinned
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
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertTrue
import kotlin.time.Duration.Companion.seconds

/**
 * iOS-simulator end-to-end: bootstrap, SOCKS HTTP via Tor, pause/resume.
 */
class TorIosE2ETest {

    @Test
    fun bootstrapFetchPauseResume() = runBlocking {
        val dataDir = NSTemporaryDirectory() + "arti-ios-e2e"
        val client = ArtiTorClient()
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
        println("TOR(SOCKS) response =\n$response")
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

@OptIn(ExperimentalForeignApi::class)
private fun socksHttpGet(socksPort: Int, host: String): String {
    val fd = socket(AF_INET, SOCK_STREAM, 0)
    check(fd >= 0) { "socket() failed" }
    try {
        memScoped {
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
            if (n <= 0L) break
            sb.append(buf.decodeToString(0, n.toInt()))
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
