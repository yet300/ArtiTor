package com.yet.tor.example

import com.yet.tor.ArtiTorClient
import com.yet.tor.TorIsolationSession
import com.yet.tor.TorIsolationSessionState
import com.yet.tor.TorIsolationSessionStatus
import com.yet.tor.TorSocksEndpoint
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.collect
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock

/** App adapters must connect ONLY through endpoint, resolve targets via SOCKS,
 * and cancel outstanding requests when close() is called. */
interface HttpPool : AutoCloseable {
    suspend fun get(url: String): ByteArray
    override fun close()
}

/** Return a fresh pool for this identity/session/endpoint. Never cache globally,
 * use a shared default connection pool, or fall back to a direct connection. */
fun interface HttpPoolFactory {
    suspend fun create(identity: String, endpoint: TorSocksEndpoint): HttpPool
}

/** Application example, not an ArtiTor API. One handle and one pool per identity.
 * Circuit isolation partitions simultaneous app traffic; it does not guarantee
 * persistent identity unlinkability or a distinct exit relay. */
class TorController(
    private val tor: ArtiTorClient,
    private val scope: CoroutineScope,
    private val factory: HttpPoolFactory,
) {
    private val identitiesMutex = Mutex()
    private val identities = mutableMapOf<String, Identity>()
    private var closed = false

    suspend fun get(identity: String, url: String): ByteArray {
        val entry = identitiesMutex.withLock {
            check(!closed) { "controller is closed" }
            identities[identity] ?: Identity(identity, tor.createIsolationSession().getOrThrow()).also {
                identities[identity] = it
                it.observe()
            }
        }
        return entry.get(url)
    }

    suspend fun closeIdentity(identity: String) {
        identitiesMutex.withLock { identities.remove(identity) }?.close()
    }

    /** Call before disposing the application scope; root ownership stays in app. */
    suspend fun close() {
        val entries = identitiesMutex.withLock {
            closed = true
            identities.values.toList().also { identities.clear() }
        }
        entries.forEach { it.close() }
    }

    private inner class Identity(val id: String, val session: TorIsolationSession) {
        private val mutex = Mutex()
        private var pool: HttpPool? = null
        private var endpoint: TorSocksEndpoint? = null
        private var observed: TorIsolationSessionStatus? = null
        private var closed = false
        private var observer: Job? = null

        fun observe() {
            observer = scope.launch {
                session.status.collect { status ->
                    mutex.withLock {
                        if (observed != status) {
                            discardPool()
                            observed = status
                        }
                        // PAUSED/CLOSED/INVALIDATED have no usable endpoint.
                        if (status.state != TorIsolationSessionState.ACTIVE) discardPool()
                    }
                }
            }
        }

        suspend fun get(url: String): ByteArray {
            val selected = mutex.withLock {
                check(!closed) { "identity is closed" }
                val current = session.status.value
                val target = current.socksEndpoint
                if (!tor.isReady || current.state != TorIsolationSessionState.ACTIVE || target == null) {
                    discardPool()
                    error("Tor session is unavailable") // fail closed
                }
                if (observed != current || endpoint != target) discardPool()
                observed = current
                if (pool == null) {
                    val candidate = try {
                        factory.create(id, target)
                    } catch (failure: Throwable) {
                        discardPool()
                        throw failure
                    }
                    // Creation may suspend across pause/restart. Reject stale pool.
                    if (!tor.isReady || session.status.value != current) {
                        candidate.close()
                        discardPool()
                        error("Tor session changed while creating HTTP pool")
                    }
                    pool = candidate
                    endpoint = target
                }
                pool!! to target
            }
            // Read the current endpoint immediately before handing traffic to
            // the app adapter. Native teardown also aborts in-flight SOCKS I/O.
            val beforeRequest = session.status.value
            if (!tor.isReady || beforeRequest.socksEndpoint != selected.second ||
                beforeRequest.state != TorIsolationSessionState.ACTIVE) {
                mutex.withLock { discardPool() }
                error("Tor session changed before HTTP request")
            }
            return selected.first.get(url)
        }

        suspend fun close() {
            observer?.cancel()
            mutex.withLock {
                closed = true
                discardPool()
                session.close()
            }
        }

        private fun discardPool() {
            val old = pool
            pool = null
            endpoint = null
            old?.close()
        }
    }
}
