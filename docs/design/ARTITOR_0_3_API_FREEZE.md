# ArtiTor 0.3 API Freeze — Normative Implementation Contract

Date: 2026-09-29
Status: FROZEN — Phase 1 CLOSED; integrated 0.3 implementation in progress
Baseline: ArtiTor 0.2 lifecycle contract stable; Ubique UniFFI migration
complete; `arti-client` 0.46.0.
Authority: this document is the implementation source of truth for 0.3.
The 1244-line capability audit remains the evidence base; wherever the two
conflict, this document wins. Remediation deltas are recorded as R1–R10 in
the audit addendum.

The original freeze was a documentation-only decision. The amendments below
authorize Phase-1 lifecycle remediation, with no Cargo feature changes or
Phase-2 work. Dormant / PT / hosting / RPC remain excluded. Scope is not expanded.

---

### Accepted Phase-1 amendments (2026-09-30)

The independent audit `docs/audit/ARTITOR_0_3_PHASE1_INDEPENDENT_AUDIT.md`
provides authoritative counterexamples. Sections 3–6 and 10–11 below amend the
original freeze: no creation timeout; opaque non-UUID-required ids; reentrant
callbacks; revisioned publications with bounded Kotlin pending bookkeeping;
atomic lifecycle/client-epoch admission; ERROR demotion before engine error
publication; identity-preserving explicit resume versus replacement start from
ERROR; optional rebind retry through pause/resume; bounded native tombstones
with permanent public terminal latches; and an honestly unmeasured private cap
with Phase-5 resource measurements. These are Phase-1 repairs, not Phase-2
permission or a declaration of final Phase-1 acceptance.

## 1. Scope (stable 0.3 core)

1. **Isolation sessions (SOCKS-level).** One shared root Arti `TorClient`;
   N additional isolation contexts, each an opaque `TorIsolationSession` with
   its own ephemeral SOCKS endpoint. No direct `DataStream` public API.
2. **Single isolation primitive.** `TorClient::isolated_client()` per session
   (§7). No `StreamPrefs::new_isolation_group()` combination in Phase 1.
3. **Bridge validation + enablement tri-state.** Keep `bridges: List<String>`
   wire shape; Rust validates every line pre-bootstrap via
   `BridgeConfigBuilder::parse/build` (typed `Config` on failure); add
   `bridgesEnabled: AUTO/ON/OFF` → `BoolOrAuto`. Rebuild-on-change (§9).
4. **Ordinary public `.onion` client as documented no-op.** No new API; prove
   + document + e2e-test SOCKS `.onion` (compiled-in stable
   `onion-service-client`, `allow_onion_addrs` default true).
5. **Stable error classification without new subclasses.** `kind: TorErrorKind`
   property on the existing 7 `ArtiException` classes, mapped only from stable
   `tor_error::ErrorKind::kind()` (§8). No `ErrorDetail`, no message parsing.
6. **Minimal config additions.** `bridgesEnabled`, `allowOnionAddrs`,
   `connectTimeout`, `resolveTimeout` (+ `stateDir`/`cacheDir` already exist).
   `allowLocalAddrs` is EXCLUDED (§9). No live `reconfigure()` (§9).
7. **Dynamic session endpoints.** `StateFlow<TorIsolationSessionStatus>` with
   atomic `(state, endpoint?)` emission (§§3, 6). Ephemeral ports; no port
   stability promise.
8. **Strict engine-owned session lifetime.** Engine owns all strong Tor
   references; session handles are logically invalidated on `close()` /
   shutdown / generation bump and never resurrect (§§4–6).
9. **Zero new Cargo features** for the stable 0.3 core (same proof as audit
   §4: isolation + onion-client + bridge validation + address/timeout knobs +
   `ErrorKind` mapping all work on the current feature set).

## 2. Non-goals (not in stable 0.3)

Pluggable transports (managed or unmanaged public API); onion-service hosting;
authenticated onion client; RPC (`tor-rpcbase`); vanguard public knob; direct
Tor streams (`DataStream`/`TorStream`/`openStream`); SOCKS username/password
isolation multiplexing; `isolate_every_stream`; `experimental-api`;
`error_detail`; `geoip` exit pinning; public dormant API; internal dormant FFI
or dogfood flag in any 0.3 phase; typed `Bridge{…}` record; `TorEvent` bus
beyond `status`+`logs`+session `status`; `ArtiTorClient` rename; full
advanced-config passthrough; any new Cargo feature in the shipped artifact.
Dormant research is retained but implementation moves to a 0.4 candidate:
`pause != dormant`, never silently coupled.

---

## 3. Public Kotlin API (frozen sketch)

Package `com.yet.tor`, `commonMain` only, `kotlinx-coroutines-core` only.
Additive to the 0.2 surface: no signature removed, no rename, no package move.
Generated UniFFI types stay internal (`com.yet.tor.ffi.*` never in signatures).

