package com.yet.tor

import com.yet.tor.ffi.ArtiConfig as FfiConfig
import com.yet.tor.ffi.ArtiTor as FfiArtiTor
import com.yet.tor.ffi.StatusListener
import com.yet.tor.ffi.TorState as FfiTorState
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.first

/** High-level lifecycle state. */
enum class TorState { OFF, STARTING, BOOTSTRAPPING, RUNNING, STOPPING, ERROR }

/**
 * First-class status. [bootstrapPercent] comes straight from Arti's
 * `bootstrap_events()` — not from log scraping. [socksPort] is non-null only
 * once the local SOCKS listener is actually bound.
 */
data class TorStatus(
    val state: TorState = TorState.OFF,
    val bootstrapPercent: Int = 0,
    val socksPort: Int? = null,
)

/** Caller-supplied configuration. [dataDir] is provided by the caller. */
data class ArtiConfig(
    val dataDir: String,
    val socksPort: Int = 9050,
    val bridges: List<String> = emptyList(),
)

/**
 * Cross-platform facade over the Arti FFI. The async tokio runtime lives inside
 * the native layer; this object never blocks the caller's thread.
 *
 * "Proxy ready" == [TorStatus] with `state == RUNNING && bootstrapPercent == 100
 * && socksPort != null`.
 */
class ArtiTorClient {
    private val native = FfiArtiTor()

    private val _status = MutableStateFlow(TorStatus())
    val status: StateFlow<TorStatus> = _status.asStateFlow()

    private val _logs = MutableSharedFlow<String>(extraBufferCapacity = 256)
    val logs: SharedFlow<String> = _logs.asSharedFlow()

    val version: String get() = native.version()

    private val listener = object : StatusListener {
        override fun onStatus(state: FfiTorState, bootstrapPercent: UInt, socksPort: UShort?) {
            _status.value = TorStatus(state.toCommon(), bootstrapPercent.toInt(), socksPort?.toInt())
        }

        override fun onLog(line: String) {
            _logs.tryEmit(line)
        }
    }

    /** Starts bootstrapping and the SOCKS proxy; suspends until ready or failed. */
    suspend fun start(config: ArtiConfig): Result<Unit> = runCatching {
        native.start(FfiConfig(config.dataDir, config.socksPort.toUShort(), config.bridges), listener)
        val ready = status.first { it.state == TorState.RUNNING || it.state == TorState.ERROR }
        if (ready.state != TorState.RUNNING) {
            throw IllegalStateException("Tor failed to reach RUNNING (state=${ready.state})")
        }
    }

    fun stop() {
        native.stop()
        _status.value = TorStatus()
    }
}

private fun FfiTorState.toCommon(): TorState = when (this) {
    FfiTorState.OFF -> TorState.OFF
    FfiTorState.STARTING -> TorState.STARTING
    FfiTorState.BOOTSTRAPPING -> TorState.BOOTSTRAPPING
    FfiTorState.RUNNING -> TorState.RUNNING
    FfiTorState.STOPPING -> TorState.STOPPING
    FfiTorState.ERROR -> TorState.ERROR
}
