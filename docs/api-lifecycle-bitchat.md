# Target API: ArtiTor for BitChat KMP

Status: Design (2026-07-19)  
Audience: ArtiTor maintainers + BitChat KMP port  
Goal: one KMP engine that replaces **both** custom stacks:

| Today | Path |
|-------|------|
| Android | `tools/arti-build` + `ArtiNative` + `ArtiProxy` + `ArtiTorManager` |
| iOS | `localPackages/Arti` (`arti-bitchat` + `TorManager` + `TorURLSession`) |

This document freezes the **library surface**. App policy (ON by default, fail-closed, OkHttp/Ktor wiring, Nostr reconnect) stays in BitChat.

---

## 1. Design principles

1. **Thin engine, fat app** — lib owns TorClient + SOCKS + status. App owns prefs, HTTP clients, UI, foreground policy.
2. **Bootstrap ≠ SOCKS** — stop SOCKS without tearing down a bootstrapped client (Android toggle fix).
3. **No log scraping** — readiness is typed `TorStatus`, never AMEx / “guard usable” strings.
4. **No platform paths** — caller always supplies `dataDir` (and optional cache/state overrides later).
5. **Same model as both apps today** — `arti-client` + minimal SOCKS5 CONNECT; not full `arti proxy`, not DNS, not WebView, not PT binaries (P2 later).

---

## 2. Lifecycle state machine

```
                    start() / ensureClient()
   OFF ──────────────────────────────────────► STARTING
    ▲                                              │
    │                                              │ bootstrap_events
    │                                              ▼
    │                                        BOOTSTRAPPING ──%──► (still BOOTSTRAPPING)
    │                                              │
    │                                              │ frac→1.0 + SOCKS bound
    │                                              ▼
    │   stopSocks() / pause()              RUNNING ◄── resumeSocks() (client kept)
    │   (client kept, % stays 100)              │
    │◄──────────────────────────────────────────┘
    │
    │   shutdown()  (drop client + runtime)
    └──────────────────────────────────────────

   any phase ──error──► ERROR
   ERROR ──shutdown() or start() after clear──► OFF / STARTING
```

### States (public)

| `TorState` | Meaning | SOCKS accepting? | Client in memory? |
|------------|---------|------------------|-------------------|
| `OFF` | Idle | no | no |
| `STARTING` | Runtime up, client creating | no | maybe |
| `BOOTSTRAPPING` | Client alive, bootstrap in progress | no* | yes |
| `RUNNING` | Bootstrap done **and** SOCKS bound | yes | yes |
| `PAUSED` | Bootstrap done, SOCKS down (toggle / background soft-stop) | no | yes |
| `STOPPING` | Teardown in progress | no | maybe |
| `ERROR` | Last op failed; see `lastError` | no | undefined — call `shutdown()` |

\* Optional future: bind SOCKS early with OnDemand bootstrap; BitChat today waits for ready first. **v1: SOCKS only after bootstrap ≥ 100.**

### Pause during STARTING/BOOTSTRAPPING (decided 0.2.0 audit)

The diagram above shows `pause()` from RUNNING. For `pause()` while no
bootstrapped client is held yet (STARTING/BOOTSTRAPPING) the behavior is
deterministic:

- cancel the bootstrap worker,
- discard any partial/unbootstrapped client,
- stop associated work (SOCKS shutdown signal + tracked connection tasks),
- transition to **OFF** (bootstrap % reset, no SOCKS port).

The client is never left observing STARTING/BOOTSTRAPPING after its worker was
aborted. Rationale: PAUSED is defined as "retained, already-bootstrapped
client"; a half-bootstrapped client is not retainable, so OFF is the only
honest state. Any suspended `start()` waiter fails fast with
`ArtiException.Runtime("...cancelled...")` instead of waiting out its timeout.

### Cancellation (decided 0.2.0 audit)

`pause()`/`shutdown()` are non-suspending and never block on the lifecycle
mutex. They bump a facade epoch; any `start()`/`resume()` waiter from before
the bump wakes immediately:

