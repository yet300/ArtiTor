package com.yet.tor

import com.yet.tor.ffi.*
import com.yet.tor.ffi.ArtiConfig as FfiConfig
import com.yet.tor.ffi.TorState as FfiState
import kotlinx.coroutines.runBlocking
import kotlin.test.*
import kotlin.time.Duration
import kotlin.time.Duration.Companion.seconds
import kotlin.time.Duration.Companion.nanoseconds
import kotlin.time.Duration.Companion.milliseconds

/** Offline: scripted retained resources; malformed input uses the actual Rust FFI parser. */
class TransactionalConfigurationTest {
    private class Native : ArtiTorInterface {
        var effective: FfiConfig? = null
        var held = false
        var state = FfiState.OFF
        var listener: StatusListener? = null
        var sessionListener: SessionStatusListener? = null
        var snapshot = SessionStatusFfi(SessionState.ACTIVE, 20001u, 1u)
        var shutdowns = 0
        val calls = mutableListOf<String>()
        val handle = object : SocksSession(NoHandle) {
            override fun id() = "audit-session"
            override fun generation(): ULong = 1u
            override fun closeSession() = Unit
            override fun statusSnapshot() = snapshot
        }
        override fun version() = "audit"
        override fun hasClient() = held
        override fun isReady() = held && state == FfiState.RUNNING
        override fun socksPort(): UShort? = if (isReady()) 19050u else null
        override fun start(config: FfiConfig, listener: StatusListener) {
            calls += "start"
            calls += "actual-rust-validation"
            validateConfig(config)
            if (held && effective != config) shutdown()
            effective = config
            this.listener = listener
            held = true; state = FfiState.RUNNING
            listener.onStatus(state, 100u, 19050u, "ready")
        }
        override fun pause() {
            state = FfiState.PAUSED
            snapshot = SessionStatusFfi(SessionState.PAUSED, null, 2u)
            sessionListener?.onSessionStatus(handle.id(), snapshot.state, null, snapshot.revision)
            listener?.onStatus(state, 100u, null, "paused")
        }
        override fun shutdown() {
            calls += "shutdown"; shutdowns++; held = false; state = FfiState.OFF
            snapshot = SessionStatusFfi(SessionState.INVALIDATED, null, 3u)
            sessionListener?.onSessionStatus(handle.id(), snapshot.state, null, snapshot.revision)
            listener?.onStatus(state, 0u, null, "")
        }
        override fun stop() = shutdown()
        override fun resume(listener: StatusListener) {
            state = FfiState.RUNNING
            snapshot = SessionStatusFfi(SessionState.ACTIVE, 20001u, snapshot.revision + 1u)
            sessionListener?.onSessionStatus(handle.id(), snapshot.state, snapshot.port, snapshot.revision)
            listener.onStatus(state, 100u, 19050u, "ready")
        }
        override fun createSession(listener: SessionStatusListener): SocksSession {
            sessionListener = listener
            listener.onSessionStatus(handle.id(), snapshot.state, snapshot.port, snapshot.revision)
            return handle
        }
        override fun closeSession(session: SocksSession) = Unit
        override fun listSessions() = if (held) listOf(SessionInfo(handle.id(), snapshot.state, snapshot.port)) else emptyList()
        override fun sessionStatus(session: SocksSession) = snapshot
    }

    @Test fun runningRejectedReplacementMustPreserveClient() = probe(false)
    @Test fun pausedRejectedReplacementPreservesClient() = probe(true)

