package com.yet.tor

import com.yet.tor.ffi.SessionState
import com.yet.tor.ffi.SessionStatusListener
import com.yet.tor.ffi.SocksSession
import com.yet.tor.ffi.StatusListener
import com.yet.tor.ffi.validateConfig
import kotlinx.coroutines.TimeoutCancellationException
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withTimeout
import kotlin.time.Duration
import kotlin.time.Duration.Companion.seconds
import kotlin.time.Duration.Companion.nanoseconds
import com.yet.tor.ffi.ArtiConfig as FfiConfig
import com.yet.tor.ffi.ArtiErrorDetail as FfiErrorDetail
import com.yet.tor.ffi.ArtiException as FfiArtiException
import com.yet.tor.ffi.ArtiTor as FfiArtiTor
import com.yet.tor.ffi.ArtiTorInterface as FfiArtiTorInterface
import com.yet.tor.ffi.ErrorKind as FfiErrorKind
import com.yet.tor.ffi.BridgesEnabled as FfiBridgesEnabled
import com.yet.tor.ffi.TorState as FfiTorState
import com.yet.tor.ffi.TorErrorKind as FfiTorErrorKind

/** High-level lifecycle state (mirrors native UniFFI enum). */
enum class TorState {
    OFF,
    STARTING,
    BOOTSTRAPPING,
    RUNNING,
    /** Bootstrapped client retained; SOCKS is down. */
    PAUSED,
    STOPPING,
    ERROR,
}

/**
 * First-class status. [bootstrapPercent] comes from Arti's `bootstrap_events()`,
 * not log scraping. [socksPort] is non-null only while the local SOCKS listener
 * is bound.
 */
data class TorStatus(
    val state: TorState = TorState.OFF,
    val bootstrapPercent: Int = 0,
    val socksPort: Int? = null,
    val bootstrapSummary: String = "",
    val lastError: ArtiException? = null,
) {
    /** Proxy ready: bootstrapped, SOCKS accepting. */
    val isReady: Boolean
        get() = state == TorState.RUNNING &&
            bootstrapPercent >= 100 &&
            socksPort != null
}

/** Bridge policy. Every supplied line is validated in every mode. */
enum class BridgesEnabled {
    /** Historical default: use bridges iff the list is non-empty. */
    AUTO,
    /** Require bridges; an empty list fails with [ArtiException.Config]. */
    ON,
    /** Validate and retain supplied lines, but use normal guards. */
    OFF,
}

/**
 * Caller-supplied configuration. [dataDir] is required; the library never
 * invents platform paths.
 *
 * @param socksPort Local SOCKS bind. `0` = ephemeral (OS picks a free port);
 *   the actual port is reported in [TorStatus.socksPort].
 * @param bridges Direct bridge lines (one per entry), validated in Rust before
 *   bootstrap in every mode. Malformed lines fail with [ArtiException.Config].
 *   PT support is outside stable 0.3. Treat lines as secrets; do not log this
 *   config or persist it in unprotected storage.
 * @param bridgesEnabled Bridge policy; defaults to historical AUTO behavior.
 *   Changing this or [bridges] rebuilds TorClient and invalidates all sessions.
 *   Stable 0.3 does not use live reconfiguration.
 * @param stateDir Override for `$dataDir/state`.
 * @param cacheDir Override for `$dataDir/cache`.
 * @param allowOnionAddrs Allow public onion targets through SOCKS (default true).
 * @param connectTimeout Tor connection operation timeout (default 10 seconds).
 * @param resolveTimeout Tor resolution operation timeout (default 10 seconds).
 *   Timeouts must be finite, nonnegative, and exactly representable as signed
 *   64-bit nanoseconds. Changing any of these fields rebuilds the client and
 *   invalidates sessions. These are separate from readiness wait deadlines.
 */
data class ArtiConfig(
    val dataDir: String,
    val socksPort: Int = 0,
    val bridges: List<String> = emptyList(),
    val stateDir: String? = null,
    val cacheDir: String? = null,
    val bridgesEnabled: BridgesEnabled = BridgesEnabled.AUTO,
    val allowOnionAddrs: Boolean = true,
    val connectTimeout: Duration = 10.seconds,
    val resolveTimeout: Duration = 10.seconds,
)

