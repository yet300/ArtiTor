# ArtiTor 0.2.0 Lifecycle Conformance Report

Date: 2026-09-29
Branch: `0.2.0`
Spec: `docs/api-lifecycle-bitchat.md` (now tracked in Git; was untracked)
Scope: audit + remediate the existing 0.2.0 lifecycle against the spec.
Explicitly out of scope (not started): arti-client upgrade, bridges/PT/isolation/onion-service API expansion, project redesign.

Method: implementation (`rust/arti-kmp-ffi/src/lib.rs`, `tor/src/commonMain/.../ArtiTorClient.kt`) compared against the spec section by section before changing code. Remediation preserves the public API shape and all existing E2E tests.

## 0. Specification file

| Item | Status |
|---|---|
| `docs/api-lifecycle-bitchat.md` tracked in Git | DONE (added on this branch) |
| Stale Gobley reference (§5, line 342) | FIXED → Ubique UniFFI artifacts |
| Architecture principle (lib owns TorClient+SOCKS+typed status; app owns policy/prefs/wiring/UI) | KEPT, no BitChat policy moved into ArtiTor |
| §2.1 pause-during-bootstrap, §2.2 cancellation, §3 config identity, §6 `on_error`/connections/single-instance, §10 checklist | DOCUMENTED from verified behavior |

## 1. Typed asynchronous errors

**Spec requirement.** Fixed SOCKS collision → `ArtiException.Bind`; bootstrap failure → `Bootstrap`; config failure → `Config`; runtime failure → `Runtime`. Error type must not live only in a status summary string.

**Implementation locations.** `rust/arti-kmp-ffi/src/lib.rs`: `ErrorKind`, `ArtiErrorDetail`, `StatusListener::on_error`, `Shared::notify_error`, `spawn_cold`/`spawn_socks` error paths. `tor/src/commonMain/.../ArtiTorClient.kt`: `StatusListener.onError`, `FfiErrorDetail.toPublic()`.

**Prior behavior.** Rust async work ran inside a `runtime.spawn` after the synchronous UniFFI call had returned `Ok(())`. Failures were reported as `on_status(Error, pct, None, "error: {e}")` / `"socks error: {e}"`. Kotlin mapped any `ERROR` status to `ArtiException.Runtime(summary)` — so async `Bind`/`Bootstrap`/`Config` all arrived as `Runtime`. The declared types were unobservable; only string parsing could recover them.

**Remediation.** New typed notification across UniFFI: `ArtiError` (a UniFFI `Error`, not passable as a callback argument) is mirrored by `ArtiErrorDetail { kind: ErrorKind, port: Option<u16>, msg: String }`. Rust calls `on_error(detail)` *before* the matching `on_status(Error, ..)` on the same worker thread (`Shared::notify_error`). Kotlin's `onError` installs the typed `ArtiException`; `onStatus(ERROR)` preserves a pre-existing typed error (`prev.lastError ?: Runtime(...)`), so either arrival order keeps the declared type. Mapping is driven by the `kind` discriminant, never by message text.

**Tests.**
- Rust (`cargo test`, 14 pass): `error_detail_preserves_declared_types` (all 6 variants), `notify_error_delivers_typed_detail_before_status` (ordering + status payload), `fixed_port_collision_is_typed_bind` (occupied loopback port → `Bind{port}`, no Tor network).
- Kotlin (`iosSimulatorArm64Test`, 45/45 pass): `ErrorMappingTest` (6 tests incl. adversarial messages proving kind-not-text mapping), facade tests for async Bind/Bootstrap/Runtime, sync Config, reverse-order arrival, ERROR invariant snapshot.

**Status: VERIFIED** (deterministic tests; live Bind collision on-device not exercised).

## 2. Lifecycle cancellation / races