    @Test fun mutableBridgeListMustNotBypassValidation() = runBlocking<Unit> {
        val native = Native()
        val client = ArtiTorClient(native)
        try {
            val lines = mutableListOf<String>()
            val good = ArtiConfig(dataDir = "/tmp/audit-offline", bridges = lines)
            client.start(good, 1.seconds).getOrThrow()
            val status = client.status.value
            lines += "203.0.113.44:49123 SECRET_BRIDGE_FINGERPRINT_MARKER"
            val result = client.start(good.copy(bridges = lines.toList()), 1.seconds)
            assertIs<ArtiException.Config>(result.exceptionOrNull(), "new malformed bridge input bypassed Rust validation")
            assertEquals(status, client.status.value)
            assertEquals(0, native.shutdowns)
            // A rejected request must not replace the last successful snapshot.
            client.restart(timeout = 1.seconds).getOrThrow()
            assertEquals(emptyList(), native.effective?.bridges)
        } finally { client.shutdown() }
    }

    @Test fun unrepresentablePortsPreserveRunningResources() = runBlocking<Unit> {
        val native = Native()
        val client = ArtiTorClient(native)
        try {
            val good = ArtiConfig(dataDir = "/tmp/transactional-offline")
            client.start(good, 1.seconds).getOrThrow()
            val session = client.createIsolationSession().getOrThrow()
            val status = client.status.value
            val sessionStatus = session.status.value
            for (port in listOf(-1, 65536, Int.MIN_VALUE, Int.MAX_VALUE)) {
                val bad = good.copy(socksPort = port)
                assertIs<ArtiException.Config>(client.start(bad, 1.seconds).exceptionOrNull())
                assertIs<ArtiException.Config>(client.restart(bad, 1.seconds).exceptionOrNull())
                assertEquals(status, client.status.value)
                assertEquals(sessionStatus, session.status.value)
                assertEquals(0, native.shutdowns)
            }
        } finally { client.shutdown() }
    }

    @Test fun rejectedRestartPreservesRunningAndPausedResources() = runBlocking<Unit> {
        for (paused in listOf(false, true)) {
            val native = Native()
            val client = ArtiTorClient(native)
            try {
                val good = ArtiConfig(dataDir = "/tmp/transactional-offline")
                client.start(good, 1.seconds).getOrThrow()
                val session = client.createIsolationSession().getOrThrow()
                if (paused) client.pause()
                val status = client.status.value
                val sessionStatus = session.status.value
                for (mode in BridgesEnabled.entries) {
                    val bad = good.copy(bridges = listOf("SECRET_BAD_BRIDGE"), bridgesEnabled = mode)
                    assertIs<ArtiException.Config>(client.restart(bad, 1.seconds).exceptionOrNull())
                    assertEquals(status, client.status.value)
                    assertEquals(sessionStatus, session.status.value)
                    assertTrue(client.hasClient)
                    assertEquals(0, native.shutdowns)
                }
            } finally { client.shutdown() }
        }
    }

    @Test fun validReplacementInvalidatesOldHandles() = runBlocking<Unit> {
        for (paused in listOf(false, true)) {
            val native = Native()
            val client = ArtiTorClient(native)
            try {
                val good = ArtiConfig(dataDir = "/tmp/transactional-offline")
                client.start(good, 1.seconds).getOrThrow()
                val session = client.createIsolationSession().getOrThrow()
                if (paused) client.pause()
                client.start(good.copy(bridgesEnabled = BridgesEnabled.OFF), 1.seconds).getOrThrow()
                assertEquals(TorIsolationSessionState.INVALIDATED, session.status.value.state)
                assertEquals(null, session.status.value.socksEndpoint)
                assertEquals(TorState.RUNNING, client.status.value.state)
                assertTrue(client.hasClient)
                assertTrue(client.sessions.isEmpty())
                assertEquals(1, native.shutdowns)
            } finally { client.shutdown() }
        }
    }