/** Stable failure categories; independent of diagnostic message wording. */
enum class TorErrorKind {
    ALREADY_RUNNING, NOT_RUNNING, CONFIG, BIND, BOOTSTRAP, TIMEOUT, RUNTIME, NETWORK, EXIT_FAILED, TARGET_REJECTED, STORAGE, BOOTSTRAP_REQUIRED, SESSION_CLOSED, SESSION_INVALIDATED, UNKNOWN
}

/** Typed failures from the engine (maps UniFFI errors + Kotlin waits). */
sealed class ArtiException(message: String, cause: Throwable? = null) :
    Exception(message, cause) {
    abstract val kind: TorErrorKind
    class AlreadyRunning : ArtiException("Tor client already starting or running") {
        override val kind = TorErrorKind.ALREADY_RUNNING
    }
    class NotRunning : ArtiException("Tor client is not running") {
        override val kind = TorErrorKind.NOT_RUNNING
    }
    class Config(msg: String) : ArtiException("configuration error: $msg") {
        override val kind = TorErrorKind.CONFIG
    }
    class Bind(val port: Int, msg: String) : ArtiException("failed to bind SOCKS on $port: $msg") {
        override val kind = TorErrorKind.BIND
    }
    class Bootstrap internal constructor(msg: String, override val kind: TorErrorKind) :
        ArtiException("bootstrap failed: $msg") {
        constructor(msg: String) : this(msg, TorErrorKind.BOOTSTRAP)
    }
    class Timeout(msg: String = "timed out waiting for Tor ready") : ArtiException(msg) {
        override val kind = TorErrorKind.TIMEOUT
    }
    class Runtime(msg: String, override val kind: TorErrorKind = TorErrorKind.RUNTIME) :
        ArtiException("runtime error: $msg")
}

/**
 * True when [new] requires a new TorClient/bootstrap relative to [old].
 * Only `socksPort` may change without rebuilding; `dataDir`/`stateDir`/
 * `cacheDir`/`bridges`/`bridgesEnabled`/onion policy/timeouts (compared exactly)
 * define the client. Mirrors the native `tor_client_config_changed`.
 */
internal fun torClientConfigChanged(old: ArtiConfig, new: ArtiConfig): Boolean =
    old.dataDir != new.dataDir ||
        old.stateDir != new.stateDir ||
        old.cacheDir != new.cacheDir ||
        old.bridges != new.bridges ||
        old.bridgesEnabled != new.bridgesEnabled ||
        old.allowOnionAddrs != new.allowOnionAddrs ||
        old.connectTimeout != new.connectTimeout ||
        old.resolveTimeout != new.resolveTimeout

/**
 * State-machine invariant violations for an observed snapshot, per
 * docs/api-lifecycle-bitchat.md §2. Empty = all applicable invariants hold.
 * (STARTING/BOOTSTRAPPING worker existence is owned by the native layer and
 * cannot be observed from here; "no reported ready SOCKS" is checked.)
 */
internal fun checkLifecycleInvariants(
    state: TorState,
    hasClient: Boolean,
    socksPort: Int?,
    bootstrapPercent: Int,
    lastError: ArtiException?,
): List<String> {
    val violations = mutableListOf<String>()
    when (state) {
        TorState.OFF -> {
            if (hasClient) violations += "OFF must hold no client"
            if (socksPort != null) violations += "OFF must report no SOCKS port"
        }
        TorState.STARTING, TorState.BOOTSTRAPPING -> {
            if (socksPort != null) violations += "$state must not report a ready SOCKS port"
        }
        TorState.RUNNING -> {
            if (!hasClient) violations += "RUNNING requires a bootstrapped client"
            if (socksPort == null) violations += "RUNNING requires a SOCKS port"
            if (bootstrapPercent < 100) violations += "RUNNING requires bootstrap 100"
        }
        TorState.PAUSED -> {
            if (!hasClient) violations += "PAUSED requires a retained bootstrapped client"
            if (socksPort != null) violations += "PAUSED must report no SOCKS port"
            if (bootstrapPercent != 100) violations += "PAUSED requires bootstrap 100"
        }
        TorState.ERROR -> {
            if (lastError == null) violations += "ERROR requires a typed lastError"
            if (socksPort != null) violations += "ERROR must not report readiness"
        }
        TorState.STOPPING -> {
            if (socksPort != null) violations += "STOPPING must report no SOCKS port"
        }
    }
    return violations
}

