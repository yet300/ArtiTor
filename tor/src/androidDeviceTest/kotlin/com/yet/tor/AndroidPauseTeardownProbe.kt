package com.yet.tor

import android.system.ErrnoException
import android.system.OsConstants
import android.util.Log
import androidx.test.platform.app.InstrumentationRegistry
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import org.junit.Assert.*
import org.junit.Test
import java.io.File
import java.net.ConnectException
import java.net.InetSocketAddress
import java.net.Socket
import kotlin.time.Duration.Companion.seconds

/** Local SOCKS greeting/refusal gate. Never sends CONNECT or resolves a target. */
class AndroidPauseTeardownProbe {
    private fun greeting(port: Int) = Socket().use { socket ->
        socket.connect(InetSocketAddress("127.0.0.1", port), 5000)
        socket.soTimeout = 5000
        socket.getOutputStream().apply { write(byteArrayOf(5, 1, 0)); flush() }
        assertEquals(5, socket.getInputStream().read())
        assertEquals(0, socket.getInputStream().read())
    }

    private fun refused(port: Int) {
        try {
            Socket().use { it.connect(InetSocketAddress("127.0.0.1", port), 1500) }
            fail("old listener accepted TCP after synchronous teardown: $port")
        } catch (error: ConnectException) {
            val errno = generateSequence(error as Throwable?) { it.cause }
                .filterIsInstance<ErrnoException>().firstOrNull()?.errno
            assertEquals(OsConstants.ECONNREFUSED, errno)
        }
    }

    @Test fun hundredLocalPauseCycles(): Unit = runBlocking {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val config = ArtiConfig(File(context.filesDir, "arti-pause-teardown").apply { mkdirs() }.absolutePath)
        val tor = ArtiTorClient()
        val timings = mutableListOf<Long>()
        try {
            tor.start(config, 180.seconds).getOrThrow()
            repeat(50) { greeting(requireNotNull(tor.socksEndpoint).port) }
            repeat(100) {
                val s = tor.createIsolationSession().getOrThrow()
                val port = requireNotNull(s.status.value.socksEndpoint).port
                greeting(port)
                s.close()
                assertEquals(TorIsolationSessionState.CLOSED, s.status.value.state)
                refused(port)
            }
            val session = tor.createIsolationSession().getOrThrow()
            repeat(100) { cycle ->
                val oldRoot = requireNotNull(tor.socksEndpoint).port
                val oldSession = requireNotNull(session.status.value.socksEndpoint).port
                val begin = System.nanoTime()
                tor.pause()
                timings += System.nanoTime() - begin
                assertEquals(TorState.PAUSED, tor.status.value.state)
                assertNull(tor.socksEndpoint)
                assertTrue(tor.hasClient)
                assertEquals(TorIsolationSessionState.PAUSED, session.status.value.state)
                assertNull(session.status.value.socksEndpoint)
                refused(oldRoot)
                refused(oldSession)
                tor.resume(45.seconds).getOrThrow()
                greeting(requireNotNull(tor.socksEndpoint).port)
                val active = withTimeout(30_000) { session.status.first {
                    it.state == TorIsolationSessionState.ACTIVE && it.socksEndpoint != null
                } }
                greeting(requireNotNull(active.socksEndpoint).port)
                Log.i("ArtiPauseTeardown", "CYCLE_PASS,index=$cycle,old_root_refused=true,old_session_refused=true,pause_ns=${timings.last()}")
            }
            val oldRoot = requireNotNull(tor.socksEndpoint).port
            val oldSession = requireNotNull(session.status.value.socksEndpoint).port
            tor.shutdown()
            refused(oldRoot); refused(oldSession)
            assertFalse(tor.hasClient)
            assertEquals(TorIsolationSessionState.INVALIDATED, session.status.value.state)
            tor.start(config, 180.seconds).getOrThrow()
            greeting(requireNotNull(tor.socksEndpoint).port)
            val fresh = tor.createIsolationSession().getOrThrow()
            greeting(requireNotNull(fresh.status.value.socksEndpoint).port)
            assertEquals(TorIsolationSessionState.INVALIDATED, session.status.value.state)
            val sorted = timings.sorted()
            Log.i("ArtiPauseTeardown", "LOCAL_TEARDOWN_PASS,cycles=100,root_greetings=151,created_session_greetings=100,resumed_session_greetings=100,fresh_session_greetings=1,paused_old_refused=200,closed_old_refused=100,shutdown_old_refused=2,accepted_old=0,pause_min_ns=${sorted.first()},pause_median_ns=${sorted[50]},pause_max_ns=${sorted.last()}")
        } finally { tor.shutdown() }
    }
}
