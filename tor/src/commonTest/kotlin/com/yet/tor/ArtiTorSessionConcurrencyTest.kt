package com.yet.tor

import com.yet.tor.ffi.ArtiConfig as FfiConfig
import com.yet.tor.ffi.ArtiException as FfiArtiException
import com.yet.tor.ffi.ArtiTorInterface as FfiArtiTorInterface
import com.yet.tor.ffi.NoHandle
import com.yet.tor.ffi.SessionState
import com.yet.tor.ffi.SessionStatusFfi
import com.yet.tor.ffi.SessionStatusListener
import com.yet.tor.ffi.SocksSession
import com.yet.tor.ffi.StatusListener
import com.yet.tor.ffi.TorState as FfiTorState
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertTrue
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.async
import kotlinx.coroutines.awaitAll
import kotlinx.coroutines.launch
import kotlinx.coroutines.flow.take
import kotlinx.coroutines.flow.toList
import kotlinx.coroutines.runBlocking

class ArtiTorSessionConcurrencyTest {

    private class FakeSessionNative(val scope: CoroutineScope) : FfiArtiTorInterface {
        var hasClientValue = false
        var readyPort: Int? = null
        var engineState = FfiTorState.OFF
        var sessionCounter = 0
        val sessions = mutableMapOf<String, SessionStatusFfi>()
        var sessionListener: SessionStatusListener? = null
        var createSessionBehavior: (SessionStatusListener) -> SocksSession = { _ ->
            throw UnsupportedOperationException("createSession not implemented")
        }
        var closeSessionBehavior: (SocksSession) -> Unit = { _ -> }
        var pauseBehavior: () -> Unit = {}
        var shutdownBehavior: () -> Unit = {}
        var startBehavior: (FfiConfig, StatusListener) -> Unit = { _, _ -> }
        var resumeBehavior: (StatusListener) -> Unit = { _ -> }
        var sessionStatusBehavior: ((SocksSession) -> SessionStatusFfi)? = null

        override fun version(): String = "fake"
        override fun hasClient(): Boolean = hasClientValue
        override fun isReady(): Boolean = readyPort != null && hasClientValue
        override fun socksPort(): UShort? = readyPort?.toUShort()
        override fun start(config: FfiConfig, listener: StatusListener) {
            startBehavior(config, listener)
        }
        override fun resume(listener: StatusListener) {
            resumeBehavior(listener)
        }
        override fun pause() {
            pauseBehavior()
        }
        override fun shutdown() {
            shutdownBehavior()
        }
        override fun stop() = shutdown()
        override fun createSession(listener: SessionStatusListener): SocksSession {
            sessionListener = listener
            return createSessionBehavior(listener)
        }
        override fun closeSession(session: SocksSession) {
            closeSessionBehavior(session)
        }
        override fun listSessions(): List<com.yet.tor.ffi.SessionInfo> {
            return sessions.map { (id, snap) ->
                com.yet.tor.ffi.SessionInfo(id, snap.state, snap.port)
            }
        }
        override fun sessionStatus(session: SocksSession): SessionStatusFfi {
            sessionStatusBehavior?.let { return it(session) }
            return sessions[session.id()] ?: SessionStatusFfi(SessionState.INVALIDATED, null, 3u)
        }
    }

    private class FakeSocksSession(
        private val sessionId: String,
        private val fake: FakeSessionNative,
        private val listener: SessionStatusListener,
    ) : SocksSession(NoHandle) {
        override fun id(): String = sessionId
        override fun generation(): ULong = 1u
        override fun closeSession() {
            fake.sessions[sessionId] = SessionStatusFfi(SessionState.CLOSED, null, 3u)
            listener.onSessionStatus(sessionId, SessionState.CLOSED, null, 3u)
        }
        override fun statusSnapshot(): SessionStatusFfi {
            return fake.sessions[sessionId] ?: SessionStatusFfi(SessionState.INVALIDATED, null, 3u)
        }
    }