**Spec requirement.** No suspend caller may wait out its timeout after another lifecycle operation definitively cancelled its operation; cancelled starts terminate deterministically; no state strands in STARTING/BOOTSTRAPPING after its worker is aborted. Cases: start→shutdown (STARTING/BOOTSTRAPPING), start→pause (STARTING/BOOTSTRAPPING), pause→pause, shutdown→shutdown, resume→shutdown, concurrent starts, start-while-RUNNING, start-after-ERROR, start-after-shutdown.

**Implementation locations.** `ArtiTorClient.kt`: `epoch: MutableStateFlow<Long>`, `awaitReady(myEpoch, timeout)`, `doPauseLocked`/`doShutdownLocked`, mutex discipline. `lib.rs`: `pause`/`shutdown` abort paths.

**Prior behavior.** `start()`/`resume()` held `lifecycleMutex` across the whole `awaitReady`, while `pause()`/`shutdown()` (non-suspending, no mutex) ran concurrently — but `awaitReady` only woke on `isReady || ERROR`. A shutdown/pause during bootstrap left the waiter hanging until its full timeout (up to 90 s). Rust `pause()` during bootstrap aborted the worker yet reported nothing, and Kotlin left `BOOTSTRAPPING` displayed forever.

**Remediation.** `pause()`/`shutdown()` bump a facade epoch (atomic `StateFlow.update`, no mutex needed). `awaitReady` captures the epoch after kicking work and wakes on `isReady || ERROR || epoch != myEpoch`; stale terminal snapshots at entry never match. ERROR throws the typed `lastError`; a foreign epoch bump throws `ArtiException.Runtime("...cancelled...")`; `withTimeout` still maps true timeouts to `ArtiException.Timeout`. `restart()` captures the epoch after its own internal shutdown bump so only foreign operations cancel it. Second concurrent starts serialize on the mutex then no-op (ready) or re-kick; a start after a facade-level timeout with the native worker still alive joins it via `AlreadyRunning`.

**Tests.** Facade fake tests: shutdown-during-bootstrap cancels fast (<10 s vs 30 s timeout), pause-during-bootstrap settles OFF + cancels fast, resume→shutdown cancels fast, concurrent double start (single kick, both succeed), post-timeout join of in-flight worker, start-timeout → `Timeout`. All deterministic, no network.

**Status: VERIFIED** (deterministic tests).

## 3. State-machine invariants

**Spec requirement.** OFF: no client/SOCKS/worker. STARTING/BOOTSTRAPPING: worker exists, no ready SOCKS. RUNNING: client + SOCKS + 100 + port. PAUSED: client, no SOCKS, 100, no port. ERROR: typed error, no readiness. Post-shutdown: OFF + nothing retained + runtime released.

**Implementation locations.** `lib.rs` (`pause`/`shutdown` postconditions, `Shared::report` port bookkeeping), `ArtiTorClient.kt` (`checkLifecycleInvariants`, status construction).

**Prior behavior.** Invariants held on the happy path but were never asserted; pause-during-bootstrap violated "no stranded BOOTSTRAPPING"; ERROR violated "typed error available" (see §1).

**Remediation.** `checkLifecycleInvariants(...)` internal helper encoding every spec invariant observable from the facade; asserted in facade tests after each scenario. Rust `pause`/`shutdown` now establish the terminal states synchronously (OFF/PAUSED/OFF) with client/flags/port cleared; Kotlin fallbacks cover FFI callback races only.

**Tests.** `LifecycleInvariantTest` (6 tests: every state, positive + violation cases, `isReady` definition); facade tests assert empty violations after cold start, pause, error, shutdown. Rust worker-existence half is structural (single `worker` slot, abort-on-pause/shutdown).

**Status: VERIFIED** (deterministic tests; worker-existence by code inspection).

## 4. Pause semantics during bootstrap

**Spec requirement.** Deterministic; preferred: cancel bootstrap, discard partial client, stop work, go OFF.

**Implementation locations.** `ArtiTor::pause` (`lib.rs`), `ArtiTorClient.pause` (facade sync), spec §2.1, report §4.

**Prior behavior.** Worker aborted, partial `TorClient` (if created) retained, no status emitted; Kotlin stuck in BOOTSTRAPPING indefinitely.