```kotlin
package com.yet.tor

import kotlin.time.Duration
import kotlin.time.Duration.Companion.seconds
import kotlinx.coroutines.flow.StateFlow

// --- Values ---------------------------------------------------------------
data class TorSocksEndpoint(val host: String, val port: Int)
// host is always "127.0.0.1" in 0.3. Never 0.0.0.0.

enum class BridgesEnabled { AUTO, ON, OFF }
// AUTO = use bridges iff non-empty (today's behaviour, now explicit).
// ON + empty bridges = ArtiException.Config. OFF + lines = lines ignored.

data class ArtiConfig(
    val dataDir: String,
    val socksPort: Int = 0,                       // root listener; 0 = ephemeral
    val bridges: List<String> = emptyList(),      // UNCHANGED wire shape
    val stateDir: String? = null,
    val cacheDir: String? = null,
    val bridgesEnabled: BridgesEnabled = BridgesEnabled.AUTO, // NEW, appended for 0.2 source compatibility
    val allowOnionAddrs: Boolean = true,          // NEW, Arti default
    // allowLocalAddrs: EXCLUDED from 0.3 (see §9).
    val connectTimeout: Duration = 10.seconds,    // NEW, Arti default
    val resolveTimeout: Duration = 10.seconds,    // NEW, Arti default
)

// --- Sessions --------------------------------------------------------------
enum class TorIsolationSessionState { ACTIVE, PAUSED, CLOSED, INVALIDATED }

data class TorIsolationSessionStatus(
    val state: TorIsolationSessionState,
    val socksEndpoint: TorSocksEndpoint?,
)
// Invariant: socksEndpoint != null ⟺ state == ACTIVE.
// Emission of (state, endpoint) pairs is always atomic: observers never see a
// new port with an old state or vice versa.

interface TorIsolationSession : AutoCloseable {
    val id: String                                // opaque diagnostic only (§10)
    val status: StateFlow<TorIsolationSessionStatus>
    val isClosed: Boolean                         // true ⟺ status.value.state == CLOSED
    override fun close()                          // idempotent, never throws
}

// --- Engine -----------------------------------------------------------------
class ArtiTorClient {
    // existing (UNCHANGED signatures): version, status, logs, isReady,
    // hasClient, start/pause/resume/shutdown/stop/restart.

    /** Root SOCKS endpoint convenience, derived from status. Null unless RUNNING. */
    val socksEndpoint: TorSocksEndpoint?
    // Equivalent to status.value.socksPort mapped to 127.0.0.1. Kept because
    // call sites wiring HTTP stacks want a typed endpoint; TorStatus.socksPort
    // remains the back-compat representation and the two MUST agree.

    /** Create an additional isolation context. Fails while engine has no client. */
    suspend fun createIsolationSession(): Result<TorIsolationSession>

    /** Snapshot of live (non-CLOSED, non-INVALIDATED) additional sessions. */
    val sessions: List<TorIsolationSession>
    // Root/default traffic is NOT in this list (see §4).

    /** Close one session (== session.close()). No-op on unknown/closed handles. */
    fun closeSession(session: TorIsolationSession)
}

// --- Errors ------------------------------------------------------------------
enum class TorErrorKind {
    // Legacy operation buckets (defaults for the 7 existing classes):
    ALREADY_RUNNING, NOT_RUNNING, CONFIG, BIND, BOOTSTRAP, TIMEOUT, RUNTIME,
    // Stable tor_error::ErrorKind-derived buckets (mapped from kind() only):
    NETWORK, EXIT_FAILED, TARGET_REJECTED, STORAGE, BOOTSTRAP_REQUIRED,
    // Session-lifecycle buckets (reuse Runtime/NotRunning classes, see §8):
    SESSION_CLOSED, SESSION_INVALIDATED,
    // Upstream #[non_exhaustive] fallback. Future Arti variants map here.
    UNKNOWN,
}

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
    class Bind(port: Int, msg: String) :
        ArtiException("failed to bind SOCKS on $port: $msg") {
        override val kind = TorErrorKind.BIND
    }
    class Bootstrap(msg: String) : ArtiException("bootstrap failed: $msg") {
        override val kind = TorErrorKind.BOOTSTRAP
    }
    class Timeout(msg: String = "timed out waiting for Tor ready") : ArtiException(msg) {
        override val kind = TorErrorKind.TIMEOUT
    }
    class Runtime(msg: String, override val kind: TorErrorKind = TorErrorKind.RUNTIME) :
        ArtiException("runtime error: $msg")
    // NOTE: zero new subclasses. Richer classification travels in `kind`
    // (overridden per throw-site from ErrorKind, defaulting as above).
    // Kotlin `when (e)` over ArtiException stays exhaustive; `when (e.kind)`
    // MUST always have an `else` branch (non-exhaustive tolerance).
}
```

Deliberately absent: `defaultSession` (withdrawn, see §4);
`TorIsolationSession.socksEndpoint` as a plain val (withdrawn — dynamic state,
see §6); `openStream()`; SOCKS auth; `allowLocalAddrs`; `goDormant/wake`;
session limit constant; typed bridge record.

---

### Integrated-run compatibility and rejection clarification (2026-09-30)

Phase 1 is CLOSED at `86da3c8dc71025278df66d588126cc0bef475b49`, independently accepted PASS WITH FOLLOW-UPS; the amendments above remain the governing semantics.
Phase-2 bridge hardening was implemented at `8bb0d95a4a865b1d322086e2b2ec37c26fb22c57`; integrated remediation addresses the subsequently reproduced public replacement defect.

The five pre-0.3 constructor positions are `dataDir`, `socksPort`, `bridges`, `stateDir`, `cacheDir`. New fields are appended after those positions. This corrects the decorative sketch's contradiction with §13's required old-source compilation; it changes no configuration semantics.

A candidate rejected during configuration validation must preserve the previously effective client, root/session states, resources and identities in RUNNING and PAUSED. Validation precedes destructive pause/shutdown/replacement, including explicit restart with a supplied candidate. Rust remains the configuration parsing/build authority; a small internal UniFFI preflight is permitted, without a new public KMP validation API. This clarifies accepted replacement versus rejected candidate; it does not promise rollback after filesystem, construction, bootstrap or bind failures following acceptance.

## 4. Native ownership model (frozen)