- `ERROR` → throw the typed `lastError`,
- foreign epoch bump (pause/shutdown) → throw `ArtiException.Runtime` (cancelled),
- stale OFF/PAUSED/STOPPING snapshots observed at entry never match — only a
  *new* READY/ERROR or a newer epoch wakes the waiter.

No suspend caller remains waiting until its timeout after another lifecycle
operation has definitively cancelled its operation.

### Readiness (shared definition — replaces both apps)

```text
isReady ⇔ state == RUNNING
       && bootstrapPercent >= 100
       && socksPort != null
```

- Android `ArtiTorManager.isProxyEnabled()` → `client.isReady`
- iOS `TorManager.isReady` (`socksReady && progress >= 100`) → `client.isReady`

---

## 3. Public Kotlin API (target)

Package: `com.yet.tor`  
Module: `commonMain` only (no Android Context, no iOS paths).

```kotlin
package com.yet.tor

import kotlin.time.Duration
import kotlin.time.Duration.Companion.seconds
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.StateFlow

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/**
 * Caller-owned paths and listen options.
 *
 * @param dataDir Root; lib uses `{dataDir}/state` and `{dataDir}/cache`
 *                unless [stateDir]/[cacheDir] override.
 * @param socksPort Local SOCKS5 bind. `0` = ephemeral (OS picks free port);
 *                  actual port appears in [TorStatus.socksPort].
 * @param bridges Bridge lines (one per entry). Empty = default guards only.
 *                Pluggable-transport endpoints are out of scope for v1.
 */
data class ArtiConfig(
    val dataDir: String,
    val socksPort: Int = 0,                 // 0 = auto (recommended for apps)
    val bridges: List<String> = emptyList(),
    val stateDir: String? = null,           // default: "$dataDir/state"
    val cacheDir: String? = null,           // default: "$dataDir/cache"
)

// ---------------------------------------------------------------------------
// Status / errors
// ---------------------------------------------------------------------------

enum class TorState {
    OFF,
    STARTING,
    BOOTSTRAPPING,
    RUNNING,
    PAUSED,
    STOPPING,
    ERROR,
}

/**
 * @param bootstrapPercent 0..100 from Arti bootstrap_events (not logs).
 * @param socksPort Bound port when accepting; null if not listening.
 * @param bootstrapSummary Short human string for UI (optional; may be "").
 * @param lastError Last failure if [state] == ERROR; cleared on successful start.
 */
data class TorStatus(
    val state: TorState = TorState.OFF,
    val bootstrapPercent: Int = 0,
    val socksPort: Int? = null,
    val bootstrapSummary: String = "",
    val lastError: ArtiException? = null,
) {
    val isReady: Boolean
        get() = state == TorState.RUNNING &&
            bootstrapPercent >= 100 &&
            socksPort != null
}

sealed class ArtiException(message: String, cause: Throwable? = null) :
    Exception(message, cause) {

    class AlreadyRunning : ArtiException("Tor client already starting or running")
    class NotRunning : ArtiException("Tor client is not running")
    class Config(msg: String) : ArtiException("configuration error: $msg")
    class Bind(val port: Int, msg: String) :
        ArtiException("failed to bind SOCKS on $port: $msg")
    class Bootstrap(msg: String) : ArtiException("bootstrap failed: $msg")
    class Timeout(msg: String = "timed out waiting for Tor ready") :
        ArtiException(msg)
    class Runtime(msg: String) : ArtiException("runtime error: $msg")
}

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

/**
 * Cross-platform Tor engine.
 *
 * Threading: never blocks the caller; tokio lives in native code.
 * Concurrency: methods are safe to call from multiple coroutines; internal
 * mutex serializes lifecycle transitions (like ArtiTorManager.applyMutex).
 *
 * Typical BitChat flow:
 * ```
 * val tor = ArtiTorClient()
 * scope.launch { tor.status.collect { ui.update(it) } }
 *
 * // cold start → ready
 * tor.start(ArtiConfig(dataDir = dir, socksPort = 0)).getOrThrow()
 * val port = tor.status.value.socksPort!!
 * // wire OkHttp/Ktor/URLSession to 127.0.0.1:port
 *
 * // user toggles Tor OFF (keep bootstrap for fast re-enable)
 * tor.pause()
 *
 * // user toggles Tor ON again
 * tor.resume()   // or start() if was shutdown
 *
 * // process death / panic wipe
 * tor.shutdown()
 * ```
 */
class ArtiTorClient {

    val version: String
    val status: StateFlow<TorStatus>
    val logs: SharedFlow<String>

    /** True iff [TorStatus.isReady]. Convenience over status.value. */
    val isReady: Boolean

    /** True if a TorClient (possibly paused) is held in memory. */
    val hasClient: Boolean

    /**
     * Bootstrap (if needed) and ensure SOCKS is listening.
     * Suspends until [TorStatus.isReady] or failure.
     *
     * - If already READY with same effective config → Ok immediately.
     * - If PAUSED → equivalent to [resume] (re-bind SOCKS; no re-bootstrap).
     * - If OFF/ERROR → full start.
     * - If STARTING/BOOTSTRAPPING → wait on in-flight start (no second bootstrap).
     *
     * @throws ArtiException.Timeout if not ready within [timeout]
     * @throws ArtiException.Bind if fixed socksPort is taken (not used when port=0)
     * @throws ArtiException.Bootstrap / Config / Runtime on hard failure
     */
    suspend fun start(
        config: ArtiConfig,
        timeout: Duration = 90.seconds,
    ): Result<Unit>

    /**
     * Stop SOCKS listener; **keep** bootstrapped TorClient + runtime.
     * Status → PAUSED (bootstrapPercent stays 100, socksPort = null).
     *
     * Maps to:
     * - Android: `stop()` SOCKS while retaining client after `initialize`
     * - iOS soft path: mark not ready without `arti_stop` (better than today)
     *
     * No-op if already OFF/PAUSED/STOPPING.
     */
    fun pause()

    /**
     * Re-bind SOCKS using last [ArtiConfig] (or updated port via [start]).
     * Requires hasClient; otherwise returns failure → call [start].
     * Suspends until READY or error.
     */
    suspend fun resume(timeout: Duration = 30.seconds): Result<Unit>

    /**
     * Full teardown: abort SOCKS, drop TorClient, shutdown tokio runtime.
     * Status → OFF. Prefer for process exit / panic wipe / config path change.
     *
     * Maps to: Android hard stop + drop client; iOS `arti_stop` / `shutdownCompletely`.
     */
    fun shutdown()

    /**
     * Alias for [shutdown] for callers that used the old API.
     * @deprecated Use [pause] (soft) or [shutdown] (hard).
     */
    @Deprecated("Use pause() or shutdown()", ReplaceWith("shutdown()"))
    fun stop() = shutdown()

    /**
     * Convenience: [shutdown] then [start] with last or new config.
     * Maps to iOS `restartArti` / Android `restartArti`.
     *
     * The facade stores the last successful/effective config independently of
     * native shutdown state, so `restart()` (null) works after `shutdown()`.
     * With no prior successful start and no supplied config it fails with
     * `ArtiException.Config`.
     */
    suspend fun restart(
        config: ArtiConfig? = null,
        timeout: Duration = 90.seconds,
    ): Result<Unit>
}
```