    private fun createActiveSession(
        fake: FakeSessionNative,
        listener: SessionStatusListener,
    ): SocksSession {
        val id = "sess-${++fake.sessionCounter}"
        val port = 20000 + fake.sessionCounter
        fake.sessions[id] = SessionStatusFfi(SessionState.ACTIVE, port.toUShort(), 1u)
        val session = FakeSocksSession(id, fake, listener)
        fake.closeSessionBehavior = { s ->
            if (s is FakeSocksSession) s.closeSession()
        }
        listener.onSessionStatus(id, SessionState.ACTIVE, port.toUShort(), 1u)
        return session
    }

    @Test
    fun createMapsEveryGeneratedFailureToExactPublicClass() = runBlocking<Unit> {
        val failures = listOf(
            FfiArtiException.NotRunning() to ArtiException.NotRunning::class,
            FfiArtiException.AlreadyRunning() to ArtiException.AlreadyRunning::class,
            FfiArtiException.Bind(1234u, "collision") to ArtiException.Bind::class,
            FfiArtiException.Config("invalid") to ArtiException.Config::class,
            FfiArtiException.Bootstrap("failed") to ArtiException.Bootstrap::class,
            FfiArtiException.Runtime("session limit reached") to ArtiException.Runtime::class,
        )
        for ((failure, expected) in failures) {
            val fake = FakeSessionNative(this)
            fake.createSessionBehavior = { throw failure }
            val result = ArtiTorClient(fake).createIsolationSession()
            assertTrue(result.isFailure)
            assertEquals(expected, result.exceptionOrNull()!!::class)
        }
    }

    @Test
    fun terminalBeforeWrapperChurnDoesNotRetainHistoricalPendingStatuses() = runBlocking<Unit> {
        val fake = FakeSessionNative(this)
        val client = ArtiTorClient(fake)
        fake.createSessionBehavior = { listener ->
            val session = createActiveSession(fake, listener)
            session.closeSession()
            // Orphan callbacks during the creation window must also be cleared.
            listener.onSessionStatus("orphan-" + session.id(), SessionState.PAUSED, null, 2u)
            session
        }
        repeat(160) {
            val session = client.createIsolationSession().getOrThrow()
            assertEquals(TorIsolationSessionState.CLOSED, session.status.value.state)
            assertTrue(client.sessions.isEmpty())
            assertTrue(client.pendingSessionStatusCount == 0, "iteration $it retained pending history")
        }
    }

    @Test
    fun lateCallbacksAfterCloseChurnDoNotRetainHistoryOrResurrect() = runBlocking<Unit> {
        val fake = FakeSessionNative(this)
        fake.createSessionBehavior = { createActiveSession(fake, it) }
        val client = ArtiTorClient(fake)
        repeat(160) {
            val session = client.createIsolationSession().getOrThrow()
            session.close()
            fake.sessionListener!!.onSessionStatus(session.id, SessionState.ACTIVE, 20001u, 1u)
            fake.sessionListener!!.onSessionStatus(session.id, SessionState.PAUSED, null, 2u)
            // Native diagnostics can degrade after pruning a tombstone.
            fake.sessionListener!!.onSessionStatus(session.id, SessionState.INVALIDATED, null, 3u)
            assertEquals(TorIsolationSessionState.CLOSED, session.status.value.state)
            assertEquals(null, session.status.value.socksEndpoint)
            assertTrue(client.sessions.isEmpty())
            assertTrue(client.pendingSessionStatusCount == 0, "iteration $it retained pending history")
        }
    }

    @Test
    fun failedCreationClearsOrphanPendingStatuses() = runBlocking<Unit> {
        val fake = FakeSessionNative(this)
        val client = ArtiTorClient(fake)
        fake.createSessionBehavior = { listener ->
            listener.onSessionStatus("orphan", SessionState.PAUSED, null, 2u)
            throw FfiArtiException.Runtime("session limit reached")
        }
        assertTrue(client.createIsolationSession().isFailure)
        assertTrue(client.sessions.isEmpty())
        assertTrue(client.pendingSessionStatusCount == 0)
    }