```text
ArtiTor / Engine (Rust `Shared`, behind Mutex — NEVER held across .await)
  owns strong:
    tokio Runtime
    root TorClient (Arc)
    root SOCKS listener task + tracked root connection tasks
    sessions: HashMap<SessionId, SessionRuntime>
    StatusListener (per instance) + generation counter (u64)
    root-client epoch + lifecycle/session transition gate

SessionRuntime (ENGINE-OWNED, never in the FFI object):
    isolated_client: Arc<TorClient>   // from root.isolated_client()
    socks listener JoinHandle + shutdown channel
    tracked connection JoinHandles (per accepted socket)
    state publisher (endpoint + state → Kotlin StateFlow via on_status-style callback)
    generation: u64                   // engine generation that created it
    status_revision: u64              // increments with state/endpoint mutation

SocksSession (UniFFI exported object — lightweight, non-owning):
    id: SessionId (opaque string)
    generation: u64
    weak: Weak<Shared>                // registry access only
  MUST NOT hold: TorClient / runtime / listener / tasks (strong).
```

Consequences:

- `shutdown()` drops root client + every `SessionRuntime` (strong isolated
  handles, listeners, tasks) + runtime. A Kotlin-retained `SocksSession`
  keeps only `(id, generation, Weak)` — Tor resources are unconditionally
  released. `Weak::upgrade()` fails → handle reports `INVALIDATED`.
- `close()` removes one `SessionRuntime` from the registry, aborts its
  listener + tracked connections, publishes `CLOSED`. Other sessions and root
  traffic are untouched.
- `pause()` stops root + all session listeners, aborts tracked connections,
  retains all clients (`root` + isolated handles → isolation identity
  preserved), publishes engine `PAUSED` + per-session `PAUSED` (endpoint null).
- Cold `start()` after shutdown bumps `generation`; old handles carry the old
  generation and can never validate against the new registry even if an `id`
  collided (ids are never reused across generations in practice; generation
  check is the hard barrier, id uniqueness the soft one).
- Stale-handle rule: a session created in generation N belongs only to N. It
  MUST NOT attach silently to generation N+1. Any liveness check compares
  `(id, generation)` against the live registry under lock, then releases the
  lock before any async work (no awaiting while holding a blocking mutex;
  tokio `Mutex` or `RwLock` for the registry with a strict ordering:
  transition gate → authoritative lifecycle/client epoch → registry, never
  the reverse). Callbacks may synchronously re-enter public/native accessors;
  no callback is invoked while the transition gate or registry is held.

Synchronization (Rust):

- One lifecycle/session transition gate makes lifecycle/client-epoch validation
  and registry mutation one transaction. Create final admission, rebind ACTIVE
  commit, pause/ERROR demotion, shutdown/rebuild invalidation, and relevant root
  transitions share it. Lock order: transition gate → inspect lifecycle/client
  epoch → registry; release registry and gate before callbacks. No `.await` or
  callback occurs while the gate is held.
- Root replacement/discard increments an internal client epoch, including
  config rebuild and ERROR cold recovery. Creation snapshots Shared identity,
  generation, client epoch, and root handle; final admission must still match.
  A mismatched epoch fails with existing `NotRunning`, even if replacement
  startup failed synchronously and no new root exists.
- Every session state/endpoint mutation increments its internal revision under
  the registry lock. Internal UniFFI callbacks carry id, state, port, revision.
  Kotlin accepts only revisions newer than that wrapper's latest revision and
  additionally latches terminal states. Revision is not public API.
- Kotlin pending callbacks exist only during the serialized in-flight creation
  scope. Reconciliation consumes the current id and clears orphan statuses.
  Unknown callbacks outside creation are ignored: storage is O(live wrappers +
  in-flight creation), not O(historical ids).
- `close()` vs `pause()`/`shutdown()` race: all three take the transition gate
  before the registry lock,
  `close()` removes-or-marks its entry idempotently; whoever wins, the loser
  observes absence and is a no-op. Endpoint updates are published exactly once
  per transition (idempotent publisher: duplicate `CLOSED`/`INVALIDATED`
  emissions are suppressed).
- Tasks aborted via per-session shutdown channel + `JoinHandle::abort()` +
  `on_closed` cleanup removing the entry if still present.
- After `shutdown()`, a Rust-level debug assertion (tests) verifies
  `Arc::strong_count` of every session client reached the engine-held baseline
  (i.e. no FFI object retained a strong client) — see §12.

UniFFI surface (sketch, names illustrative):

```rust
#[derive(uniffi::Object)] pub struct SocksSession { id: String, generation: u64, weak: Weak<Shared> }
impl ArtiTor {
    pub fn create_session(&self, listener: Box<dyn StatusListener>) -> Result<Arc<SocksSession>, ArtiError>;
    pub fn close_session(&self, session: &SocksSession);
    pub fn session_status(&self, session: &SocksSession) -> SessionStatusFfi; // (state, port?)
}
impl SocksSession { pub fn id(&self) -> String; pub fn close(&self); pub fn status_snapshot(&self) -> SessionStatusFfi; }
```

`ArtiError` gains NO new variants for sessions in the FFI enum; session
staleness is reported through the existing `Runtime`/`NotRunning` variants
with `kind` set to `SESSION_CLOSED`/`SESSION_INVALIDATED` in the Kotlin
mapping when Phase 3 adds the frozen kind taxonomy (see §8); Phase-1 admission
failures use the existing error model and add no public taxonomy.

---

## 5. Session lifecycle (frozen per-handle semantics)

