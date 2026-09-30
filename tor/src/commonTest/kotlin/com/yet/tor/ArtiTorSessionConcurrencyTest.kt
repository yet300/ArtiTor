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
import kotlin.test.assertNull
import kotlin.test.assertTrue
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.async
import kotlinx.coroutines.awaitAll
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.withTimeout
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.onSubscription
import kotlinx.coroutines.launch
import kotlinx.coroutines.flow.take
import kotlinx.coroutines.flow.toList
import kotlinx.coroutines.runBlocking

class ArtiTorSessionConcurrencyTest {

    @Test
    fun a01CachedActiveCannotPublishAfterCompletedClose() = runBlocking<Unit> {
        assertCachedActiveCannotPublishAfterTerminal(TorIsolationSessionState.CLOSED)
    }

    @Test
    fun a01CachedActiveCannotPublishAfterCompletedInvalidation() = runBlocking<Unit> {
        assertCachedActiveCannotPublishAfterTerminal(TorIsolationSessionState.INVALIDATED)
    }

    private suspend fun CoroutineScope.assertCachedActiveCannotPublishAfterTerminal(
        terminal: TorIsolationSessionState,
    ) {
        val fake = FakeSessionNative(this)
        fake.createSessionBehavior = { createActiveSession(fake, it) }
        val reachedPublication = CompletableDeferred<Unit>()
        val releasePublication = CompletableDeferred<Unit>()
        val gated = kotlinx.coroutines.flow.MutableStateFlow(false)
        val client = ArtiTorClient(fake) { _, revision ->
            if (revision == 2uL && gated.compareAndSet(false, true)) {
                reachedPublication.complete(Unit)
                // Only the test gate blocks; the production algorithm holds no lock.
                runBlocking { withTimeout(10_000) { releasePublication.await() } }
            }
        }
        val session = client.createIsolationSession().getOrThrow()
        val history = mutableListOf<TorIsolationSessionStatus>()
        val collector = launch(Dispatchers.Unconfined, start = CoroutineStart.UNDISPATCHED) {
            session.status.collect { history += it }
        }
        val staleWriter = async(Dispatchers.Default) {
            fake.sessionListener!!.onSessionStatus(session.id, SessionState.ACTIVE, 21001u, 2u)
        }
        try {
            withTimeout(10_000) { reachedPublication.await() }
            if (terminal == TorIsolationSessionState.CLOSED) {
                session.close()
            } else {
                fake.shutdownBehavior = {
                    fake.sessionListener!!.onSessionStatus(session.id, SessionState.INVALIDATED, null, 3u)
                    fake.sessions.clear()
                }
                client.shutdown()
            }
            assertEquals(terminal, session.status.value.state, "lifecycle operation completed")
            assertNull(session.status.value.socksEndpoint)
            assertTrue(client.sessions.isEmpty())
            assertEquals(0, client.pendingSessionStatusCount)
            releasePublication.complete(Unit)
            withTimeout(10_000) { staleWriter.await() }
            assertEquals(terminal, session.status.value.state)
            assertNull(session.status.value.socksEndpoint)
            assertTrue(client.sessions.isEmpty())
            assertEquals(0, client.pendingSessionStatusCount)
            val terminalIndex = history.indexOfFirst { it.state == terminal }
            assertTrue(terminalIndex >= 0)
            assertTrue(
                history.drop(terminalIndex).all { it.state == terminal && it.socksEndpoint == null },
                "terminal history resurrected: $history",
            )
            assertEquals(listOf(TorIsolationSessionState.ACTIVE, terminal), history.map { it.state })
        } finally {
            releasePublication.complete(Unit)
            withTimeout(10_000) { staleWriter.await() }
            collector.cancelAndJoin()
        }
    }

    @Test
    fun a01FiniteCompetingWritersCompleteAndConverge() = runBlocking<Unit> {
        val fake = FakeSessionNative(this)
        fake.createSessionBehavior = { createActiveSession(fake, it) }
        val client = ArtiTorClient(fake)
        val session = client.createIsolationSession().getOrThrow()
        val start = CompletableDeferred<Unit>()
        val ready = List(8) { CompletableDeferred<Unit>() }
        val writers = ready.mapIndexed { writer, signal ->
            async(Dispatchers.Default) {
                signal.complete(Unit)
                start.await()
                repeat(32) { step ->
                    val revision = (2 + step * 8 + writer).toULong()
                    val last = revision == 257uL
                    fake.sessionListener!!.onSessionStatus(
                        session.id,
                        if (last) SessionState.PAUSED else SessionState.ACTIVE,
                        if (last) null else (21000 + writer).toUShort(),
                        revision,
                    )
                }
            }
        }
        withTimeout(10_000) {
            ready.forEach { it.await() }
            start.complete(Unit)
            writers.awaitAll()
        }
        assertEquals(TorIsolationSessionState.PAUSED, session.status.value.state)
        assertNull(session.status.value.socksEndpoint)
        assertEquals(listOf(session), client.sessions)
        assertEquals(0, client.pendingSessionStatusCount)
    }