    @Test
    fun delayedActiveAfterPausedIsRejectedByRevision() = runBlocking<Unit> {
        val fake = FakeSessionNative(this)
        fake.createSessionBehavior = { createActiveSession(fake, it) }
        val client = ArtiTorClient(fake)
        val session = client.createIsolationSession().getOrThrow()
        val listener = fake.sessionListener!!
        listener.onSessionStatus(session.id, SessionState.PAUSED, null, 2u)
        listener.onSessionStatus(session.id, SessionState.ACTIVE, 20001u, 1u)
        assertEquals(TorIsolationSessionState.PAUSED, session.status.value.state)
        assertEquals(null, session.status.value.socksEndpoint)
        assertEquals(listOf(session), client.sessions)
        // Equal revisions cannot replace state/endpoint either.
        listener.onSessionStatus(session.id, SessionState.ACTIVE, 21001u, 2u)
        assertEquals(TorIsolationSessionState.PAUSED, session.status.value.state)
        listener.onSessionStatus(session.id, SessionState.ACTIVE, 22001u, 3u)
        assertEquals(TorSocksEndpoint("127.0.0.1", 22001), session.status.value.socksEndpoint)
        // A delayed terminal event must not remove a newer live wrapper.
        listener.onSessionStatus(session.id, SessionState.CLOSED, null, 2u)
        assertEquals(listOf(session), client.sessions)
        assertEquals(TorIsolationSessionState.ACTIVE, session.status.value.state)
    }

    @Test
    fun invalidatedRevisionLatchesAgainstAllLaterCallbacks() = runBlocking<Unit> {
        val fake = FakeSessionNative(this)
        fake.createSessionBehavior = { createActiveSession(fake, it) }
        val client = ArtiTorClient(fake)
        val session = client.createIsolationSession().getOrThrow()
        val listener = fake.sessionListener!!
        listener.onSessionStatus(session.id, SessionState.INVALIDATED, null, 5u)
        listener.onSessionStatus(session.id, SessionState.ACTIVE, 20001u, 1u)
        listener.onSessionStatus(session.id, SessionState.PAUSED, null, 4u)
        listener.onSessionStatus(session.id, SessionState.CLOSED, null, 6u)
        assertEquals(TorIsolationSessionState.INVALIDATED, session.status.value.state)
        assertEquals(null, session.status.value.socksEndpoint)
        assertTrue(client.sessions.isEmpty())
        assertEquals(0, client.pendingSessionStatusCount)
    }

    @Test
    fun preRegistrationRevisionsAndSnapshotAreReconciledByRevision() = runBlocking<Unit> {
        val fake = FakeSessionNative(this)
        fake.createSessionBehavior = { listener ->
            val session = createActiveSession(fake, listener)
            listener.onSessionStatus(session.id(), SessionState.PAUSED, null, 2u)
            listener.onSessionStatus(session.id(), SessionState.ACTIVE, 20001u, 1u)
            session
        }
        val client = ArtiTorClient(fake)
        val session = client.createIsolationSession().getOrThrow()
        assertEquals(TorIsolationSessionState.PAUSED, session.status.value.state)
        assertEquals(null, session.status.value.socksEndpoint)
        assertEquals(0, client.pendingSessionStatusCount)
    }

    @Test
    fun newerNativeSnapshotWinsOverEarlierPendingCallback() = runBlocking<Unit> {
        val fake = FakeSessionNative(this)
        fake.createSessionBehavior = { listener ->
            val session = createActiveSession(fake, listener)
            fake.sessions[session.id()] = SessionStatusFfi(SessionState.PAUSED, null, 2u)
            session
        }
        val client = ArtiTorClient(fake)
        val session = client.createIsolationSession().getOrThrow()
        assertEquals(TorIsolationSessionState.PAUSED, session.status.value.state)
        assertEquals(null, session.status.value.socksEndpoint)
        assertEquals(0, client.pendingSessionStatusCount)
    }