- `createIsolationSession()`:
  - Legal only when the engine holds a client (RUNNING or PAUSED). Otherwise
    fails with `ArtiException.NotRunning` (kind `NOT_RUNNING`).
  - While RUNNING: binds a new loopback listener immediately (ephemeral port
    `0` → actual port), publishes session `ACTIVE` with endpoint, returns.
    Fixed-port app pinning is NOT offered in 0.3 (all session ports ephemeral).
  - While PAUSED: creates the session object + isolated client handle but does
    NOT bind yet; publishes session `PAUSED` (endpoint null). The listener
    binds on next successful engine `resume()` alongside the others.
  - While STARTING/BOOTSTRAPPING/STOPPING/ERROR/OFF: fails `NotRunning`.
    Rationale: sessions need a live `TorClient` to derive `isolated_client()`
    from; pre-bootstrap creation would mint an owner token on a discarded
    partial client (audit §3.1), so it is rejected, not queued.
  - Every synchronous generated UniFFI ArtiException is mapped through the
    existing public mapper before Result.failure. No `com.yet.tor.ffi.*`
    exception leaks through the public Result.
  - Bind failure on create: fails that call with `ArtiException.Bind`
    (kind `BIND`); engine state untouched; no half-registered session leaks.
- `session.close()`: idempotent, synchronous, never throws. First call stops
  its listener, aborts its tracked connections (fail-closed), publishes
  `CLOSED` (endpoint null), removes engine-side resources. Later calls are
  no-ops. `close()` is legal in every session state including `INVALIDATED`.
- Engine `pause()`: synchronously transitions every live session to `PAUSED`
  (endpoint null) before returning — same guarantee as the 0.2 engine
  `pause()` fallback semantics, extended across N listeners. Isolation
  identity (owner token) is preserved because isolated handles are retained.
  Old ports MUST stop accepting (listener dropped, backlog drained via abort).
- Engine `resume()`: rebinds root + every live non-CLOSED session. Ports MAY
  change (normally will). Each `(state, endpoint)` pair is emitted atomically.
  Root rebind failure → engine `ERROR` (existing semantics). Per-session
  rebind failure → that session stays `PAUSED` with null endpoint (+ log line,
  no secret material); engine stays `RUNNING` if root bound (§6 matrix).
  Retry happens through a normal pause → resume cycle or close/recreate.
  `resume()` while already RUNNING is a no-op, not a standalone session retry
  API. There is no dedicated retry API in 0.3.
- Engine entry into `ERROR`: set native ERROR and demote live sessions to
  `PAUSED`/null under the transition gate. After unlocking, publish session
  notifications, typed `on_error`, matching engine `on_status(ERROR)`, then
  diagnostic logging. Both engine callbacks must observe no ACTIVE session.
- `resume()` from ERROR may reuse retained root/isolated clients when native
  state permits. `start(...)` from ERROR is recovery/reconfiguration: invalidate
  existing sessions, advance the root-client epoch, and use a cold/replacement
  start even for identical configuration and a bootstrapped retained root.
- Engine `shutdown()` / replacement `start()` / `restart()` teardown: every
  live session transitions to `INVALIDATED` (endpoint null) exactly once.
  Handles stay physically reachable in Kotlin but are logically dead.
- Unexpected current session accept-loop OS failure demotes only that session
  from ACTIVE to PAUSED/null. A listener identity/revision check prevents an old
  listener failure from changing a closed, paused, invalidated, or rebound
  entry; it never poisons the root engine.
- Cold `start()` after shutdown: new `generation`; all pre-existing handles
  remain `INVALIDATED` forever. No resurrection, no silent re-attachment.
- Retained public Kotlin wrappers latch CLOSED or INVALIDATED forever. Native
  terminal tombstones are bounded implementation diagnostics: after pruning, a
  stale native CLOSED snapshot may degrade to INVALIDATED. It cannot override
  the public wrapper's authoritative terminal latch.
- `status` property access never throws in any state (returns the current
  snapshot, including `CLOSED`/`INVALIDATED`). `isClosed` is a pure alias.
  Any *future* session operation requiring live Tor resources (none exists in
  0.3 beyond `close`) MUST fail on `CLOSED` with
  `ArtiException.Runtime(kind=SESSION_CLOSED)` and on `INVALIDATED` with
  `ArtiException.Runtime(kind=SESSION_INVALIDATED)` (or `NotRunning` with the
  same kinds where the call site is engine-scoped — mapping table fixed in
  Phase 3; no new subclasses either way).

Why root and sessions are different ownership surfaces: the root endpoint is
the engine's own readiness signal (`TorStatus.socksPort`/`isReady`/`RUNNING`).
Letting app code close it independently would let the engine claim RUNNING
with no usable endpoint. Sessions are *additional* endpoints with no engine
readiness role, so per-session `close()` is safe. This asymmetry is intentional
and permanent.

---

## 6. Engine/session state matrix (frozen)

Engine states (unchanged 0.2 machine): `OFF STARTING BOOTSTRAPPING RUNNING
PAUSED STOPPING ERROR`. Session states: `NONEXISTENT ACTIVE PAUSED CLOSED
INVALIDATED` (`NONEXISTENT` = never created / already removed server-side).