    @Test
    fun activePublicationCollectorReentersCloseWithoutResurrection() = runBlocking<Unit> {
        val fake = FakeSessionNative(this)
        fake.createSessionBehavior = { createActiveSession(fake, it) }
        val client = ArtiTorClient(fake)
        val session = client.createIsolationSession().getOrThrow()
        val listener = fake.sessionListener!!
        listener.onSessionStatus(session.id, SessionState.PAUSED, null, 2u)
        fake.closeSessionBehavior = {
            fake.sessions[session.id] = SessionStatusFfi(SessionState.CLOSED, null, 4u)
            listener.onSessionStatus(session.id, SessionState.CLOSED, null, 4u)
        }
        val history = mutableListOf<TorIsolationSessionStatus>()
        val collector = launch(Dispatchers.Unconfined, start = CoroutineStart.UNDISPATCHED) {
            session.status.collect {
                history += it
                if (it.state == TorIsolationSessionState.ACTIVE) session.close()
            }
        }
        listener.onSessionStatus(session.id, SessionState.ACTIVE, 21001u, 3u)
        assertEquals(TorIsolationSessionState.CLOSED, session.status.value.state)
        assertNull(session.status.value.socksEndpoint)
        assertTrue(client.sessions.isEmpty())
        assertEquals(0, client.pendingSessionStatusCount)
        assertEquals(
            listOf(TorIsolationSessionState.PAUSED, TorIsolationSessionState.ACTIVE, TorIsolationSessionState.CLOSED),
            history.map { it.state },
        )
        collector.cancelAndJoin()
    }

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
    @Test
    fun auditOnSubscriptionIsInvokedBeforeInitialReplay() = runBlocking<Unit> {
        val fake = FakeSessionNative(this)
        fake.createSessionBehavior = { createActiveSession(fake, it) }
        val session = ArtiTorClient(fake).createIsolationSession().getOrThrow()
        var controlCalls = 0
        kotlinx.coroutines.flow.MutableStateFlow(session.status.value)
            .onSubscription { controlCalls++ }.first()
        assertEquals(1, controlCalls, "supported implementation control")
        var calls = 0
        val initial = withTimeout(2000) { session.status.onSubscription { calls++ }.first() }
        assertEquals(session.status.value, initial)
        assertEquals(1, calls, "StateFlow onSubscription action must run before replay")
    }

    @Test
    fun auditCollectorCancellationPropagates() = runBlocking<Unit> {
        val fake = FakeSessionNative(this)
        fake.createSessionBehavior = { createActiveSession(fake, it) }
        val session = ArtiTorClient(fake).createIsolationSession().getOrThrow()
        var completed = false
        val job = launch(Dispatchers.Unconfined, start = CoroutineStart.UNDISPATCHED) {
            try { session.status.collect { } } finally { completed = true }
        }
        assertTrue(job.isActive)
        withTimeout(2000) { job.cancelAndJoin() }
        assertTrue(completed)
    }

    @Test
    fun auditNestedCollectorCanCloseWithoutDeadlock() = runBlocking<Unit> {
        val fake = FakeSessionNative(this)
        fake.createSessionBehavior = { createActiveSession(fake, it) }
        val client = ArtiTorClient(fake)
        val session = client.createIsolationSession().getOrThrow()
        val observed = mutableListOf<TorIsolationSessionState>()
        val job = launch(Dispatchers.Unconfined, start = CoroutineStart.UNDISPATCHED) {
            session.status.collect {
                observed += it.state
                if (it.state == TorIsolationSessionState.PAUSED) session.close()
            }
        }
        fake.sessionListener!!.onSessionStatus(session.id, SessionState.PAUSED, null, 2u)
        assertEquals(TorIsolationSessionState.CLOSED, session.status.value.state)
        assertTrue(client.sessions.isEmpty())
        assertEquals(
            listOf(TorIsolationSessionState.ACTIVE, TorIsolationSessionState.PAUSED, TorIsolationSessionState.CLOSED),
            observed,
        )
        assertEquals(0, client.pendingSessionStatusCount)
        fake.sessionListener!!.onSessionStatus(session.id, SessionState.PAUSED, null, 2u)
        assertEquals(3, observed.size, "retry and stale publication must not duplicate transitions")
        assertTrue(client.sessions.isEmpty())
        withTimeout(2000) { job.cancelAndJoin() }
    }