/** SOCKS endpoint (always loopback in 0.3). */
data class TorSocksEndpoint(
    val host: String,
    val port: Int,
)

/** Per-session lifecycle state. */
enum class TorIsolationSessionState {
    ACTIVE,
    PAUSED,
    CLOSED,
    INVALIDATED;

    val isTerminal: Boolean
        get() = this == CLOSED || this == INVALIDATED

    val isLive: Boolean
        get() = this == ACTIVE || this == PAUSED
}

/** Atomic (state, endpoint) snapshot for one isolation session.
 *
 * Invariant: [socksEndpoint] != null ⟺ [state] == ACTIVE.
 */
data class TorIsolationSessionStatus(
    val state: TorIsolationSessionState,
    val socksEndpoint: TorSocksEndpoint?,
)

/** Opaque handle to an isolation session.
 *
 * Retained handles remain valid as terminal-state observers even after the
 * native session is destroyed. [status] emits exactly one terminal update
 * ([CLOSED] or [INVALIDATED]) which is latched forever.
 */
interface TorIsolationSession : AutoCloseable {
    val id: String
    val status: StateFlow<TorIsolationSessionStatus>
    val isClosed: Boolean
    override fun close()
}

/** Revision is an internal ordering token and never part of public session status. */
private data class RevisionedSessionStatus(
    val status: TorIsolationSessionStatus,
    val revision: ULong,
)

private fun sessionStatus(state: SessionState, port: UShort?, revision: ULong): RevisionedSessionStatus {
    val mapped = when (state) {
        SessionState.ACTIVE -> TorIsolationSessionState.ACTIVE
        SessionState.PAUSED -> TorIsolationSessionState.PAUSED
        SessionState.CLOSED -> TorIsolationSessionState.CLOSED
        SessionState.INVALIDATED -> TorIsolationSessionState.INVALIDATED
    }
    val endpoint = if (mapped == TorIsolationSessionState.ACTIVE && port != null) {
        TorSocksEndpoint("127.0.0.1", port.toInt())
    } else null
    return RevisionedSessionStatus(TorIsolationSessionStatus(mapped, endpoint), revision)
}

private class TorIsolationSessionImpl(
    val nativeSession: SocksSession,
    private val client: ArtiTorClient,
    initialStatus: RevisionedSessionStatus,
) : TorIsolationSession {
    // Keep the revision fence internal; the public surface uses supported StateFlow.
    private val accepted = MutableStateFlow(initialStatus)
    private val publicStatus = MutableStateFlow(initialStatus.status)
    override val id: String = nativeSession.id()
    override val status: StateFlow<TorIsolationSessionStatus> = publicStatus.asStateFlow()
    override val isClosed: Boolean
        get() = status.value.state == TorIsolationSessionState.CLOSED

    override fun close() {
        client.closeSession(this)
    }

    fun acceptStatus(incoming: RevisionedSessionStatus) {
        accepted.update { current ->
            if (current.status.state.isTerminal || incoming.revision <= current.revision) current
            else incoming
        }
        publishLatestAccepted()
    }

    private fun publishLatestAccepted() {
        // CAS can synchronously resume a collector that re-enters the facade.
        // Revalidate the accepted record, then condition publication on the
        // observed public value so an older writer cannot undo a newer one.
        while (true) {
            val target = accepted.value
            val observedPublic = publicStatus.value
            if (observedPublic == target.status) {
                if (accepted.value == target) return
                continue
            }
            if (accepted.value != target) continue
            client.beforeSessionStatusPublication?.invoke(id, target.revision)
            if (!publicStatus.compareAndSet(observedPublic, target.status)) continue
            // A resumed collector may have accepted/published a newer revision.
            if (accepted.value == target) return
        }
    }

}