### Configuration identity (decided 0.2.0 audit)

`start(config)` while a bootstrapped client is held (RUNNING reconfigure or
PAUSED) separates configuration into:

- **SOCKS-only** (`socksPort`): rebind SOCKS without rebuilding TorClient.
- **TorClient-defining** (`dataDir`, `stateDir`, `cacheDir`, `bridges` —
  compared exactly, so non-empty → empty counts): tear down the old client
  safely and bootstrap a new one. Stored config is replaced wholesale; bridges
  are never silently merged into a client built with different bridges.

`start()` while RUNNING applies the same rule (port-only → pause + rebind;
client-defining → shutdown + cold start).

### Defaults chosen for BitChat

| Knob | Default | Why |
|------|---------|-----|
| `socksPort = 0` | auto | Replaces Android bind-retry `port++` and avoids 9060/39050 conflicts |
| `start` timeout 90s | — | iOS awaitReady 75s + margin; Android inactivity 5s is app-level |
| `pause` keeps client | — | Android toggle performance |
| No DNS / no WebView / no PT in config | — | Neither app uses them in native path today |

---

## 4. Mapping: Android `ArtiTorManager` → ArtiTor + app

| Android today | Where it goes |
|---------------|---------------|
| `ArtiTorManager` singleton + `applyMutex` | **App** `TorService` / `TorController` (singleton OK) holding one `ArtiTorClient` |
| `TorMode.ON/OFF` + `TorPreferenceManager` | **App** prefs |
| `init(Application)` + data dir `filesDir/arti` | **App** builds `ArtiConfig(dataDir = File(filesDir,"arti").path)` |
| `ArtiProxy.Builder.setSocksPort/setDnsPort` | SOCKS → config; **DNS dropped** (never wired) |
| `ArtiLogListener` + log scrape (`Sufficiently bootstrapped`, `guard usable`) | **Delete** — use `status` / `logs` only for debug UI |
| `bootstrapPercent` fake 75/100 from logs | **Lib** real % |
| `startArti` / bind retry port++ | **Lib** `socksPort=0` or app catches `ArtiException.Bind` |
| `stopArti` soft (SOCKS) vs process death | **Lib** `pause()` vs `shutdown()` |
| `waitUntilBootstrapped` | **Lib** `start()` suspend |
| `isProxyEnabled()` | **Lib** `isReady` / `status.value.isReady` |
| `currentSocksAddress()` | **App** `InetSocketAddress("127.0.0.1", status.socksPort!!)` |
| `OkHttpProvider` proxy | **App** (KMP: expect/actual or multiplatform HTTP) |
| `resetNetworkConnections` / Nostr reconnect | **App** collect `status` → on ready rebuild clients |
| inactivity restart / scheduleRetry | **App** (optional; or rely on lib errors + user retry) |
| `tools/arti-build`, `libarti_android.so`, `ArtiNative` | **Delete** — Maven `com.yet.tor:tor` |
| `info.guardianproject.arti.*` shim | **Delete** |