| Operation                                                       | Engine effect                                                                       | Session effect                                                                                           | Notes                                                                     |
|-----------------------------------------------------------------|-------------------------------------------------------------------------------------|----------------------------------------------------------------------------------------------------------|---------------------------------------------------------------------------|
| `createSession` while RUNNING                                   | unchanged (stays RUNNING)                                                           | new session `ACTIVE` + bound endpoint                                                                    | bind failure → `Bind`, engine untouched                                   |
| `createSession` while PAUSED                                    | unchanged (stays PAUSED)                                                            | new session `PAUSED`, endpoint null, binds on next resume                                                | needs retained client; else `NotRunning`                                  |
| `createSession` while STARTING/BOOTSTRAPPING/STOPPING/ERROR/OFF | unchanged                                                                           | rejected: `NotRunning`                                                                                   | never queued; no token minted on partial clients                          |
| `session.close()` (ACTIVE or PAUSED)                            | unchanged (RUNNING stays RUNNING even if zero sessions remain; PAUSED stays PAUSED) | → `CLOSED`, endpoint null, connections aborted                                                           | root traffic + siblings unaffected; idempotent                            |
| `session.close()` (CLOSED/INVALIDATED)                          | unchanged                                                                           | no-op                                                                                                    | never throws                                                              |
| `pause()` from RUNNING                                          | → `PAUSED`, root listener down, % stays 100                                         | all live sessions → `PAUSED`, endpoints null, identities kept                                            | synchronous transition before return; old ports stop accepting            |
| `pause()` during STARTING/BOOTSTRAPPING                         | → `OFF` (0.2 rule, unchanged)                                                       | n/a (no sessions can exist)                                                                              | partial client discarded                                                  |
| `resume()` success (root + all sessions bound)                  | → `RUNNING`                                                                         | all live sessions → `ACTIVE` (possibly new ports, atomic pairs)                                          | no re-bootstrap (wall clock ≪ cold start)                                 |
| `resume()` root OK, ≥1 session bind fails                       | → `RUNNING` (root readiness decides)                                                | failed sessions stay `PAUSED` (null endpoint); bound sessions → `ACTIVE`                                 | session failure NEVER puts engine into ERROR; logged, secret-free         |
| `resume()` root bind fails                                      | → `ERROR` (existing) + typed `lastError`                                            | all live sessions → `PAUSED` (endpoints null, identities kept)                                           | 0.2 root semantics unchanged                                              |
| `shutdown()` from any state                                     | → `STOPPING` → `OFF`, runtime + all clients dropped                                 | all live sessions → `INVALIDATED`, endpoints null                                                        | strong refs gone (§12 invariant)                                          |
| `restart()` (shutdown+start)                                    | new generation                                                                      | all pre-restart sessions → `INVALIDATED` forever                                                         | new sessions get new ids under new generation                             |
| session endpoint bind failure (create or resume)                | never ERROR by itself                                                               | that session `PAUSED`/`Bind`-failed, siblings + root unaffected                                          | isolation: one session's error affects nothing else                       |
| engine enters ERROR (root cause)                                | → `ERROR`, zero ports, typed `lastError`                                            | all live sessions frozen as `PAUSED`-with-null-endpoint, then `INVALIDATED` on subsequent shutdown/start | session PAUSED callbacks precede typed error and engine ERROR publication |
| `resume()` from ERROR with reusable retained root | recovery/rebind → RUNNING if successful | retained sessions may → ACTIVE with preserved identity | native state must permit reuse; root bind first |
| `start(...)` from ERROR, including same config | cold/replacement start; new root-client epoch | all existing sessions → INVALIDATED forever | never the same-config SOCKS-only rebind path |
| `resume()` while RUNNING | unchanged (no-op) | unchanged, including failed PAUSED optional sessions | retry through pause → resume or close/recreate |
| closing last/only session                                       | engine stays RUNNING (root endpoint decides readiness, not session count)           | that session → `CLOSED`                                                                                  | audit §8 "RUNNING requires ≥1 session" rule is WITHDRAWN                  |

`isReady` definition UNCHANGED (root only): `state == RUNNING && percent ≥ 100
&& socksPort != null`. Session usability is separate:
`sessionUsable ⟺ engine hasClient && session status == ACTIVE &&
endpoint != null`. `TorStatus` gains no session fields in 0.3.

App rebuild rule: HTTP/Ktor/OkHttp/URLSession clients bound to a session
endpoint MUST be rebuilt on every `status` emission where `socksEndpoint`
changes (including `ACTIVE→PAUSED` teardown). Keeping a stale port is a
fail-closed violation: the old listener is gone, so stale use fails fast
(connection refused) rather than leaking clearnet.

---

## 7. Privacy / isolation guarantees (frozen contract language)

- Root/default SOCKS: pre-0.3 behaviour unchanged. Traffic through the same
  root endpoint MAY share circuits per Arti defaults. Document as-is; no new
  promise.
- Isolation session SOCKS: traffic within the same session MAY share circuits
  with other traffic in that same session. Traffic across distinct sessions
  MUST NOT share circuits. Traffic in any session MUST NOT share circuits with
  root/default traffic.
- Mechanism (Phase 1, exactly one primitive): each session holds
  `root.isolated_client()` (fresh owner `IsolationToken`, `client.rs:1466`);
  session accept loops call plain `isolated.connect(target)`. Two streams share
  a circuit iff owner token AND stream prefs both match (audit §3.1); distinct
  `isolated_client()` handles have distinct owner tokens, so cross-session (and
  session↔root) sharing is impossible by construction. No
  `connect_with_prefs` / `new_isolation_group()` in Phase 1.
- Still shared (intentional, documented): tokio runtime, `circmgr`/`dirmgr`/
  `chanmgr`/`guardmgr`, guard selection, directory/consensus, keymgr/state
  files, config, dormant sender. Sessions are circuit-partitioned, NOT
  network-unlinkable. Pause/resume preserves owner tokens (fast resume =
  correlation-preserving by design). Restart mints fresh tokens but persisted
  guards/state may still link at the network layer.
- NEVER promise: different exits, different guards, unlinkability, different IP
  addresses, anonymity identities, "new identity" on restart/resume. Docs and
  sample `TorController` MUST use the circuit-sharing wording above verbatim
  in spirit; marketing-grade anonymity claims are forbidden.
- Identity lifetime: session owner token lives as long as the engine-held
  `Arc<TorClient>` (retained across pause, dropped at shutdown). Tokens joined
  into live circuits persist there until circuits expire naturally.

---

## 8. Error evolution strategy (frozen: design B+C hybrid)

Problem: adding `Network/ExitFailed/TargetRejected/Storage/BootstrapRequired`
as sealed subclasses (audit §6) makes previously exhaustive downstream
`when (e)` non-exhaustive on recompilation — a source-incompatible minor.

