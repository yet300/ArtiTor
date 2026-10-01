package com.yet.tor

import com.yet.tor.example.HttpPool
import com.yet.tor.example.HttpPoolFactory
import com.yet.tor.example.TorController
import com.yet.tor.ffi.ArtiTorInterface
import com.yet.tor.ffi.NoHandle
import com.yet.tor.ffi.SessionInfo
import com.yet.tor.ffi.SessionState
import com.yet.tor.ffi.SessionStatusFfi
import com.yet.tor.ffi.SessionStatusListener
import com.yet.tor.ffi.SocksSession
import com.yet.tor.ffi.StatusListener
import com.yet.tor.ffi.ArtiConfig as FfiConfig
import com.yet.tor.ffi.TorState as FfiState
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.yield
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertNotEquals
import kotlin.test.assertTrue

/** Application sample contracts: no network or platform HTTP dependency. */
class TorControllerExampleTest {
    private class Native : ArtiTorInterface {
        private var rootListener: StatusListener? = null
        private var state = FfiState.OFF
        private var nextPort: UShort = 21000u
        private val entries = mutableMapOf<String, Entry>()
        private class Entry(val handle: SocksSession, val listener: SessionStatusListener, var status: SessionStatusFfi)

        override fun version() = "example-test"
        override fun hasClient() = state == FfiState.RUNNING || state == FfiState.PAUSED
        override fun isReady() = state == FfiState.RUNNING
        override fun socksPort(): UShort? = if (isReady()) 19050u else null
        override fun start(config: FfiConfig, listener: StatusListener) {
            rootListener = listener
            state = FfiState.RUNNING
            listener.onStatus(state, 100u, 19050u, "ready")
        }
        private fun transition(target: SessionState) {
            entries.values.forEach {
                if (it.status.state == SessionState.CLOSED || it.status.state == SessionState.INVALIDATED) return@forEach
                val port = if (target == SessionState.ACTIVE) nextPort++ else null
                it.status = SessionStatusFfi(target, port, it.status.revision + 1u)
                it.listener.onSessionStatus(it.handle.id(), target, port, it.status.revision)
            }
        }
        override fun pause() {
            state = FfiState.PAUSED
            transition(SessionState.PAUSED)
            rootListener?.onStatus(state, 100u, null, "paused")
        }
        override fun resume(listener: StatusListener) {
            state = FfiState.RUNNING
            transition(SessionState.ACTIVE)
            listener.onStatus(state, 100u, 19050u, "ready")
        }
        override fun shutdown() {
            state = FfiState.OFF
            transition(SessionState.INVALIDATED)
            rootListener?.onStatus(state, 0u, null, "off")
        }
        override fun stop() = shutdown()
        override fun createSession(listener: SessionStatusListener): SocksSession {
            val id = "session-${nextPort++}"
            val handle = object : SocksSession(NoHandle) {
                override fun id() = id
                override fun generation(): ULong = 1u
                override fun closeSession() { this@Native.closeSession(this) }
                override fun statusSnapshot() = entries.getValue(id).status
            }
            val snapshot = SessionStatusFfi(SessionState.ACTIVE, nextPort++, 1u)
            entries[id] = Entry(handle, listener, snapshot)
            return handle
        }
        override fun closeSession(session: SocksSession) {
            val entry = entries.getValue(session.id())
            entry.status = SessionStatusFfi(SessionState.CLOSED, null, entry.status.revision + 1u)
            entry.listener.onSessionStatus(session.id(), entry.status.state, null, entry.status.revision)
        }
        override fun listSessions(): List<SessionInfo> = entries.map { (id, entry) ->
            SessionInfo(id, entry.status.state, entry.status.port)
        }
        override fun sessionStatus(session: SocksSession) = entries.getValue(session.id()).status
    }

    private class Pool : HttpPool {
        var closed = false
        var requests = 0
        override suspend fun get(url: String): ByteArray {
            check(!closed)
            requests++
            return byteArrayOf(1)
        }
        override fun close() { closed = true }
    }

    @Test
    fun identitiesUseSeparatePoolsAndResumeRebuildsCurrentEndpoints() = runBlocking<Unit> {
        val tor = ArtiTorClient(Native())
        tor.start(ArtiConfig("/tmp/example-test")).getOrThrow()
        val created = mutableListOf<Triple<String, TorSocksEndpoint, Pool>>()
        val controller = TorController(tor, this, HttpPoolFactory { identity, endpoint ->
            Pool().also { created += Triple(identity, endpoint, it) }
        })
        try {
            controller.get("alice", "https://example.invalid")
            controller.get("bob", "https://example.invalid")
            assertEquals(listOf("alice", "bob"), created.map { it.first })
            assertNotEquals(created[0].second, created[1].second)
            assertTrue(created[0].third !== created[1].third)
            tor.pause()
            yield()
            assertTrue(created.all { it.third.closed })
            assertFailsWith<IllegalStateException> { controller.get("alice", "https://example.invalid") }
            assertEquals(2, created.size)
            tor.resume().getOrThrow()
            yield()
            controller.get("alice", "https://example.invalid")
            assertEquals(3, created.size)
            assertEquals("alice", created.last().first)
            assertNotEquals(created.first().second, created.last().second)
            controller.get("bob", "https://example.invalid")
            val resumedAlice = created[2].third
            val resumedBob = created[3].third
            controller.closeIdentity("alice")
            assertTrue(resumedAlice.closed)
            assertTrue(!resumedBob.closed)
        } finally { controller.close(); tor.shutdown() }
    }

    @Test
    fun factoryFailureLeavesNoPoolAndTerminalSessionNeverFallsBack() = runBlocking<Unit> {
        val tor = ArtiTorClient(Native())
        tor.start(ArtiConfig("/tmp/example-test")).getOrThrow()
        var attempts = 0
        val pool = Pool()
        val controller = TorController(tor, this, HttpPoolFactory { _, _ ->
            attempts++
            if (attempts == 1) error("HTTP adapter initialization failed")
            pool
        })
        try {
            assertFailsWith<IllegalStateException> { controller.get("alice", "https://example.invalid") }
            assertEquals(0, pool.requests)
            controller.get("alice", "https://example.invalid")
            assertEquals(2, attempts)
            assertEquals(1, pool.requests)
            tor.shutdown()
            yield()
            assertTrue(pool.closed)
            assertFailsWith<IllegalStateException> { controller.get("alice", "https://example.invalid") }
            assertEquals(2, attempts)
            assertEquals(1, pool.requests)
        } finally { controller.close(); tor.shutdown() }
    }
}