    @Test
    fun latestReplayAndSameStateRevisionFence() = runBlocking<Unit> {
        val fake = FakeSessionNative(this)
        fake.createSessionBehavior = { createActiveSession(fake, it) }
        val client = ArtiTorClient(fake)
        val session = client.createIsolationSession().getOrThrow()
        val values = mutableListOf<TorIsolationSessionStatus>()
        val job = launch(Dispatchers.Unconfined, start = CoroutineStart.UNDISPATCHED) {
            session.status.collect { values += it }
        }
        val listener = fake.sessionListener!!
        listener.onSessionStatus(session.id, SessionState.ACTIVE, 20001u, 5u)
        listener.onSessionStatus(session.id, SessionState.PAUSED, null, 4u)
        assertEquals(1, values.size)
        assertEquals(TorIsolationSessionState.ACTIVE, session.status.value.state)
        listener.onSessionStatus(session.id, SessionState.PAUSED, null, 6u)
        assertEquals(session.status.value, session.status.first())
        assertEquals(session.status.value, session.status.replayCache.single())
        assertEquals(listOf(session), client.sessions)
        assertEquals(0, client.pendingSessionStatusCount)
        job.cancelAndJoin()
    }

    @Test
    fun activePublicationCollectorReentersPauseWithoutResurrection() = runBlocking<Unit> {
        val fake = FakeSessionNative(this)
        fake.createSessionBehavior = { createActiveSession(fake, it) }
        val client = ArtiTorClient(fake)
        val session = client.createIsolationSession().getOrThrow()
        val listener = fake.sessionListener!!
        listener.onSessionStatus(session.id, SessionState.PAUSED, null, 2u)
        fake.pauseBehavior = {
            fake.sessions[session.id] = SessionStatusFfi(SessionState.PAUSED, null, 4u)
            listener.onSessionStatus(session.id, SessionState.PAUSED, null, 4u)
        }
        val values = mutableListOf<TorIsolationSessionState>()
        val job = launch(Dispatchers.Unconfined, start = CoroutineStart.UNDISPATCHED) {
            session.status.collect {
                values += it.state
                if (it.state == TorIsolationSessionState.ACTIVE) client.pause()
            }
        }
        listener.onSessionStatus(session.id, SessionState.ACTIVE, 21001u, 3u)
        listener.onSessionStatus(session.id, SessionState.ACTIVE, 21001u, 3u)
        assertEquals(TorIsolationSessionState.PAUSED, session.status.value.state)
        assertEquals(listOf(TorIsolationSessionState.PAUSED, TorIsolationSessionState.ACTIVE, TorIsolationSessionState.PAUSED), values)
        assertEquals(listOf(session), client.sessions)
        assertEquals(0, client.pendingSessionStatusCount)
        job.cancelAndJoin()
    }
    @Test
    fun typedErrorCollectorShutdownFinishesFacadeOffWithNoLastError() = runBlocking<Unit> {
        val fake = FakeSessionNative(this)
        var engineListener: StatusListener? = null
        fake.startBehavior = { _, listener ->
            engineListener = listener
            fake.hasClientValue = true
            fake.readyPort = 19050
            listener.onStatus(FfiTorState.RUNNING, 100u, 19050u, "ready")
        }
        fake.createSessionBehavior = { createActiveSession(fake, it) }
        fake.shutdownBehavior = {
            fake.hasClientValue = false
            fake.readyPort = null
            for (id in fake.sessions.keys.toList()) {
                fake.sessionListener!!.onSessionStatus(id, SessionState.INVALIDATED, null, 3u)
            }
            fake.sessions.clear()
            engineListener!!.onStatus(FfiTorState.OFF, 0u, null, "")
        }
        val client = ArtiTorClient(fake)
        client.start(ArtiConfig(dataDir = "/tmp/facade-error-reentry")).getOrThrow()
        val session = client.createIsolationSession().getOrThrow()
        val observed = mutableListOf<TorState>()
        val job = launch(Dispatchers.Unconfined, start = CoroutineStart.UNDISPATCHED) {
            client.status.collect {
                observed += it.state
                if (it.lastError != null) client.shutdown()
            }
        }
        // Native ordering/freshness is tested against production notify_worker_error
        // in Rust. Here exercise the real facade during its typed callback assignment.
        fake.sessionListener!!.onSessionStatus(session.id, SessionState.PAUSED, null, 2u)
        engineListener!!.onError(com.yet.tor.ffi.ArtiErrorDetail(com.yet.tor.ffi.ErrorKind.RUNTIME, null, "injected"))
        assertEquals(TorState.OFF, client.status.value.state)
        assertEquals(null, client.status.value.lastError)
        assertFalse(client.hasClient)
        assertEquals(TorIsolationSessionState.INVALIDATED, session.status.value.state)
        assertTrue(client.sessions.isEmpty())
        assertEquals(0, client.pendingSessionStatusCount)
        assertEquals(listOf(TorState.RUNNING, TorState.ERROR, TorState.OFF), observed)
        job.cancelAndJoin()
    }
}