private data class SessionRegistry(
    val wrappers: Map<String, TorIsolationSessionImpl> = emptyMap(),
    val pending: Map<String, RevisionedSessionStatus> = emptyMap(),
    val creating: Boolean = false,
)

/**
 * Cross-platform facade over the Arti FFI. The async tokio runtime lives inside
 * the native layer; this object never blocks the caller's thread.
 *
 * Lifecycle (see docs/api-lifecycle-bitchat.md):
 * - [start] — bootstrap if needed + SOCKS; suspends until ready
 * - [pause] — drop SOCKS, keep bootstrapped client; during STARTING/
 *   BOOTSTRAPPING cancels the bootstrap, discards any partial client, and
 *   settles to OFF (never stranded mid-bootstrap)
 * - [resume] — rebind SOCKS without full bootstrap
 * - [shutdown] — full teardown; status OFF
 * - [restart] — shutdown then start with the supplied or last-effective config
 *
 * "Proxy ready" == [TorStatus.isReady].
 *
 * Cancellation: [pause]/[shutdown] bump an internal epoch; any [start]/[resume]
 * waiter from before the bump fails fast with [ArtiException.Runtime]
 * ("cancelled") instead of waiting out its timeout.
 *
 * Process-global note: the native layer routes tracing logs to the most
 * recently installed listener, so only one live ArtiTorClient per process is
 * supported (last-writer-wins for logs; status/error callbacks stay
 * per-instance).
 */