### Thin Android app wrapper (sketch)

```kotlin
// app layer — NOT in ArtiTor
class TorController(private val app: Application) {
    private val client = ArtiTorClient()
    val status = client.status

    fun dataConfig(port: Int = 0) = ArtiConfig(
        dataDir = File(app.filesDir, "arti").absolutePath,
        socksPort = port,
        bridges = BridgePrefs.load(app), // empty until settings ship
    )

    suspend fun setEnabled(on: Boolean) {
        if (on) client.start(dataConfig()).getOrThrow()
        else client.pause()
    }

    fun socksProxyOrNull(): Proxy? {
        val p = client.status.value.socksPort ?: return null
        if (!client.isReady) return null
        return Proxy(Proxy.Type.SOCKS, InetSocketAddress("127.0.0.1", p))
    }
}
```

---

## 5. Mapping: iOS `TorManager` → ArtiTor + app

| iOS today | Where it goes |
|-----------|---------------|
| `TorManager.shared` | **App** shared `TorController` + one `ArtiTorClient` (KMP) |
| `socksPort = 39050` fixed | **Lib** auto port or app-chosen; session rebuilt from `status.socksPort` |
| `arti_start` / `arti_stop` | **Lib** `start` / `shutdown` |
| `arti_is_running` | **Lib** `hasClient` / state ≠ OFF |
| `arti_bootstrap_progress` / `summary` poll loop | **Lib** `status` Flow (no 1s poll) |
| `waitForSocksReady` TCP probe | **Unnecessary** if lib only sets `socksPort` after bind |
| `isReady` / `awaitReady(timeout:)` | **Lib** `start` / `status.first { isReady }` |
| `torEnforced` / `networkPermitted` | **App** |
| `TorURLSession` SOCKS dict | **App** rebuild session when `socksPort` changes |
| `goDormantOnBackground` (stub) | **App** `pause()` or leave RUNNING (policy choice) |
| `ensureRunningOnForeground` / `restartArti` | **App** `start` or `restart` |
| `shutdownCompletely` | **Lib** `shutdown()` |
| `pathMonitor` poke | **App** |
| `localPackages/Arti` xcframework + C shim | **Delete** — KMP framework / SPM from Ubique UniFFI artifacts |
| `arti_go_dormant` / `arti_wake` stubs | **Not in lib v1** |