**Remediation.** Adopted the preferred behavior exactly: abort worker, abort tracked connections, `set_client(None)`, clear flags/port, report `OFF` ("bootstrap cancelled"). Facade mirrors to OFF if still STARTING/BOOTSTRAPPING without a client. Documented in spec §2.1 and in `pause()` rustdoc/KDoc.

**Tests.** Facade `pauseDuringBootstrapSettlesToOffAndCancelsWaiter`; E2E pause-after-ready unchanged.

**Status: VERIFIED** (deterministic test).

## 5. Configuration identity

**Spec requirement.** `socksPort` rebinds without bootstrap; `dataDir`/`stateDir`/`cacheDir`/`bridges` (incl. non-empty→empty) rebuild; never silently keep a client built with different bridges.

**Implementation locations.** `lib.rs`: `tor_client_config_changed`, `start()` resume-vs-rebuild branch, wholesale `last_config` replacement. `ArtiTorClient.kt`: `torClientConfigChanged`, PAUSED/RUNNING `start(config)` routing, spec §3 note.

**Prior behavior.** Two defects: (a) Rust resume path merged only `socks_port` and non-empty `bridges` into stored config while keeping the old client — clearing bridges was silently dropped, and `dataDir`/`stateDir`/`cacheDir` changes were ignored entirely; (b) Kotlin's PAUSED path called `native.resume(listener)`, discarding the supplied config (a requested port change never reached native).

**Remediation.** Exact comparison (`data_dir`, `state_dir`, `cache_dir`, `bridges`; `socks_port` excluded). Difference → teardown old client + cold bootstrap; otherwise → rebind with new port. Stored config replaced wholesale. Kotlin passes the supplied config through `native.start` for PAUSED and reconfigures RUNNING (port-only → pause+rebind; client-defining → shutdown+cold start).

**Tests.** Rust: 8 config-identity tests incl. `bridges_nonempty_to_empty_needs_new_client` regression. Kotlin: `ConfigIdentityTest` (8, mirrors Rust) + 4 facade routing tests (port-only rebind without teardown; bridges change tears down; PAUSED start forwards new port; PAUSED start forwards cleared bridges).

**Status: VERIFIED** (deterministic tests).

## 6. Restart contract

**Spec requirement.** `restart(config: ArtiConfig? = null, timeout = 90.seconds)`; null uses the last successful/effective config.

**Implementation location.** `ArtiTorClient.kt`: `lastConfig` (mutex-guarded, set only on success), `restart`.

**Prior behavior.** Signature was `restart(config: ArtiConfig, ...)` — config required; native `shutdown()` cleared `last_config`, so "restart with saved config after shutdown" was impossible. Silently diverged from the spec.

**Remediation.** Implemented the spec signature exactly. `restart(null)` uses the facade-stored last-effective config (independent of native shutdown state); with neither it fails fast with `ArtiException.Config`. No spec divergence, so no spec rewrite was needed — only a clarifying paragraph.

**Tests.** Facade: restart-after-shutdown uses saved config, supplied config wins, restart-without-any-config fails `Config` with zero native calls.

**Status: VERIFIED** (deterministic tests + live second cold start via E2E `shutdown → start`).

## 7. Worker / task ownership

**Spec requirement.** No lifecycle-critical detached tasks surviving pause/shutdown; explicit drain-vs-terminate decision for SOCKS streams.

**Implementation locations.** `lib.rs`: in-task bootstrap progress loop, `Shared::{track_connection, abort_connections}`, `pause`/`shutdown`.

**Prior behavior.** (a) The bootstrap progress reporter was a detached `tokio::spawn` sibling of the worker — aborting the worker leaked it, and it kept emitting BOOTSTRAPPING against the shared listener. (b) Per-accepted-connection `tokio::spawn(handle_socks…)` tasks were untracked; pause/shutdown left live Tor streams running after "Tor OFF".