class ArtiTorClient internal constructor(
    private val native: FfiArtiTorInterface,
    // Internal scheduling seam for deterministic publication-race tests.
    internal val beforeSessionStatusPublication: ((String, ULong) -> Unit)? = null,
) {
    constructor() : this(FfiArtiTor())

    private val lifecycleMutex = Mutex()

    private val _status = MutableStateFlow(TorStatus())
    val status: StateFlow<TorStatus> = _status.asStateFlow()

    private val _logs = MutableSharedFlow<String>(extraBufferCapacity = 256)
    val logs: SharedFlow<String> = _logs.asSharedFlow()

    /** Bumped by every pause/shutdown (and observed by waiters) for cancellation. */
    private val epoch = MutableStateFlow(0L)

    /** Last successful/effective config; guarded by [lifecycleMutex]. */
    private var lastConfig: ArtiConfig? = null

    val version: String get() = native.version()

    val isReady: Boolean get() = _status.value.isReady

    /** True if a bootstrapped TorClient is held (RUNNING or PAUSED). */
    val hasClient: Boolean get() = native.hasClient()

    /** Root SOCKS endpoint convenience, derived from status.
     * Null unless RUNNING. When non-null, host is "127.0.0.1" and port equals status.value.socksPort.
     */
    val socksEndpoint: TorSocksEndpoint?
        get() = _status.value.socksPort?.let { TorSocksEndpoint("127.0.0.1", it) }

    // Registration and pre-registration callbacks share one atomic record.
    // Historical IDs have no entry after creation finishes or a wrapper terminates.
    private val sessionRegistry = MutableStateFlow(SessionRegistry())
    internal val pendingSessionStatusCount: Int get() = sessionRegistry.value.pending.size

    /** Snapshot of live additional sessions, excluding the root/default endpoint. */
    val sessions: List<TorIsolationSession>
        get() = sessionRegistry.value.wrappers.values.filter { it.status.value.state.isLive }.toList()

    private val sessionListener = object : SessionStatusListener {
        override fun onSessionStatus(
            sessionId: String,
            state: SessionState,
            port: UShort?,
            revision: ULong,
        ) {
            val incoming = sessionStatus(state, port, revision)
            sessionRegistry.update { registry ->
                val wrapper = registry.wrappers[sessionId]
                if (wrapper != null) {
                    // CAS retries are safe: acceptance itself is monotonic and
                    // terminal-latched, and no native call occurs in this region.
                    wrapper.acceptStatus(incoming)
                    if (wrapper.status.value.state.isTerminal) {
                        registry.copy(wrappers = registry.wrappers - sessionId)
                    } else registry
                } else if (registry.creating) {
                    val previous = registry.pending[sessionId]
                    if (previous != null &&
                        (previous.status.state.isTerminal || incoming.revision <= previous.revision)
                    ) registry
                    else registry.copy(pending = registry.pending + (sessionId to incoming))
                } else registry
            }
        }
    }

    private val listener = object : StatusListener {
        override fun onStatus(
            state: FfiTorState,
            bootstrapPercent: UInt,
            socksPort: UShort?,
            summary: String,
        ) {
            val mapped = state.toCommon()
            val prev = _status.value
            _status.value = TorStatus(
                state = mapped,
                bootstrapPercent = bootstrapPercent.toInt(),
                socksPort = socksPort?.toInt(),
                bootstrapSummary = summary,
                lastError = if (mapped == TorState.ERROR) {
                    // A typed onError either already set this or arrives next
                    // (native guarantees onError precedes onStatus(Error)).
                    prev.lastError ?: ArtiException.Runtime(summary.ifEmpty { "error" })
                } else {
                    null
                },
            )
        }

        override fun onLog(line: String) {
            _logs.tryEmit(line)
        }

        override fun onError(error: FfiErrorDetail) {
            // Typed async failure: applied whether it lands just before or
            // just after the matching onStatus(Error, ..) — either order keeps
            // the declared type. Never parse summary strings for types.
            val typed = error.toPublic()
            val prev = _status.value
            _status.value = prev.copy(
                state = TorState.ERROR,
                socksPort = null,
                bootstrapSummary = error.msg,
                lastError = typed,
            )
        }
    }

    /**
     * Bootstrap (if needed) and ensure SOCKS is listening.
     * Suspends until [TorStatus.isReady] or failure.
     *
     * - Already ready with the same effective config → Ok immediately.
     * - Already ready with a different port only → rebind SOCKS (no bootstrap).
     * - Already ready with a different client-defining config → full restart
     *   with the new config.
     * - PAUSED → start with the supplied config: rebind when only the port
     *   changed, otherwise tear down and bootstrap a new client.
     * - OFF/ERROR → cold start. STARTING/BOOTSTRAPPING → join in-flight work.
     *
     * A [pause]/[shutdown] from another coroutine cancels the wait with
     * [ArtiException.Runtime] ("cancelled"), never a full-timeout hang.
     */
    suspend fun start(
        config: ArtiConfig,
        timeout: Duration = 90.seconds,
    ): Result<Unit> = lifecycleMutex.withLock {
        runCatching {
            // Freeze caller-owned collections before comparing identity or
            // validating; retain this same snapshot after a successful start.
            val requested = config.copy(bridges = config.bridges.toList())
            val cur = _status.value
            if (cur.isReady) {
                val effective = lastConfig
                if (effective != null &&
                    !torClientConfigChanged(effective, requested) &&
                    effective.socksPort == requested.socksPort
                ) {
                    return@runCatching
                }
                preflight(requested)
                if (effective != null && !torClientConfigChanged(effective, requested)) {
                    // Port-only change while RUNNING: drop SOCKS, rebind.
                    doPauseLocked()
                    kickStart(requested)
                } else {
                    // Client-defining change (or unknown baseline): cold start.
                    doShutdownLocked()
                    kickStart(requested)
                }
            } else {
                // OFF/ERROR/PAUSED/STARTING/BOOTSTRAPPING/STOPPING: the native
                // layer applies config identity (rebind vs rebuild) for the
                // PAUSED case; cold start otherwise; AlreadyRunning joins
                // in-flight work.
                kickStart(requested)
            }
            awaitReady(epoch.value, timeout)
            lastConfig = requested
        }
    }

    /**
     * Stop SOCKS; keep the bootstrapped client (status → PAUSED).
     * During STARTING/BOOTSTRAPPING: cancel the bootstrap, discard any partial
     * client, status → OFF. Terminates live SOCKS streams (fail-closed).
     * Safe to call redundantly.
     */
    fun pause() {
        doPauseLocked()
    }

    /**
     * Re-bind SOCKS using the last configuration. Requires [hasClient].
     */
    suspend fun resume(timeout: Duration = 30.seconds): Result<Unit> = lifecycleMutex.withLock {
        runCatching {
            if (_status.value.isReady) return@runCatching
            try {
                native.resume(listener)
            } catch (e: FfiArtiException) {
                throw e.toPublic()
            }
            awaitReady(epoch.value, timeout)
        }
    }

    /**
     * Full teardown: SOCKS, TorClient, tokio runtime. Status → OFF.
     * Safe to call redundantly.
     */
    fun shutdown() {
        doShutdownLocked()
    }

    /**
     * @deprecated Use [pause] (soft) or [shutdown] (hard).
     */
    @Deprecated("Use pause() or shutdown()", ReplaceWith("shutdown()"))
    fun stop() = shutdown()

    /**
     * [shutdown] then [start] with the supplied config, or — when null — the
     * last successful/effective config. Fails with [ArtiException.Config] when
     * no config is available (e.g. `restart()` before any successful start).
     */
    suspend fun restart(
        config: ArtiConfig? = null,
        timeout: Duration = 90.seconds,
    ): Result<Unit> = lifecycleMutex.withLock {
        runCatching {
            val effective = (config ?: lastConfig)
                ?: throw ArtiException.Config(
                    "restart() without a config requires a previous successful start",
                )
            val requested = effective.copy(bridges = effective.bridges.toList())
            preflight(requested)
            doShutdownLocked()
            kickStart(requested)
            // Epoch captured after our own shutdown bump, so only *foreign*
            // pause/shutdown cancels this wait.
            awaitReady(epoch.value, timeout)
            lastConfig = requested
        }
    }

    /**
     * Create an additional circuit-isolated SOCKS session.
     *
     * - While RUNNING: derives isolated client, binds ephemeral loopback listener,
     *   publishes ACTIVE with endpoint, returns session.
     * - While PAUSED: derives isolated client, registers as PAUSED (no endpoint),
     *   binds on next successful engine resume().
     * - While STARTING/BOOTSTRAPPING/STOPPING/ERROR/OFF: fails with [ArtiException.NotRunning].
     * - Over the internal safety cap: fails with [ArtiException.Runtime] ("session limit reached").
     *
     * The returned session's [status] StateFlow emits atomic (state, endpoint) pairs.
     * Terminal states (CLOSED, INVALIDATED) are latched forever on the Kotlin side.
     */
    suspend fun createIsolationSession(): Result<TorIsolationSession> = lifecycleMutex.withLock {
        sessionRegistry.update { it.copy(creating = true, pending = emptyMap()) }
        try {
            runCatching {
                val nativeSession = try {
                    native.createSession(sessionListener)
                } catch (e: FfiArtiException) {
                    throw e.toPublic()
                }
                val sessionId = nativeSession.id()
                val snapshot = native.sessionStatus(nativeSession)
                val snapshotStatus = sessionStatus(snapshot.state, snapshot.port, snapshot.revision)
                val wrapper = TorIsolationSessionImpl(
                    nativeSession, this, sessionRegistry.value.pending[sessionId] ?: snapshotStatus,
                )
                wrapper.acceptStatus(snapshotStatus)
                sessionRegistry.update { registry ->
                    registry.pending[sessionId]?.let { wrapper.acceptStatus(it) }
                    registry.copy(
                        wrappers = if (wrapper.status.value.state.isLive) {
                            registry.wrappers + (sessionId to wrapper)
                        } else registry.wrappers,
                        pending = emptyMap(),
                        creating = false,
                    )
                }
                wrapper
            }
        } finally {
            // Includes native failures and orphan callbacks from the creation
            // window. No pending revision/status is historical bookkeeping.
            sessionRegistry.update { it.copy(creating = false, pending = emptyMap()) }
        }
    }

    /**
     * Close one session (== [TorIsolationSession.close]).
     * No-op on unknown/closed/stale handles; never throws.
     */
    fun closeSession(session: TorIsolationSession) {
        if (session is TorIsolationSessionImpl) {
            native.closeSession(session.nativeSession)
            sessionRegistry.update { it.copy(wrappers = it.wrappers - session.id) }
        }
    }

    // -- internals (lifecycleMutex held unless noted) -------------------------

    /** Synchronous kick; AlreadyRunning joins in-flight work in [awaitReady]. */
    private fun kickStart(config: ArtiConfig) {
        try {
            native.start(config.toFfi(), listener)
        } catch (e: FfiArtiException) {
            when (e) {
                is FfiArtiException.AlreadyRunning -> Unit // join in-flight
                is FfiArtiException.NotRunning -> try {
                    native.start(config.toFfi(), listener)
                } catch (e2: FfiArtiException) {
                    if (e2 is FfiArtiException.AlreadyRunning) Unit else throw e2.toPublic()
                }
                else -> throw e.toPublic()
            }
        }
    }

    private fun preflight(config: ArtiConfig) {
        try {
            validateConfig(config.toFfi())
        } catch (e: FfiArtiException) {
            throw e.toPublic()
        }
    }

    private fun doPauseLocked() {
        native.pause()
        epoch.update { it + 1 }
        // The native layer reports PAUSED/OFF synchronously in the common
        // paths; these fallbacks only cover FFI callback races so no caller
        // observes a stranded STARTING/BOOTSTRAPPING/RUNNING-with-port state.
        val s = _status.value
        when (s.state) {
            TorState.STARTING, TorState.BOOTSTRAPPING ->
                if (!native.hasClient()) _status.value = TorStatus(state = TorState.OFF)
            TorState.RUNNING ->
                if (s.socksPort != null) {
                    _status.value = s.copy(state = TorState.PAUSED, socksPort = null)
                }
            else -> Unit
        }
    }

    private fun doShutdownLocked() {
        native.shutdown()
        epoch.update { it + 1 }
        _status.value = TorStatus()
    }

    /**
     * Wait for readiness. Wakes on READY, typed ERROR, or a *foreign* epoch
     * bump (pause/shutdown from another coroutine) — never sits out the full
     * timeout after definitive cancellation. Stale terminal states observed at
     * entry do not match: only a new READY/ERROR or a newer epoch does.
     */
    private suspend fun awaitReady(myEpoch: Long, timeout: Duration) {
        try {
            withTimeout(timeout) {
                val (end, endEpoch) = combine(status, epoch) { s, e -> s to e }
                    .first { (s, e) -> s.isReady || s.state == TorState.ERROR || e != myEpoch }
                if (!end.isReady) {
                    if (end.state == TorState.ERROR) {
                        throw end.lastError
                            ?: ArtiException.Runtime("Tor failed (state=${end.state})")
                    } else {
                        throw ArtiException.Runtime(
                            "Tor start cancelled by lifecycle operation " +
                                "(state=${end.state}, epoch $myEpoch→$endEpoch)",
                        )
                    }
                }
            }
        } catch (e: TimeoutCancellationException) {
            throw ArtiException.Timeout()
        }
    }
}