    @Test
    fun closedBeforeWrapperRemainsClosedWhenNativeDiagnosticTombstoneWasPruned() = runBlocking<Unit> {
        val fake = FakeSessionNative(this)
        fake.createSessionBehavior = { listener ->
            val session = createActiveSession(fake, listener)
            session.closeSession()
            fake.sessions.remove(session.id())
            session
        }
        val client = ArtiTorClient(fake)
        val session = client.createIsolationSession().getOrThrow()
        assertEquals(TorIsolationSessionState.CLOSED, session.status.value.state)
        assertEquals(0, client.pendingSessionStatusCount)
        assertTrue(client.sessions.isEmpty())
    }

    @Test
    fun statusFlowProjectsRevisionChangesWithoutDuplicatePublicStatuses() = runBlocking<Unit> {
        val fake = FakeSessionNative(this)
        fake.createSessionBehavior = { createActiveSession(fake, it) }
        val client = ArtiTorClient(fake)
        val session = client.createIsolationSession().getOrThrow()
        val values = mutableListOf<TorIsolationSessionStatus>()
        val collector = launch(Dispatchers.Unconfined, start = CoroutineStart.UNDISPATCHED) {
            session.status.take(3).toList(values)
        }
        val listener = fake.sessionListener!!
        listener.onSessionStatus(session.id, SessionState.ACTIVE, 20001u, 2u)
        listener.onSessionStatus(session.id, SessionState.PAUSED, null, 3u)
        listener.onSessionStatus(session.id, SessionState.ACTIVE, 21001u, 2u)
        listener.onSessionStatus(session.id, SessionState.CLOSED, null, 4u)
        collector.join()
        assertEquals(
            listOf(TorIsolationSessionState.ACTIVE, TorIsolationSessionState.PAUSED, TorIsolationSessionState.CLOSED),
            values.map { it.state },
        )
        assertEquals(session.status.value, session.status.replayCache.single())
    }

    @Test
    fun concurrentCallbacksKeepHighestRevisionWithoutSecondaryPublicationRace() = runBlocking<Unit> {
        val fake = FakeSessionNative(this)
        fake.createSessionBehavior = { createActiveSession(fake, it) }
        val client = ArtiTorClient(fake)
        val session = client.createIsolationSession().getOrThrow()
        val listener = fake.sessionListener!!
        (2..200).map { revision ->
            async(Dispatchers.Default) {
                listener.onSessionStatus(session.id, SessionState.ACTIVE, 20001u, revision.toULong())
            }
        }.plus(async(Dispatchers.Default) {
            listener.onSessionStatus(session.id, SessionState.PAUSED, null, 201u)
        }).awaitAll()
        assertEquals(TorIsolationSessionState.PAUSED, session.status.value.state)
        assertEquals(null, session.status.value.socksEndpoint)
        assertEquals(listOf(session), client.sessions)
    }

    @Test
    fun closedSessionNeverAppearsInSessions() = runBlocking<Unit> {
        val fake = FakeSessionNative(this)
        fake.hasClientValue = true
        fake.readyPort = 19050
        fake.engineState = FfiTorState.RUNNING
        fake.createSessionBehavior = { listener ->
            createActiveSession(fake, listener)
        }
        val client = ArtiTorClient(fake)

        val session = client.createIsolationSession().getOrThrow()
        assertEquals(1, client.sessions.size)
        assertTrue(client.sessions.contains(session))

        session.close()
        assertEquals(0, client.sessions.size)
        assertFalse(client.sessions.any { it.status.value.state == TorIsolationSessionState.INVALIDATED })
    }