---

## 6. Rust / UniFFI surface (target)

Keep one object; split lifecycle explicitly.

```rust
// conceptual — names match Kotlin facade

enum TorState { Off, Starting, Bootstrapping, Running, Paused, Stopping, Error }

struct ArtiConfig {
    data_dir: String,
    socks_port: u16,          // 0 = ephemeral
    bridges: Vec<String>,
    state_dir: Option<String>,
    cache_dir: Option<String>,
}

enum ArtiError {
    AlreadyRunning,
    NotRunning,
    Config { msg: String },
    Bind { port: u16, msg: String },
    Bootstrap { msg: String },
    Runtime { msg: String },
}

trait StatusListener {
    fn on_status(&self, state: TorState, bootstrap_percent: u32,
                 socks_port: Option<u16>, summary: String);
    fn on_log(&self, line: String);
    /// Typed async failure; always precedes the matching Error status.
    /// Carries (kind, port?, msg) so Kotlin reconstructs the declared
    /// ArtiException variant without parsing strings.
    fn on_error(&self, error: ArtiErrorDetail);
}

impl ArtiTor {
    fn new() -> Arc<Self>;
    fn version(&self) -> String;
    fn is_ready(&self) -> bool;
    fn has_client(&self) -> bool;
    fn socks_port(&self) -> Option<u16>;

    /// Non-blocking: kicks work; Kotlin suspend wait is on status Flow.
    /// Or provide blocking wait only on Kotlin side (preferred — already done).
    fn start(&self, config: ArtiConfig, listener: Box<dyn StatusListener>)
        -> Result<(), ArtiError>;

    fn pause(&self);      // stop SOCKS, keep client
    fn resume(&self, listener: Box<dyn StatusListener>) -> Result<(), ArtiError>;
    fn shutdown(&self);   // full teardown
}
```

### Internal `Inner` (implementation notes)

```text
runtime: Option<Runtime>          // lives until shutdown
client:  Option<Arc<TorClient>>   // lives until shutdown; kept across pause
socks_task: Option<JoinHandle>    // only while RUNNING
connections: Vec<JoinHandle>      // per-accepted-connection handlers; aborted on pause/shutdown (fail-closed: Tor OFF terminates live streams, it does not drain them)
listener: Option<Arc<dyn StatusListener>>
last_config: Option<ArtiConfig>   // replaced wholesale on every start
bootstrap_done: bool
```

Single-instance: `LOG_SINK` (tracing → on_log) is process-global and routes to
the most recently installed listener — one live `ArtiTor` per process is
supported (last-writer-wins for logs; status/error callbacks stay
per-instance). `shutdown()` releases the sink only if it still points at its
own listener.

| Call | runtime | client | socks_task |
|------|---------|--------|------------|
| start (cold) | create | bootstrap | spawn after 100% |
| pause | keep | keep | abort |
| resume | keep | keep | spawn bind |
| shutdown | drop | drop | abort |

### SOCKS

- Keep CONNECT-only handler (same as both BitChat trees and current ArtiTor).
- Document: HTTP(S)/WebSocket via SOCKS OK; no UDP/DNS.

### Bootstrap

- Always `create_unbootstrapped` + `bootstrap_events` + `bootstrap().await` (current approach).
- Do **not** reintroduce Android’s synthetic log lines.

---

## 7. What is explicitly out of scope (v1)