**Remediation.** Progress reporting folded into the worker task via `select!` over `bootstrap_events` + the pinned `bootstrap()` future (abort kills both; scoped so the borrow ends before `client` moves into `run_socks`). Connection handlers are tracked in `Shared::connections` (opportunistic prune) and aborted on pause/shutdown. Decision (documented in code + spec §6): **option B — terminate all active streams** (fail-closed Tor OFF; BitChat's OFF toggle must not retain traffic, and no drain timeout exists in the spec).

**Tests.** Rust `abort_connections_drains_tracked_tasks`; facade cancellation tests prove no stranded waiters; live E2E resume timing heuristic (<30 s) still guards against accidental re-bootstrap.

**Status: VERIFIED** (deterministic tests + live E2E).

## 8. Multiple instances / global state

**Spec requirement.** Decide one-vs-many instances for `LOG_SINK` et al.; enforce/document, don't silently misbehave or redesign.

**Implementation location.** `lib.rs`: `LOG_SINK` handling in `set_listener`/`shutdown`, `ArtiTor::new` docs; facade KDoc.

**Prior behavior.** `LOG_SINK` last-writer-wins with no documentation; `shutdown()` unconditionally cleared it, stealing log routing from a second live instance.

**Remediation.** Documented single-instance support (last-writer-wins for *logs only*; status/error callbacks are per-instance and unaffected). `shutdown()` now clears `LOG_SINK` only if it still points at its own listener (`Arc::ptr_eq`). No multi-instance redesign.

**Tests.** By inspection + build; no live multi-instance test (out of scope by design).

**Status: VERIFIED by inspection** (documented; conditional release).

## 9. Remaining verification matrix

| Check | Result |
|---|---|
| `cargo test` (rust/arti-kmp-ffi) | 14/14 PASS |
| `:tor:compileTestKotlinIosSimulatorArm64` + `:tor:iosSimulatorArm64Test` | 45/45 PASS (24 lifecycle + 8 config + 6 error-map + 6 invariant + 1 live E2E) |
| `:tor:assembleAndroidDeviceTest` (extended Android E2E compiles) | PASS |
| Live iOS simulator E2E: cold start → 100% → SOCKS req → pause → resume → 2nd req → shutdown → cold start again → 3rd req → shutdown | **VERIFIED** (46 s run; ephemeral port; HTTP 200 via Tor; `SECOND COLD START ok`) |
| Live Android E2E (`:tor:connectedAndroidDeviceTest`) | **NOT VERIFIED** — no `adb`/device/emulator in this environment |
| `cargo check --examples` (host_poc with `on_error`) | PASS |

No compile success was counted as runtime verification: the live column above is a real Tor-network run; everything else is labeled deterministic-test or inspection.

## 10. Remaining risks / follow-ups

1. **Android live E2E not run** — execute `:tor:connectedAndroidDeviceTest` on hardware before release (same script as iOS, now extended with second cold start).
2. **Live fixed-port collision** not exercised on-device (Bind path proven by Rust + facade tests only).
3. **Multi-instance** remains single-instance-by-documentation; a second `ArtiTor` in one process steals log routing (status/error unaffected).
4. **Concurrent-start serialization**: a second `start()` waits on the lifecycle mutex for the first to finish, then no-ops/re-kicks — correct but not parallel; acceptable per spec ("no second bootstrap").
5. Facade `lastConfig` is in-memory only; process death still requires the app to supply a config (unchanged from spec).

## Recommendation

**PASS WITH FOLLOW-UPS**

All eight audit areas are remediated, specified, and tested: typed async errors cross UniFFI without string parsing; cancellation is deterministic; pause-during-bootstrap settles to OFF; config identity rebuilds vs rebinds correctly; `restart()` matches the spec signature; no detached tasks survive; single-instance is documented and enforced where cheap. 14 Rust + 45 simulator tests pass, and the full live Tor sequence (including shutdown → cold start again) is verified on the iOS simulator. Ship 0.2.0 after follow-up (1): the Android live E2E on hardware. Do not begin the Arti upgrade or API expansion (explicitly out of scope).