    @Test
    fun createWhileRunningProducesActiveSession() = runBlocking<Unit> {
        val fake = FakeSessionNative(this)
        fake.hasClientValue = true
        fake.readyPort = 19050
        fake.engineState = FfiTorState.RUNNING
        fake.createSessionBehavior = { listener ->
            createActiveSession(fake, listener)
        }
        val client = ArtiTorClient(fake)

        val session = client.createIsolationSession().getOrThrow()
        assertEquals(TorIsolationSessionState.ACTIVE, session.status.value.state)
        assertEquals(1, client.sessions.size)
        client.shutdown()
    }

    @Test
    fun createWhilePausedProducesPausedSession() = runBlocking<Unit> {
        val fake = FakeSessionNative(this)
        fake.hasClientValue = true
        fake.readyPort = null
        fake.engineState = FfiTorState.PAUSED
        fake.createSessionBehavior = { listener ->
            val id = "sess-${++fake.sessionCounter}"
            fake.sessions[id] = SessionStatusFfi(SessionState.PAUSED, null, 2u)
            val session = FakeSocksSession(id, fake, listener)
            listener.onSessionStatus(id, SessionState.PAUSED, null, 2u)
            session
        }
        val client = ArtiTorClient(fake)

        val session = client.createIsolationSession().getOrThrow()
        assertEquals(TorIsolationSessionState.PAUSED, session.status.value.state)
        assertEquals(1, client.sessions.size)
        client.shutdown()
    }

    @Test
    fun pauseDemotesActiveSessionToPaused() = runBlocking<Unit> {
        val fake = FakeSessionNative(this)
        fake.hasClientValue = true
        fake.readyPort = 19050
        fake.engineState = FfiTorState.RUNNING
        fake.createSessionBehavior = { listener ->
            createActiveSession(fake, listener)
        }
        fake.pauseBehavior = {
            fake.engineState = FfiTorState.PAUSED
            fake.readyPort = null
            val listener = fake.sessionListener
            if (listener != null) {
                for ((id, snap) in fake.sessions) {
                    if (snap.state == SessionState.ACTIVE) {
                        fake.sessions[id] = SessionStatusFfi(SessionState.PAUSED, null, 2u)
                        listener.onSessionStatus(id, SessionState.PAUSED, null, 2u)
                    }
                }
            }
        }
        val client = ArtiTorClient(fake)

        val session = client.createIsolationSession().getOrThrow()
        assertEquals(TorIsolationSessionState.ACTIVE, session.status.value.state)

        client.pause()
        assertEquals(TorIsolationSessionState.PAUSED, session.status.value.state)
        assertEquals(1, client.sessions.size)
        client.shutdown()
    }

    @Test
    fun shutdownInvalidatesActiveSession() = runBlocking<Unit> {
        val fake = FakeSessionNative(this)
        fake.hasClientValue = true
        fake.readyPort = 19050
        fake.engineState = FfiTorState.RUNNING
        fake.createSessionBehavior = { listener ->
            createActiveSession(fake, listener)
        }
        fake.shutdownBehavior = {
            fake.engineState = FfiTorState.OFF
            fake.hasClientValue = false
            fake.readyPort = null
            val listener = fake.sessionListener
            if (listener != null) {
                for (id in fake.sessions.keys.toList()) {
                    listener.onSessionStatus(id, SessionState.INVALIDATED, null, 3u)
                }
            }
            fake.sessions.clear()
        }
        val client = ArtiTorClient(fake)

        val session = client.createIsolationSession().getOrThrow()
        assertEquals(TorIsolationSessionState.ACTIVE, session.status.value.state)

        client.shutdown()
        assertEquals(TorIsolationSessionState.INVALIDATED, session.status.value.state)
        assertEquals(0, client.sessions.size)
    }