private fun ArtiConfig.toFfi(): FfiConfig {
    // The FFI u16 cannot represent these values; reject rather than wrap.
    if (socksPort !in 0..65535) throw ArtiException.Config("SOCKS port must be between 0 and 65535")
    return FfiConfig(
        dataDir = dataDir,
        socksPort = socksPort.toUShort(),
        bridges = bridges,
        bridgesEnabled = when (bridgesEnabled) {
            BridgesEnabled.AUTO -> FfiBridgesEnabled.AUTO
            BridgesEnabled.ON -> FfiBridgesEnabled.ON
            BridgesEnabled.OFF -> FfiBridgesEnabled.OFF
        },
        stateDir = stateDir,
        cacheDir = cacheDir,
        allowOnionAddrs = allowOnionAddrs,
        connectTimeoutNanos = connectTimeout.toFfiNanoseconds(),
        resolveTimeoutNanos = resolveTimeout.toFfiNanoseconds(),
    )
}

/** Reject values the signed nanosecond FFI cannot represent exactly; Rust validates semantics. */
private fun Duration.toFfiNanoseconds(): Long {
    if (!isFinite()) throw ArtiException.Config("timeout must be finite")
    val nanos = inWholeNanoseconds
    if (nanos.nanoseconds != this) throw ArtiException.Config("timeout must fit signed nanoseconds exactly")
    return nanos
}

