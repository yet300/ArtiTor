package com.yet.tor

import com.yet.tor.ffi.ArtiConfig as FfiConfig
import com.yet.tor.ffi.ArtiException as FfiArtiException
import com.yet.tor.ffi.ArtiTor as FfiArtiTor
import com.yet.tor.ffi.StatusListener
import com.yet.tor.ffi.TorState as FfiTorState
import kotlin.time.Duration
import kotlin.time.Duration.Companion.seconds
import kotlinx.coroutines.TimeoutCancellationException
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withTimeout

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

/**
 * Caller-supplied configuration. [dataDir] is required; the library never
 * invents platform paths.
 *
 * @param socksPort Local SOCKS bind. `0` = ephemeral (OS picks a free port);
 *   the actual port is reported in [TorStatus.socksPort].
 * @param bridges Bridge lines (one per entry). Empty = default guards.
 * @param stateDir Override for `$dataDir/state`.
 * @param cacheDir Override for `$dataDir/cache`.
 */
data class ArtiConfig(
    val dataDir: String,
    val socksPort: Int = 0,
    val bridges: List<String> = emptyList(),
    val stateDir: String? = null,
    val cacheDir: String? = null,
)

/** Typed failures from the engine (maps UniFFI [FfiArtiError] + Kotlin waits). */
sealed class ArtiException(message: String, cause: Throwable? = null) :
    Exception(message, cause) {
    class AlreadyRunning : ArtiException("Tor client already starting or running")
    class NotRunning : ArtiException("Tor client is not running")
    class Config(msg: String) : ArtiException("configuration error: $msg")
    class Bind(val port: Int, msg: String) :
        ArtiException("failed to bind SOCKS on $port: $msg")
    class Bootstrap(msg: String) : ArtiException("bootstrap failed: $msg")
    class Timeout(msg: String = "timed out waiting for Tor ready") : ArtiException(msg)
    class Runtime(msg: String) : ArtiException("runtime error: $msg")
}

/**
 * Cross-platform facade over the Arti FFI. The async tokio runtime lives inside
 * the native layer; this object never blocks the caller's thread.
 *
 * Lifecycle (see docs/api-lifecycle-bitchat.md):
 * - [start] — bootstrap if needed + SOCKS; suspends until ready
 * - [pause] — drop SOCKS, keep bootstrapped client
 * - [resume] — rebind SOCKS without full bootstrap
 * - [shutdown] — full teardown
 *
 * "Proxy ready" == [TorStatus.isReady].
 */
class ArtiTorClient {
    private val native = FfiArtiTor()
    private val lifecycleMutex = Mutex()

    private val _status = MutableStateFlow(TorStatus())
    val status: StateFlow<TorStatus> = _status.asStateFlow()

    private val _logs = MutableSharedFlow<String>(extraBufferCapacity = 256)
    val logs: SharedFlow<String> = _logs.asSharedFlow()

    val version: String get() = native.version()

    val isReady: Boolean get() = _status.value.isReady

    /** True if a bootstrapped TorClient is held (RUNNING or PAUSED). */
    val hasClient: Boolean get() = native.hasClient()

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
                    prev.lastError ?: ArtiException.Runtime(summary.ifEmpty { "error" })
                } else {
                    null
                },
            )
        }

        override fun onLog(line: String) {
            _logs.tryEmit(line)
        }
    }

    /**
     * Bootstrap (if needed) and ensure SOCKS is listening.
     * Suspends until [TorStatus.isReady] or failure.
     *
     * - Already ready → Ok immediately
     * - PAUSED → [resume]
     * - OFF/ERROR → cold start
     */
    suspend fun start(
        config: ArtiConfig,
        timeout: Duration = 90.seconds,
    ): Result<Unit> = lifecycleMutex.withLock {
        runCatching {
            if (_status.value.isReady) return@runCatching

            when {
                _status.value.state == TorState.PAUSED ||
                    (native.hasClient() && !_status.value.isReady) -> {
                    try {
                        native.resume(listener)
                    } catch (e: FfiArtiException) {
                        // Fall through to cold start if client was lost.
                        if (e is FfiArtiException.NotRunning) {
                            native.start(config.toFfi(), listener)
                        } else {
                            throw e.toPublic()
                        }
                    }
                }
                else -> {
                    try {
                        native.start(config.toFfi(), listener)
                    } catch (e: FfiArtiException) {
                        if (e is FfiArtiException.AlreadyRunning) {
                            // In-flight start: just wait for ready below.
                        } else {
                            throw e.toPublic()
                        }
                    }
                }
            }

            awaitReady(timeout)
        }
    }

    /**
     * Stop SOCKS; keep bootstrapped client. Status → PAUSED when a client is held.
     */
    fun pause() {
        native.pause()
        // Native reports PAUSED; if still OFF/bootstrapping, leave status as-is
        // until the next onStatus. Ensure socks cleared if native already paused.
        val s = _status.value
        if (s.state == TorState.RUNNING || native.hasClient()) {
            if (s.state != TorState.PAUSED) {
                // Defensive: native should have emitted PAUSED.
                if (_status.value.socksPort != null && _status.value.state == TorState.RUNNING) {
                    _status.value = s.copy(state = TorState.PAUSED, socksPort = null, bootstrapSummary = "paused")
                }
            }
        }
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
            awaitReady(timeout)
        }
    }

    /**
     * Full teardown: SOCKS, TorClient, tokio runtime. Status → OFF.
     */
    fun shutdown() {
        native.shutdown()
        _status.value = TorStatus()
    }

    /**
     * @deprecated Use [pause] (soft) or [shutdown] (hard).
     */
    @Deprecated("Use pause() or shutdown()", ReplaceWith("shutdown()"))
    fun stop() = shutdown()

    /**
     * [shutdown] then [start] with [config] (or last paths if null is not possible —
     * config is required after shutdown).
     */
    suspend fun restart(
        config: ArtiConfig,
        timeout: Duration = 90.seconds,
    ): Result<Unit> {
        shutdown()
        return start(config, timeout)
    }

    private suspend fun awaitReady(timeout: Duration) {
        try {
            withTimeout(timeout) {
                val ready = status.first { it.isReady || it.state == TorState.ERROR }
                if (!ready.isReady) {
                    throw ready.lastError
                        ?: ArtiException.Runtime("Tor failed (state=${ready.state})")
                }
            }
        } catch (e: TimeoutCancellationException) {
            throw ArtiException.Timeout()
        }
    }
}

private fun ArtiConfig.toFfi(): FfiConfig = FfiConfig(
    dataDir = dataDir,
    socksPort = socksPort.toUShort(),
    bridges = bridges,
    stateDir = stateDir,
    cacheDir = cacheDir,
)

private fun FfiArtiException.toPublic(): ArtiException = when (this) {
    is FfiArtiException.AlreadyRunning -> ArtiException.AlreadyRunning()
    is FfiArtiException.NotRunning -> ArtiException.NotRunning()
    is FfiArtiException.Config -> ArtiException.Config(msg)
    is FfiArtiException.Bind -> ArtiException.Bind(port.toInt(), msg)
    is FfiArtiException.Bootstrap -> ArtiException.Bootstrap(msg)
    is FfiArtiException.Runtime -> ArtiException.Runtime(msg)
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