Decision: **no new `ArtiException` subclasses in 0.3.** The 7 existing classes
keep their names, constructors (plus an optional `kind` default on `Runtime`),
and exhaustiveness. Richer classification travels in
`abstract val kind: TorErrorKind` (§3):

- A — more sealed subclasses: REJECTED (breaks downstream exhaustiveness).
- **B — stable category property: ADOPTED.** `TorErrorKind` enum with legacy
  defaults + new buckets + `UNKNOWN` fallback. Existing `catch` clauses keep
  working; `when (e)` stays exhaustive; new code branches on `e.kind` with a
  mandatory `else` (documents `#[non_exhaustive]` tolerance across FFI).
- C — structured metadata on existing broad exceptions: ADOPTED as the
  mechanism inside B (the `kind` value IS the structured metadata; no
  free-form detail record, no `ErrorDetail`).

Mapping rules (normative for Phase 3):

- Classification originates ONLY from `tor_error::ErrorKind::kind()` (stable,
  ~60 variants), never from message parsing. `Display`/`Debug`/`source` are
  explicitly excluded from semver (upstream `err.rs:28-33`).
- Representative mappings (Phase 3 pins the full table against 0.46 sources):
  directory/circuit-collapse → `Runtime`/`Bootstrap` class with kind `NETWORK`;
  exit refused/not-found/timeout → kind `EXIT_FAILED`; invalid/forbidden stream
  target (incl. `.onion` policy rejections) → kind `TARGET_REJECTED`; state/
  cache/keystore/`FsMistrust` → kind `STORAGE`; use-before-bootstrap →
  kind `BOOTSTRAP_REQUIRED`; closed-session use → `Runtime` kind
  `SESSION_CLOSED`; invalidated-generation use → `Runtime`/`NotRunning` kind
  `SESSION_INVALIDATED`.
- Upstream `#[non_exhaustive]` (`ErrorKind`, plus `DormantMode`-style enums
  generally): any unmapped or future variant → kind `UNKNOWN`. Future Arti
  variants MUST NOT require an ArtiTor breaking release.
- FFI transport: the existing `ArtiErrorDetail{kind, port?, msg}` pattern is
  extended with the `TorErrorKind` discriminant only (never `ErrorDetail`,
  never key material, never bridge lines — see §10 redaction). Kotlin maps
  `detail.kind` → `ArtiException.kind` without parsing `msg`.
- `msg` stays human-diagnostic only. It MUST NOT carry secrets (see §10) and
  MUST NOT be machine-parsed (adversarial-message tests already enforce this
  in 0.2 `ErrorMappingTest`; Phase 3 extends them with kind-not-text cases +
  synthetic-future-variant → `UNKNOWN`).

---

## 9. Config semantics (frozen)

```text
socksPort-only change → normal listener pause/rebind lifecycle (root and live sessions).
The bootstrapped TorClient and isolation-session identities are preserved, so
no Tor re-bootstrap occurs. Additional-session endpoints are ephemeral and may
receive new ports even without a collision: ACTIVE(old endpoint) → PAUSED(null)
→ ACTIVE(new endpoint). There is no session-port stability guarantee.
Applications MUST observe each session.status and rebuild proxy-bound network
clients when the endpoint changes. The configured root socksPort never selects
additional-session ports.
Any other public ArtiConfig change → teardown + rebuild (new TorClient, new bootstrap,
    all sessions INVALIDATED, callers recreate).
```

- TorClient-defining (rebuild-triggering): `dataDir`, `stateDir`, `cacheDir`,
  `bridges`, `bridgesEnabled`, `allowOnionAddrs`, `connectTimeout`,
  `resolveTimeout`. Compared exactly wholesale; non-empty→empty counts;
  never silently merged.