    @Test
    fun callbackBeforeWrapperRegistrationIsReconciled() = runBlocking<Unit> {
        val fake = FakeSessionNative(this)
        fake.hasClientValue = true
        fake.readyPort = 19050
        fake.engineState = FfiTorState.RUNNING
        fake.createSessionBehavior = { listener ->
            createActiveSession(fake, listener)
        }
        val client = ArtiTorClient(fake)

        val session = client.createIsolationSession().getOrThrow()
        assertEquals(TorIsolationSessionState.ACTIVE, session.status.value.state)
        assertTrue(client.sessions.contains(session))
        client.shutdown()
    }

    @Test
    fun closedCallbackDuringOlderActiveSnapshotIsReconciledAndLatched() = runBlocking<Unit> {
        assertTerminalDuringOlderActiveSnapshot(SessionState.CLOSED, TorIsolationSessionState.CLOSED)
    }

    @Test
    fun invalidatedCallbackDuringOlderActiveSnapshotIsReconciledAndLatched() = runBlocking<Unit> {
        assertTerminalDuringOlderActiveSnapshot(SessionState.INVALIDATED, TorIsolationSessionState.INVALIDATED)
    }

    private suspend fun CoroutineScope.assertTerminalDuringOlderActiveSnapshot(
        terminal: SessionState,
        expected: TorIsolationSessionState,
    ) {
        val fake = FakeSessionNative(this)
        fake.hasClientValue = true
        fake.readyPort = 19050
        fake.engineState = FfiTorState.RUNNING
        fake.createSessionBehavior = { listener ->
            // Suppress the initial ACTIVE callback so creation must query the
            // native snapshot before registering the Kotlin wrapper.
            val id = "sess-${++fake.sessionCounter}"
            fake.sessions[id] = SessionStatusFfi(SessionState.ACTIVE, 20001u, 1u)
            FakeSocksSession(id, fake, listener)
        }
        var snapshotCalls = 0
        fake.sessionStatusBehavior = { handle ->
            snapshotCalls++
            val olderActive = fake.sessions.getValue(handle.id())
            fake.sessions[handle.id()] = SessionStatusFfi(terminal, null, 3u)
            fake.sessionListener!!.onSessionStatus(handle.id(), terminal, null, 3u)
            // The callback is newer than this deliberately stale snapshot.
            olderActive
        }
        val client = ArtiTorClient(fake)

        val session = client.createIsolationSession().getOrThrow()
        assertEquals(1, snapshotCalls, "must exercise snapshot initialization")
        assertEquals(expected, session.status.value.state)
        assertEquals(null, session.status.value.socksEndpoint)
        assertEquals(0, client.sessions.size)
        assertTrue(client.pendingSessionStatusCount == 0, "pending terminal reconciliation is consumed")
        // Both stale live notifications and later terminal notifications must
        // leave the first terminal status latched in the returned wrapper.
        fake.sessionListener!!.onSessionStatus(session.id, SessionState.ACTIVE, 20001u, 1u)
        fake.sessionListener!!.onSessionStatus(session.id, SessionState.PAUSED, null, 2u)
        fake.sessionListener!!.onSessionStatus(session.id, SessionState.INVALIDATED, null, 3u)
        session.close()
        assertEquals(expected, session.status.value.state)
        assertEquals(null, session.status.value.socksEndpoint)
        assertFalse(client.sessions.contains(session))
        client.shutdown()
    }

    @Test
    fun sessionsContainsOnlyLiveSessions() = runBlocking<Unit> {
        val fake = FakeSessionNative(this)
        fake.hasClientValue = true
        fake.readyPort = 19050
        fake.engineState = FfiTorState.RUNNING
        fake.createSessionBehavior = { listener ->
            createActiveSession(fake, listener)
        }
        val client = ArtiTorClient(fake)

        val s1 = client.createIsolationSession().getOrThrow()
        val s2 = client.createIsolationSession().getOrThrow()
        assertEquals(2, client.sessions.size)

        s1.close()
        assertEquals(1, client.sessions.size)
        assertTrue(client.sessions.contains(s2))
        assertFalse(client.sessions.contains(s1))

        s2.close()
        assertEquals(0, client.sessions.size)
        client.shutdown()
    }
}