internal fun FfiArtiException.toPublic(): ArtiException = when (this) {
    is FfiArtiException.AlreadyRunning -> ArtiException.AlreadyRunning()
    is FfiArtiException.NotRunning -> ArtiException.NotRunning()
    is FfiArtiException.Config -> ArtiException.Config(msg)
    is FfiArtiException.Bind -> ArtiException.Bind(port.toInt(), msg)
    is FfiArtiException.Bootstrap -> ArtiException.Bootstrap(msg, errorKind.toPublic())
    is FfiArtiException.Runtime -> ArtiException.Runtime(msg, errorKind.toPublic())
}

/** Typed async failure (no string parsing: [FfiErrorDetail.kind] drives the type). */
internal fun FfiErrorDetail.toPublic(): ArtiException = when (kind) {
    FfiErrorKind.CONFIG -> ArtiException.Config(msg)
    FfiErrorKind.BIND -> ArtiException.Bind((port ?: 0u).toInt(), msg)
    FfiErrorKind.BOOTSTRAP -> ArtiException.Bootstrap(msg, errorKind.toPublic())
    FfiErrorKind.RUNTIME -> ArtiException.Runtime(msg, errorKind.toPublic())
    FfiErrorKind.ALREADY_RUNNING -> ArtiException.AlreadyRunning()
    FfiErrorKind.NOT_RUNNING -> ArtiException.NotRunning()
}