    @Test fun invalidTimeoutReplacementPreservesRunningAndPausedResources() = runBlocking<Unit> {
        for (paused in listOf(false, true)) {
            val native = Native()
            val client = ArtiTorClient(native)
            try {
                val good = ArtiConfig(dataDir = "/tmp/transactional-offline")
                client.start(good, 1.seconds).getOrThrow()
                val session = client.createIsolationSession().getOrThrow()
                if (paused) client.pause()
                val status = client.status.value
                val sessionStatus = session.status.value
                for (timeout in listOf((-1).nanoseconds, Duration.INFINITE, 10_000_000_000.seconds, 9_223_372_036_855.milliseconds)) {
                    for (bad in listOf(good.copy(connectTimeout = timeout), good.copy(resolveTimeout = timeout))) {
                        assertIs<ArtiException.Config>(client.start(bad, 1.seconds).exceptionOrNull())
                        assertIs<ArtiException.Config>(client.restart(bad, 1.seconds).exceptionOrNull())
                        assertEquals(status, client.status.value)
                        assertEquals(sessionStatus, session.status.value)
                        assertTrue(client.hasClient)
                        assertEquals(0, native.shutdowns)
                    }
                }
            } finally { client.shutdown() }
        }
    }

    @Test fun zeroAndLargestExactNanosecondTimeoutsCrossFfiWithoutClamping() = runBlocking<Unit> {
        for (timeout in listOf(Duration.ZERO, 9_223_372_036_854.milliseconds)) {
            val native = Native()
            val client = ArtiTorClient(native)
            try {
                val config = ArtiConfig(
                    dataDir = "/tmp/transactional-offline",
                    connectTimeout = timeout,
                    resolveTimeout = timeout,
                )
                client.start(config, 1.seconds).getOrThrow()
                assertEquals(timeout.inWholeNanoseconds, native.effective?.connectTimeoutNanos)
                assertEquals(timeout.inWholeNanoseconds, native.effective?.resolveTimeoutNanos)
            } finally { client.shutdown() }
        }
    }

    @Test fun validOnionAndTimeoutReplacementInvalidatesOldHandles() = runBlocking<Unit> {
        val good = ArtiConfig(dataDir = "/tmp/transactional-offline")
        for (replacement in listOf(
            good.copy(allowOnionAddrs = false),
            good.copy(connectTimeout = 5.seconds),
            good.copy(resolveTimeout = 5.seconds),
        )) {
            for (paused in listOf(false, true)) {
                val native = Native()
                val client = ArtiTorClient(native)
                try {
                    client.start(good, 1.seconds).getOrThrow()
                    val session = client.createIsolationSession().getOrThrow()
                    if (paused) client.pause()
                    client.start(replacement, 1.seconds).getOrThrow()
                    assertEquals(TorIsolationSessionState.INVALIDATED, session.status.value.state)
                    assertEquals(null, session.status.value.socksEndpoint)
                    assertEquals(TorState.RUNNING, client.status.value.state)
                    assertTrue(client.hasClient)
                    assertTrue(client.sessions.isEmpty())
                    assertEquals(1, native.shutdowns)
                } finally { client.shutdown() }
            }
        }
    }

    private fun probe(paused: Boolean) = runBlocking<Unit> {
        val native = Native()
        val client = ArtiTorClient(native)
        try {
            val good = ArtiConfig(dataDir = "/tmp/audit-offline")
            client.start(good, 1.seconds).getOrThrow()
            val session = client.createIsolationSession().getOrThrow()
            if (paused) client.pause()
            val expected = client.status.value.state
            val expectedSession = session.status.value
            val status = client.status.value
            for (mode in BridgesEnabled.entries) {
                val result = client.start(good.copy(bridges = listOf("203.0.113.44:49123 SECRET_BRIDGE_FINGERPRINT_MARKER"), bridgesEnabled = mode), 1.seconds)
                assertIs<ArtiException.Config>(result.exceptionOrNull())
                assertEquals(status, client.status.value)
                assertEquals(expectedSession, session.status.value)
                assertEquals(0, native.shutdowns)
            }
            assertTrue(client.hasClient, "rejected config destroyed retained client")
            assertEquals(expected, client.status.value.state)
            assertEquals(expectedSession, session.status.value)
            assertEquals(0, native.shutdowns)
        } finally {
            client.shutdown()
        }
    }
}