- `socksPort`: preserves TorClient and session identities; normal listener pause/rebind for root and live sessions, whose ephemeral ports may change.
- Live `TorClient::reconfigure()`: NOT used in stable 0.3 for ANY section.
  The audit's 2-section whitelist is withdrawn: upstream's non-reconfigurable
  list is explicitly incompletely documented (arti#1721), and deterministic
  rebuild already covers these fields with acceptable cost. `reconfigure()`
  stays future/internal research; any revival needs its own proposal with
  measurable justification (e.g. bootstrap-time win) — not a drive-by
  optimization inside 0.3 phases.
- `bridgesEnabled` semantics: `AUTO` = bridges used iff list non-empty
  (back-compat default); `ON` = bridges required (empty list → `Config`
  before bootstrap); `OFF` = list ignored (ship lines while disabled).
  Malformed lines → typed `Config` pre-bootstrap in all modes (fail fast).
- `allowOnionAddrs` (default true = Arti default): `false` + `.onion` target →
  `TargetRejected` classification (kind `TARGET_REJECTED`). No new API.
- `allowLocalAddrs`: EXCLUDED from stable 0.3. Rationale: no demonstrated
  normal mobile KMP need; localhost/LAN Tor targeting invites SSRF-style
  misuse (`IntoTorAddr` deliberately rejects `SocketAddr`/`IpAddr` upstream as
  a DNS-leak guard); Tor expectations are remote-or-onion. May return as
  advanced/future-only with a concrete product use case + threat review. Never
  default true.
- Timeouts (`connectTimeout`/`resolveTimeout`, 10 s defaults): apply at client
  construction (BEGIN wrap); no live update in 0.3 (rebuild path covers
  changes).

---

## 10. Logging / redaction rules + session identity + resource limits (frozen)

Logging (normative, reviewable in Phase 5):

- NEVER log: bridge lines / bridge addresses / fingerprints; SOCKS credentials
  (none exist in 0.3 — rule is forward-looking); onion authentication keys or
  key material; target hostnames (unless an explicit debug policy, off by
  default, allows them — default build logs classes, not names);
  raw `IsolationToken` values or any token bit-pattern.
- Session `id` MAY be logged: it is an opaque process-local diagnostic (see
  below), never a network identity. Log shape: `session=<id> state=… port=…`
  (port numbers are loopback-local and safe).
- `msg` propagation in typed errors MUST be reviewed for secret leakage in
  Phase 3: upstream error `Display` may echo config fragments; the Rust
  `notify_error` mapper redacts bridge/credential/key material before crossing
  FFI (follow the `sensitive()` pattern at `client.rs:1591`). Kotlin `logs`
  flow stays debug-only; never persist `ArtiConfig` with bridges to
  unprotected prefs (app-owned storage, documented).
- Tracing `LOG_SINK` last-writer-wins note (0.2) is unchanged.

Session identity (frozen):

- `id: String` = opaque diagnostic identifier, process-unique for a realistic
  process lifetime, never derived from `IsolationToken`, never persisted or
  intentionally reused. It is not public privacy identity, Tor identity, or a
  circuit identifier. Validity also depends on generation/client epoch. UUID
  formatting is not required: the current monotonic `sess-...` representation
  is permitted, but its formatting remains undocumented and non-contractual.
- Docs/samples MUST NOT encourage interpreting `id` as network identity,
  rotation signal, or unlinkability proof. Do not expose `IsolationToken`
  (raw u64) over UniFFI for any reason (replay/persistence/cross-restart
  confusion per audit §3.1).

Resource limits (frozen — the "8" is withdrawn):

- Cost per session (documented, not API): one `Arc<TorClient>` clone (cheap
  handle; shares `ClientShared`, NOT a second bootstrap), one loopback TCP
  listener + accept task, per-connection handler tasks, additional circuit
  pressure (partitioned, not free), FDs, small memory per handle.
- Decision: **(B) internal safety cap, no public constant.** Rust enforces a
  private non-contractual upper bound. The current 32 is a conservative
  implementation safety cap based on resource accounting, not target-platform
  memory measurements. The exact value MUST be empirically revisited during
  the Phase-5 device/resource audit and MAY change between minors without a
  compat note. It bounds listener/FD/task/circuit pressure under app loops;
  it is not evidence of a measured device capacity.
- Exceeding the cap: `createIsolationSession()` fails with the EXISTING
  `ArtiException.Runtime("session limit reached")` (kind `RUNTIME`), engine
  state untouched. No new exception class, no public constant, no config knob
  in 0.3.

---

## 11. Implementation phases (frozen — supersedes audit §14)

Each phase lands independently testable with `main` releasable. No phase
enables dormant/PT/hosting/RPC, adds Cargo features, or expands scope.

```text
Phase 1 — Isolation session foundation (sessions ONLY)
  Rust: registry + generation + Weak session objects (§4); isolated_client()
        per session (§7); per-session ephemeral SOCKS + connect() dispatch;
        per-session tracked connections + abort; atomic (state, endpoint)
        publisher; pause/resume/shutdown/invalidate semantics (§§5–6).
  Kotlin: TorSocksEndpoint, TorIsolationSession(State)Status, ArtiTorClient
        .socksEndpoint/sessions/createIsolationSession/closeSession (§3);
        root-vs-session ownership docs.
  Tests: Rust isolation-rule units (owner tokens differ → incompatible;
        same-stream-token across isolated handles still incompatible) +
        SOCKS-dispatch tests (no Tor network) + generation/ownership tests
        (shutdown drops strong refs); Kotlin fake-registry tests
        (create/close/list, idempotent close, shutdown-invalidates,
        pause/resume endpoint transitions); live Tor smoke (2 sessions,
        distinct ports, close-one-others-unaffected, pause/resume without
        re-bootstrap, shutdown invalidates).
  Gates: §12 ownership/isolation/pause/close/shutdown/restart invariants green.
  Explicitly NOT in Phase 1: bridges tri-state, error kinds, allowOnionAddrs,
  timeouts, .onion proof, dormant, PT, hosting.

Phase 2 — Bridge configuration hardening (bridges ONLY)
  Rust: pre-bootstrap BridgeConfigBuilder validation of every line; BoolOrAuto
        mapping; typed Config; redacted logging (§§9–10).
  Kotlin: BridgesEnabled enum + ArtiConfig defaults + back-compat tests.
  Tests: good/bad line tables (direct, PT-shaped w/o pt-client →
        NoCompileTimeSupport-equivalent Config, empty, garbage); ON+empty →
        Config; OFF+lines → ignored (no network); ON/OFF/AUTO matrix.
  Gates: zero-network validation suite green; secret-safe (no bridge material
        in logs/errors) verified by redaction tests.

Phase 3 — Error classification (errors ONLY, after §8 freeze)
  Rust: ErrorKind::kind() → TorErrorKind bucket table in notify_error;
        redaction review of msg paths.
  Kotlin: kind property on 7 classes (§3, §8); no new subclasses.
  Tests: kind table incl. synthetic future variant → UNKNOWN;
        kind-not-text adversarial tests; session CLOSED/INVALIDATED kinds;
        exhaustive-`when`-still-compiles regression (existing call sites
        unchanged).
  Gates: mapping table pinned to 0.46 sources; UNKNOWN fallback proven.

Phase 4 — Onion / config polish (proof + knobs, NO reconfigure, NO dormant)
  Prove public .onion over SOCKS (no API): live fetch positive matrix +
        negatives (bad .onion → TARGET_REJECTED; allowOnionAddrs=false →
        TARGET_REJECTED).
  Wire allowOnionAddrs + connect/resolve timeouts through construction +
        rebuild semantics (§9).
  Tests: Rust address-filter units; timeout-override units (mock black-hole);
        live .onion simulator + Android-hardware runs.
  Gates: .onion positive + both negatives green; defaults (true/10s/10s)
        unchanged unless app overrides.

Phase 5 — Full 0.3 audit (regression + release)
  Lifecycle/session regression; isolation determinism; Android hardware E2E +
        iOS simulator full script (+ device run if available); ABI check
        (no raw Arti objects over UniFFI); 16 KB page alignment; Apple SQLite
        symbol isolation; Maven consumer check; binary-size check (expect ~0
        feature-driven growth); API-compat check (existing call sites compile;
        exhaustive `when` intact); docs (README, sample TorController with
        1:1 identity→session mapping + never-pool-across-sessions rule).
  Resource acceptance: measure incremental session FD, memory, and task cost
        on an Android device and on iOS device/simulator where available;
        revisit the private cap using recorded measurements. No public limit
        constant or Phase-1 target-measurement claim.
  Gate: §13 all green; §12 invariants re-run live.
```

---

## 12. Test invariants (frozen, implementation-independent)

Ownership (after `shutdown()`, assert in Rust tests via registry + strong-count
probes and in Kotlin via facade fakes):

```text
root_client == none AND session strong clients == none AND runtime released
AND all session endpoints == null AND all old handles INVALIDATED
AND generation bumped AND old (id, generation) never validates again.
```

Isolation (deterministic, NEVER exit-IP comparison — Tor may legitimately pick
the same exit twice):

```text
session A isolation != session B isolation (owner tokens differ →
  StreamIsolation::compatible() == false, asserted via circmgr internals on
  unbootstrapped clients); same stream token across two isolated_client()s
  still incompatible; SOCKS-dispatch test: each accepted socket uses its own
  session's client (recorded per-connection, no Tor network).
```

Pause/resume:

```text
pause() → engine PAUSED + every live session PAUSED + every endpoint null,
  synchronously before return; isolation identity (owner token) preserved
  (same Arc retained); old ports refuse connections.
resume() → engine RUNNING (root bound) + sessions ACTIVE with atomic
  (state, new-endpoint) pairs; endpoint MAY differ from pre-pause (never
  assert equality); wall clock ≪ cold start (no re-bootstrap); session that
  failed to rebind stays PAUSED/null while engine stays RUNNING.
```

Close:

```text
close(A) → A endpoint closed + A tracked connections terminated + A CLOSED;
  B unaffected; root unaffected; engine state unchanged; second close(A) no-op.
```

Shutdown/restart:

```text
shutdown() → every session INVALIDATED + no stale handle retains Tor resources
  (strong-count baseline) + status OFF.
restart() (new generation) → old handles remain INVALIDATED; new sessions get
  fresh ids; old (id, generation) never validates; no silent re-attachment.
```

Create/bind:

```text
create while RUNNING → ACTIVE+endpoint or typed Bind (engine untouched).
create while PAUSED → PAUSED/null, binds on resume.
create while STARTING/BOOTSTRAPPING/STOPPING/ERROR/OFF → NotRunning.
session bind failure on resume → session PAUSED/null, engine RUNNING if root OK.
```

Errors/redaction:

```text
kind-table incl. synthetic future variant → UNKNOWN; adversarial messages never
  change classification; no bridge line / hostname / key material in logs or msg.
```

---

## 13. Acceptance gates (0.3 release)

1. §§1–10 of this document implemented with zero contradictions; audit R1–R10
   deltas honored (no `defaultSession`, no strong-client session objects, no
   immutable endpoint, no new subclasses, no live reconfigure, no
   `allowLocalAddrs`, no dormant work, no frozen "8").
2. §12 invariants green at Rust unit + Kotlin fake + live levels.
3. Live E2E (simulator + Android hardware): cold start→100%→root HTTP 200→
   create 2 sessions (distinct ports)→fetch via each→close one (other+root
   unaffected)→pause (all endpoints null)→resume (all back, no re-bootstrap,
   ports MAY change, app rebuilds clients)→`.onion` fetch→bad-line `Config`→
   shutdown (all invalidated)→cold restart (old handles stay dead).
4. Compat: existing 0.2 call sites compile unchanged; exhaustive `when` over
   `ArtiException` intact; `TorStatus.socksPort` agrees with `socksEndpoint`;
   single Maven artifact, unchanged Cargo feature set, ~0 feature-driven binary
   growth; 16 KB alignment, Apple SQLite isolation, R8/JNA rules intact.
5. Docs: README + sample `TorController` (1 session per app identity, never
   pool HTTP clients across sessions/endpoints, rebuild on `status` emission,
   never fall back to direct on Tor failure).

## 14. Deferred features (explicit parking lot)

- 0.4 candidate: dormant power overlay (`goDormant/wake` as separate API with
  race docs; never folded into `pause()`); revisit only with direct app demand.
- Later experimental modules (own artifacts/designs): unmanaged PT plumbing
  (after mock-PT proof); managed PT distribution (Android/desktop only; iOS
  managed stays REJECTED — sandbox forbids exec); authenticated onion client
  (needs keystore story); public direct streams (needs overhead/cancel proof);
  SOCKS username/password multiplexing (alternative to per-session ports);
  vanguards knob (with hosting or hardened-client model); typed `Bridge{…}`
  record (when PT forces it); `TorEvent` bus; full `reconfigure()` surface
  (never full passthrough — opinionated subsets only); `allowLocalAddrs`
  (needs use case + threat review).
- Never: RPC; `isolate_every_stream`; raw `IsolationToken`/`StreamPrefs`/
  `DataStream`/`TorClientConfig` over UniFFI; `error_detail`;
  `experimental-api` leakage into stable API; `geoip` exit pinning;
  `ArtiTorClient` rename; advanced-config passthrough.

---

*End of freeze — normative for 0.3 implementation. Next step: generate the
Phase 1 implementation prompt mechanically from §11 above.*