| Feature | Reason |
|---------|--------|
| DNS proxy | Android Builder field unused; BitChat uses SOCKS only |
| WebView proxy helper | Not used |
| obfs4/snowflake PT ports | Neither app wires PT; bridges list only |
| Managed PT binaries | App size / updates |
| Real Arti dormant mode | iOS stubs; app can `pause` + `resume` |
| Control port / RPC | Not used |
| Onion service **hosting** | Client feature only |
| Foreground service / VPN | App / OS |

P2 (when BitChat settings need censorship circumvention):

```kotlin
// future ArtiConfig fields — do not implement in v1
// val obfs4Proxy: String? = null,      // "127.0.0.1:47300"
// val snowflakeProxy: String? = null,
```

---

## 8. Migration from current ArtiTor `0.1.x`

| Current | Target |
|---------|--------|
| `start(config)` → full start, suspend to ready | same; add `timeout`, `socksPort=0` |
| `stop()` full teardown | split → `pause()` / `shutdown()`; `stop()` = deprecated → `shutdown()` |
| `TorState` without PAUSED | add `PAUSED` |
| `ArtiError` as UniFFI enum only | map to sealed `ArtiException` on Kotlin side |
| single-shot lifecycle | reusable client |

**Semver:** this is a **breaking** API → `0.2.0`.

---

## 9. Implementation phases

### Phase A — API + lifecycle (blocks BitChat KMP)

1. Rust: `pause` / `resume` / `shutdown`; keep client; `socks_port=0`; `Bind` error.
2. Kotlin: match surface above; `start(timeout)`; `TorStatus.isReady`.
3. Tests: Android instrumented + iOS sim — start → ready → pause → resume (no second multi-minute bootstrap) → shutdown.
4. Docs/README update.

### Phase B — BitChat integration helpers (optional, not in core)

- Sample `TorController` in `docs/samples/` or `:samples:bitchat-style`.
- Note on Ktor/OkHttp SOCKS (`127.0.0.1:$port`).

### Phase C — P2

- Unmanaged PT addresses + bridges e2e.
- `bootstrapSummary` polish from Arti status tags.

---

## 10. Acceptance criteria (BitChat parity)

Status after the 0.2.0 lifecycle conformance audit (see
`docs/audit/ARTITOR_0_2_LIFECYCLE_CONFORMANCE_REPORT.md`):

- [x] Cold start reaches `isReady` with real bootstrap % (not log scrape). — VERIFIED live on iOS simulator.
- [x] `pause` then `resume` re-binds SOCKS **without** full directory bootstrap (wall clock ≪ cold start). — VERIFIED live on iOS simulator.
- [x] `socksPort = 0` works; status exposes actual port. — VERIFIED live (ephemeral 64869) + Rust unit test.
- [x] Fixed port already in use → `ArtiException.Bind`, not hang. — VERIFIED via typed `on_error` (Rust unit test + facade fake test); live collision not exercised on device.
- [x] `start` twice while running → second waits or no-ops to ready (no `AlreadyRunning` crash in normal app toggle). — VERIFIED via facade fake tests (concurrent double start, timeout-join).
- [x] `shutdown` allows clean `start` again. — VERIFIED live on iOS simulator (second cold start after shutdown).
- [x] HTTP GET via SOCKS exits through Tor (existing e2e). — VERIFIED live on iOS simulator (api.ipify.org, HTTP 200).
- [x] No Android/iOS platform types in `commonMain`. — VERIFIED by compilation (E2E tests live outside `commonMain`).
- [x] Binary size / features: rustls, no OpenSSL; bridge-client + onion-service-client retained. — VERIFIED via Cargo.toml features (arti-client 0.43, rustls-only).
- [ ] Android live E2E (`:tor:connectedAndroidDeviceTest`) — NOT VERIFIED, no device/emulator in this environment (compiles via `assembleAndroidDeviceTest`).

---

## 11. One-liner for BitChat KMP

> **ArtiTor** becomes the shared engine: `start` / `pause` / `resume` / `shutdown` + `status.isReady` + auto SOCKS port.  
> **BitChat** deletes both `arti-build` and `localPackages/Arti`, keeps prefs + HTTP proxy wiring + Nostr reconnect as pure app code.
