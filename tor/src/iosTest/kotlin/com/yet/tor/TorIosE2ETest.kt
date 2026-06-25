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
import kotlin.test.assertTrue

/**
 * iOS-simulator end-to-end proof: bootstrap to 100% over the real Tor network
 * (first-class status, not log scraping), then fetch through the local SOCKS
 * proxy via a raw SOCKS5 CONNECT and confirm an HTTP response comes back.
 */
class TorIosE2ETest {

    @Test
    fun bootstrapAndFetchThroughTor() = runBlocking {
        val dataDir = NSTemporaryDirectory() + "arti-ios-e2e"
        val client = ArtiTorClient()
        println("arti version = ${client.version}")

        withTimeout(300_000) {
            client.start(ArtiConfig(dataDir = dataDir, socksPort = 19061)).getOrThrow()
        }
        val port = requireNotNull(client.status.value.socksPort) { "SOCKS port not set" }
        assertEquals(100, client.status.value.bootstrapPercent, "bootstrap not 100%")
        println("BOOTSTRAP complete, SOCKS on 127.0.0.1:$port")

        val response = socksHttpGet(port, "api.ipify.org")
        println("TOR(SOCKS) response =\n$response")
        assertTrue(response.startsWith("HTTP/1"), "no HTTP response through SOCKS")

        client.stop()
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
            // Network byte order, little-endian host (all Apple targets): swap bytes.
            addr.sin_port = (((socksPort and 0xFF) shl 8) or ((socksPort shr 8) and 0xFF)).toUShort()
            // 127.0.0.1 in network order, stored on a little-endian host = 0x0100007F.
            addr.sin_addr.s_addr = 0x0100007Fu
            check(connect(fd, addr.ptr.reinterpret<sockaddr>(), sockaddr_in.size.convert()) == 0) {
                "connect to local SOCKS failed"
            }
        }

        // SOCKS5 greeting: version 5, 1 method, no-auth.
        writeAll(fd, byteArrayOf(0x05, 0x01, 0x00))
        val greeting = readN(fd, 2)
        check(greeting[1].toInt() == 0x00) { "SOCKS no-auth rejected" }

        // CONNECT to host:80 by domain name (ATYP 0x03) -> remote DNS via Tor.
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