private fun FfiTorState.toCommon(): TorState = when (this) {
    FfiTorState.OFF -> TorState.OFF
    FfiTorState.STARTING -> TorState.STARTING
    FfiTorState.BOOTSTRAPPING -> TorState.BOOTSTRAPPING
    FfiTorState.RUNNING -> TorState.RUNNING
    FfiTorState.PAUSED -> TorState.PAUSED
    FfiTorState.STOPPING -> TorState.STOPPING
    FfiTorState.ERROR -> TorState.ERROR
}

internal fun FfiTorErrorKind.toPublic(): TorErrorKind = when (this) {
    FfiTorErrorKind.ALREADY_RUNNING -> TorErrorKind.ALREADY_RUNNING
    FfiTorErrorKind.NOT_RUNNING -> TorErrorKind.NOT_RUNNING
    FfiTorErrorKind.CONFIG -> TorErrorKind.CONFIG
    FfiTorErrorKind.BIND -> TorErrorKind.BIND
    FfiTorErrorKind.BOOTSTRAP -> TorErrorKind.BOOTSTRAP
    FfiTorErrorKind.TIMEOUT -> TorErrorKind.TIMEOUT
    FfiTorErrorKind.RUNTIME -> TorErrorKind.RUNTIME
    FfiTorErrorKind.NETWORK -> TorErrorKind.NETWORK
    FfiTorErrorKind.EXIT_FAILED -> TorErrorKind.EXIT_FAILED
    FfiTorErrorKind.TARGET_REJECTED -> TorErrorKind.TARGET_REJECTED
    FfiTorErrorKind.STORAGE -> TorErrorKind.STORAGE
    FfiTorErrorKind.BOOTSTRAP_REQUIRED -> TorErrorKind.BOOTSTRAP_REQUIRED
    FfiTorErrorKind.SESSION_CLOSED -> TorErrorKind.SESSION_CLOSED
    FfiTorErrorKind.SESSION_INVALIDATED -> TorErrorKind.SESSION_INVALIDATED
    FfiTorErrorKind.UNKNOWN -> TorErrorKind.UNKNOWN
}
