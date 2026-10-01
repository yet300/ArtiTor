package com.yet.tor

import android.util.Log
import androidx.test.platform.app.InstrumentationRegistry
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.File
import java.net.InetAddress
import java.net.ServerSocket
import kotlin.time.Duration.Companion.seconds

/** Hardware diagnostic: records immediate readiness separately from eventual session callbacks. */
class AndroidGateResumeProbe {
    @Test fun resumePublicationAndAsyncBindError() = runBlocking {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val config = ArtiConfig(File(context.filesDir, "arti-resume-probe").apply { mkdirs() }.absolutePath)
        val tor = ArtiTorClient()
        val status = launch(Dispatchers.Unconfined) { tor.status.collect {
            Log.i("ArtiResumeProbe", "ROOT,state=${it.state},kind=${it.lastError?.kind}")
        } }
        val logs = launch(Dispatchers.Unconfined) { tor.logs.collect { Log.i("ArtiResumeProbe", "LOG,$it") } }
        var sessionJob: Job? = null
        try {
            tor.start(config, 180.seconds).getOrThrow()
            val session = tor.createIsolationSession().getOrThrow()
            sessionJob = launch(Dispatchers.Unconfined) { session.status.collect {
                assertEquals(it.state == TorIsolationSessionState.ACTIVE, it.socksEndpoint != null)
                Log.i("ArtiResumeProbe", "SESSION,state=${it.state},endpoint=${it.socksEndpoint}")
            } }
            repeat(5) { cycle ->
                tor.pause()
                assertEquals(TorIsolationSessionState.PAUSED, session.status.value.state)
                assertTrue(tor.hasClient)
                val begin = System.nanoTime()
                tor.resume(45.seconds).getOrThrow()
                val immediate = session.status.value
                Log.i("ArtiResumeProbe", "IMMEDIATE,cycle=$cycle,root=${tor.status.value.state},session=${immediate.state},endpoint=${immediate.socksEndpoint},elapsed_us=${(System.nanoTime()-begin)/1000}")
                withTimeout(5000) { session.status.first { it.state == TorIsolationSessionState.ACTIVE } }
                Log.i("ArtiResumeProbe", "EVENTUAL,cycle=$cycle,elapsed_us=${(System.nanoTime()-begin)/1000}")
            }
            tor.pause()
            ServerSocket(0, 1, InetAddress.getByName("127.0.0.1")).use { occupied ->
                val error = tor.start(config.copy(socksPort = occupied.localPort), 45.seconds).exceptionOrNull()
                assertTrue(error is ArtiException.Bind)
                assertEquals(TorErrorKind.BIND, (error as ArtiException).kind)
                assertEquals(TorState.ERROR, tor.status.value.state)
                assertTrue(tor.status.value.lastError is ArtiException.Bind)
                assertEquals(TorIsolationSessionState.PAUSED, session.status.value.state)
                Log.i("ArtiResumeProbe", "ASYNC_BIND_PUBLIC,kind=${error.kind},status_kind=${tor.status.value.lastError?.kind}")
            }
            tor.shutdown()
            assertEquals(TorIsolationSessionState.INVALIDATED, session.status.value.state)
            assertNull(session.status.value.socksEndpoint)
            assertFalse(tor.hasClient)
        } finally { tor.shutdown(); sessionJob?.cancel(); status.cancel(); logs.cancel() }
    }
}
