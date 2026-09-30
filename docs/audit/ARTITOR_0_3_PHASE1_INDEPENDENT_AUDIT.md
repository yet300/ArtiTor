# ArtiTor 0.3 Phase 1 Independent Audit

Date: 2026-09-30, Asia/Tbilisi. Reviewer did not implement Phase 1.

**The completion claim is disproved.** Existing tests pass in cases that do not cover several reproducible violations of session lifecycle and observable-state invariants. No production code was modified, no findings were fixed, and Phase 2 was not started.

Confidence: **high** for the reproduced failures and the source locations below; **moderate** for exhaustive concurrency coverage; **unknown** for the underlying cause of the separate repeated live root-CONNECT failure.

## 1. Scope, baseline, and evidence discipline

Repository: `https://github.com/yet300/ArtiTor`; local checkout `/Users/yet/development/Multiplatform/ArtiTor`. HEAD: `0ac8867fda0cec5fe47166ff9cb7ab17b226cfab`. The reviewed implementation is the **pre-existing dirty working tree**, not that commit alone. The native file, Kotlin facade, and two test files were already modified; the freeze, capability audit, and implementation report were already untracked. This audit does not claim to review an immutable submitted commit.

Primary authority: `docs/design/ARTITOR_0_3_API_FREEZE.md`. Claim under review: `PHASE 1: PASS WITH FOLLOW-UPS` in `docs/audit/ARTITOR_0_3_PHASE1_IMPLEMENTATION_REPORT.md`.

Location abbreviations used throughout this report:

| Name | Exact repository file |
|---|---|
| R | `rust/arti-kmp-ffi/src/lib.rs` |
| K | `tor/src/commonMain/kotlin/com/yet/tor/ArtiTorClient.kt` |
| KT | `tor/src/commonTest/kotlin/com/yet/tor/ArtiTorSessionConcurrencyTest.kt` |
| KL | `tor/src/commonTest/kotlin/com/yet/tor/ArtiTorClientLifecycleTest.kt` |
| F | `docs/design/ARTITOR_0_3_API_FREEZE.md` |
| I | `docs/audit/ARTITOR_0_3_PHASE1_IMPLEMENTATION_REPORT.md` |

Code discovery used the refreshed codebase-memory graph, including the inbound trace of `Shared.notify_error`; full lexical source inspection followed because lock lifetime and callbacks require more than a call graph. The complete production native implementation and all existing common test bodies were read. Relevant generated API/build output, iOS E2E, and the previous root SOCKS implementation at HEAD were also inspected.

Production SHA-256 values, checked again after the disposable probes:

```text
R e721119f6c84943d52f5b4cbfbfcfc42ff91d42c6e8fd2f7d9d2e4cd6112b65e
K 95566333ac556501c8f3001101212a98f8a0f7eb21feafb251a774a2f33d2216
```

The two existing edited common-test files also retained their starting hashes. Disposable probes were created under `/private/tmp`, with unchanged repository production sources. Six Rust probes append tests to a production-identical copy. Two additional scheduling probes insert **test-only barriers** immediately after an engine-state read in the copied create/rebind paths. Those barriers force a feasible preemption; they do not change the predicate, registry mutation, error transition, callback, or dispatch logic. Kotlin probes replace only the common-test source directory through a disposable Gradle init script. A separate live probe replaces only `iosTest` sources.

The disposable Rust package is named `artitor-independent-audit`, giving it a different Cargo package identity and test-binary hash from the repository package even though dependency artifacts are shared. The original repository Rust suite was rerun afterward and passed using its own binary.

## 2. Findings at a glance

| ID | Classification | Finding | Evidence |
|---|---|---|---|
| F01 | HIGH | Session creation leaks generated FFI exceptions through the public facade | Failed Kotlin probe |
| F02 | HIGH | A delayed ACTIVE publication can overwrite a completed PAUSED publication | Failed native scheduling probe without production instrumentation |
| F03 | HIGH | Engine-state validation and session commit are not atomic against ERROR or rebind versus pause | Two failed native probes with test-only scheduling barriers |
| F04 | HIGH | Rebuild does not establish a replacement epoch; an old-client create can commit after teardown | Failed native probe using the existing synchronous-failure hook |
| F05 | MEDIUM | ERROR observers see ACTIVE sessions before demotion | Failed native re-entry observation probe |
| F06 | MEDIUM | `start()` after ERROR with a retained root reuses sessions instead of invalidating as the matrix specifies | Failed native contract probe; normative contradiction acknowledged |
| F07 | MEDIUM | Pending Kotlin statuses grow with historical terminal sessions | Two failed Kotlin probes, 160 retained records each |
| F08 | MEDIUM | Session accept-loop failure leaves a dead endpoint advertised ACTIVE | Source counterexample; not fault-injected |
| F09 | LOW | Bounded tombstone cleanup changes native CLOSED snapshots to INVALIDATED | Failed native churn probe; Kotlin latch remains protective |
| F10 | DOCS | Freeze and implementation silently disagree on signature, IDs, and ERROR semantics | Document/API comparison |
| F11 | DOCS | Internal cap rationale has no target FD/memory measurement evidence | Source/report comparison |
| F12 | MEDIUM | Required wrapper-level isolation-dispatch regression evidence is incomplete | Test-body inspection; correct implementation established separately |
| F13 | MEDIUM | SOCKS framing defects persist in the shared parser/handler | Short-hostname failed probe and comparison with HEAD; inherited defects |
| F14 | LOW | A test named “typed detail before status” does not assert ordering | Test-body inspection |

There is no BLOCKER-class allegation of demonstrated cross-session circuit sharing or clearnet fallback. The HIGH findings are sufficient to reject completion. Severity is not inferred from the number of tests or failures.

### F01 — HIGH: Public session failures expose generated exceptions

**Invariant:** F §§3/5/10 require public `ArtiException.NotRunning`, `Bind`, or `Runtime` failures, with generated UniFFI types internal to the facade.

**Location:** K:486–489, `createIsolationSession`; compare K:539–556, `kickStart`, and K:426–432, `resume`, which explicitly map `FfiArtiException` through `toPublic()`.

**Counterexample:** An OFF engine causes `native.createSession()` to throw generated `com.yet.tor.ffi.ArtiException.NotRunning`. `runCatching` catches it unchanged. The public result contains that generated exception, not `com.yet.tor.ArtiException.NotRunning`. Bind and cap failures follow the same unmapped path.

**Reproduction:** `auditCreateFailureUsesPublicException` supplies that exact native failure. The simulator assertion fails with `actual=com.yet.tor.ffi.ArtiException.NotRunning`. This is a runtime type leak even though the declared method signature contains no FFI type.

**Existing coverage / detection:** KT only scripts successful session creation. `ErrorMappingTest` tests the async detail mapper, not this call site. Existing tests do **not** detect F01. Confidence: **high**.

### F02 — HIGH: Mutation order does not determine publication order

**Invariant:** Pause must synchronously publish PAUSED/null before returning and must not subsequently be undone by an older ACTIVE event. Native comments also promise no publication after a terminal event.

**Location:** R:1344–1396 versus R:1415/1423, initial registry commit and publication; R:1802–1822, rebind commit and publication; R:1080–1125, pause; K:304–305 and K:206–210, nonterminal status acceptance.

**Counterexample:**

```text
create commits ACTIVE under engine/registry locks
create releases locks and enters ACTIVE callback
callback is preempted before it records/applies ACTIVE
another thread completes pause, records PAUSED/null
older ACTIVE callback resumes and records/applies ACTIVE/old-port
```

The accept barrier still works: it remains closed until the old callback returns. That does not serialize state publications. A Kotlin ACTIVE-to-PAUSED transition has no terminal latch, so it accepts the later-arriving stale ACTIVE value. Observable status can therefore contradict the paused registry indefinitely, without further native transitions.

**Reproduction:** `audit_pause_cannot_be_overwritten_by_delayed_active_publication` blocks only the listener before recording ACTIVE. Pause completes on the independent test thread. Recorded history is exactly **PAUSED → ACTIVE**, while native snapshot is **PAUSED/null**. This appended test does not instrument production.

The same unlocked pending-publication pattern permits CLOSED/INVALIDATED followed by an already-pending ACTIVE/PAUSED native callback. Kotlin protects the terminal wrapper value, but F07 shows that stale callbacks can still affect its central pending registry.

**Existing coverage / detection:** The four publication-reentry tests apply/record ACTIVE first, then synchronously close/pause *inside* that ACTIVE callback. They prove that ordering and dispatch gating for that schedule. They do **not** force pause before application of a delayed ACTIVE callback, and remain green under F02. Confidence: **high**.

### F03 — HIGH: Final state validation is a check/use race

**Invariant:** No ACTIVE session may be committed after completed pause or in engine ERROR; final admission must validate the current usable engine state atomically with insertion/update.

**Locations:** R:1349–1368, create final state read followed by registry lock; R:1803–1816, rebind `usable` read followed by registry lock; R:517–566, asynchronous ERROR and demotion.

**Counterexample A — rebind versus pause:** Rebind reads `usable=true`, then pauses before taking `Shared.sessions`. `pause()` sets engine PAUSED and demotes the registry. Rebind then takes the registry lock and observes an entry still PAUSED: exactly the predicate it needs. The cached `usable=true` lets it commit/publish ACTIVE after pause has returned.

**Counterexample B — create versus ERROR:** Create reads RUNNING in final validation while holding `ArtiTor.inner`, then is preempted before taking `Shared.sessions`. `notify_error()` needs no `ArtiTor.inner`: it sets ERROR and demotes the presently empty registry. Create resumes, inserts ACTIVE, and publishes it. No later demotion is scheduled.

**Reproductions:** Two disposable test-only gates force those read/commit intervals. Results:

```text
rebind: engine=Paused, session=Active/Some(port)
create: engine=Error, create=Ok, session=Active/Some(port)
```

The offline harness uses the same synthetic bootstrapped-state/unbootstrapped-client technique as the existing native tests. These probes establish scheduling defects in the state/registry logic, not Tor network behavior.

The session dispatch gate separately checks engine RUNNING, so these observations do **not** establish that a socket dispatches Tor traffic while the native engine remains PAUSED/ERROR. They do establish false live endpoints, a registry invariant violation, and invalid public state. That distinction matters.

**Existing coverage / detection:** Create-versus-pause and create-versus-shutdown gate *before* final validation. Callback-reentry pause happens *after* ACTIVE commit. ERROR tests inspect only after the helper finishes. None forces either read/commit interval. Existing tests do **not** detect F03. Confidence: **high**.

### F04 — HIGH: Rebuild does not invalidate in-flight old-client derivation

**Invariant:** Every TorClient replacement invalidates old isolation contexts and prevents an old-client create from committing into the replacement lifecycle. No successful session creation is legal when the engine has no client.

**Locations:** R:1199–1233, root/Shared/generation snapshot; R:1239, isolated derivation; R:958–974, rebuild; R:1345–1395, final validation. `generation`/`Shared` replacement occurs at R:1148–1149 on shutdown, not in the native rebuild branch.

**Counterexample:**

```text
engine PAUSED, create derives isolated client from old root
create waits at existing precommit gate
start(changed dataDir) invalidates registered sessions and clears root
replacement spawn_cold fails synchronously
engine still has same Shared, same generation, state PAUSED, no root client
release create: final check accepts PAUSED and inserts old isolated client
```

**Reproduction:** `audit_create_crosses_failed_rebuild_without_generation_barrier` uses the existing failure hook and public native `start()`. Observed `before=1, after=1`, `has_client=false`, successful PAUSED creation, and one live registry entry. The expected `NotRunning` assertion fails. No production scheduling instrumentation is required for this probe.

A successful rebuild similarly has no hard epoch barrier against an operation that spans replacement: pointer/generation checks cannot distinguish old and replacement roots in the same Shared. The measured failure case is stronger than merely conjecturing eventual ID collision.

**Impact qualification:** Kotlin's `lifecycleMutex` serializes its ordinary create/start calls. The native exported API itself does not uphold the audited concurrency contract; a facade serialization property is not a native replacement barrier. Async ERROR and rebind work are also outside that facade lock.

**Existing coverage / detection:** Rebuild synchronous-failure tests establish that *already registered* sessions receive INVALIDATED. They do not start an unregistered old-client create across teardown. Shutdown-race tests pass because shutdown actually changes Shared/generation. Existing tests do **not** detect F04. Confidence: **high** for the measured failure case.

### F05 — MEDIUM: ERROR publication precedes session demotion

**Invariant:** Engine ERROR means every live session is PAUSED/null in both native and observable state.

**Location:** R:517–532; K:354–364, `onError` immediately changes public engine status to ERROR.

**Counterexample:** `notify_error()` calls `on_error`, calls `report(Error)`/`on_status(Error)`, and only then calls `demote_sessions_to_paused`. An error callback or ERROR status observer synchronously queries a session and sees ACTIVE with a non-null endpoint. During `on_error`, the native state is still RUNNING; Kotlin has already exposed ERROR.

**Reproduction:** `audit_error_callbacks_must_observe_demoted_sessions` snapshots the same session in both callbacks. Both observations are ACTIVE/Some(port). After the helper returns, the existing demotion works; this finding is about publication consistency and re-entry, not denial that eventual demotion exists.

**Existing coverage / detection:** `engine_error_freezes_sessions_paused_with_null_endpoints` and root-bind-failure tests inspect after completion, so remain green. Existing tests do **not** detect F05. Confidence: **high**.

### F06 — MEDIUM: ERROR recovery takes a retained-root resume branch

**Invariant:** F §6 explicitly says ERROR sessions are PAUSED/null and INVALIDATED on subsequent shutdown/**start**. I describes cold/error-recovery invalidation as preserved.

**Location:** R:949–995. Branch selection considers retained client/bootstrap/config identity, without excluding ERROR. R:999–1011 invalidates only after that branch is bypassed.

**Counterexample/reproduction:** Create a session; enter ERROR with `notify_error(Bind)` while retaining the bootstrapped root; call native `start()` with the same config. It takes `spawn_socks`, retains the same session, and may rebind it ACTIVE. The immediate observed snapshot is PAUSED, not INVALIDATED; recorded events contain no INVALIDATED. `audit_start_after_error_must_invalidate_retained_sessions` fails its frozen-matrix assertion.

This is distinguishable from explicit `resume()`, which may reasonably preserve identity. Whether same-config `start()` after ERROR should intentionally act like resume needs a coherent contract decision; the frozen matrix currently says otherwise. F §5's conflicting immediate ERROR invalidation wording is F10, not an excuse to silently pick a third rule.

**Existing coverage / detection:** The cold-error-recovery test explicitly removes the root and clears the bootstrap flag to force the cold branch. It misses the normal retained-root bind-error case. Kotlin `startAfterErrorRecovers` uses a fake without sessions. Existing tests do **not** detect F06. Confidence: **high** for observed branch behavior; **moderate** for intended product semantics because the freeze is inconsistent.

### F07 — MEDIUM: Pending status storage is historical, not bounded by live creation

**Invariant:** Client-owned steady-state bookkeeping must be O(live sessions + in-flight creation), with terminal wrappers latched forever and removed from live snapshots.

**Locations:** K:304–315, central listener; K:491–523, wrapper initialization/reconciliation.

**Counterexample A:** ACTIVE and then INVALIDATED arrive before native creation returns. `initialStatus` is terminal, so the `if (initialStatus.state.isLive)` block is skipped. No pending terminal record is consumed. This is a genuine supported native create/shutdown publication interval.

**Counterexample B:** After a registered wrapper receives CLOSED, its pending record is deleted. A delayed PAUSED/ACTIVE callback subsequently has neither wrapper nor pending terminal record, so it creates a new unowned pending record. Native unlocked publication permits this; the wrapper's own terminal latch does not prevent central-map growth.

**Reproductions:** `auditTerminalBeforeWrapperDoesNotAccumulateHistory` performs 160 completed terminal creations; `auditLateCallbackAfterCloseDoesNotAccumulateHistory` performs 160 closes with stale callbacks. Each has **0 live sessions, 0 creations in flight, 160 pending records**. Both fail. The latter also confirms each returned wrapper remains CLOSED.

**Existing coverage / detection:** Snapshot reconciliation tests begin with older ACTIVE, enter the live initialization block, and consume a newer terminal record there. They assert cleanup before emitting stale callbacks, but do not assert cleanup afterward. They do not exercise initial-terminal initialization. Existing tests do **not** detect F07. Confidence: **high**.

### F08 — MEDIUM: Accept-loop error does not demote the session

**Invariant:** An unusable optional session must have PAUSED/null, without poisoning the root engine.

**Location:** R:1940–1947, session `accept()` error branch; contrast R:1865–1884, conversion-error demotion.

**Counterexample:** `socks.accept()` returns an OS error, for example process FD exhaustion. The branch logs and breaks. The socket/accept loop disappears, but the registry entry remains ACTIVE with its old port and a completed listener task. No callback clears the Kotlin endpoint. Additional accepted connections are not bounded by MAX_SESSIONS, so that cap alone cannot rule out FD exhaustion.

**Evidence:** Direct source control flow; no fault injection was run for this path. Session conversion failures do attempt demotion, but their unconditional PAUSED callback can also race terminal removal; that is another instance of F02's publication problem.

**Existing coverage / detection:** No test forces session accept failure or conversion failure. Existing tests would **not** detect F08. Confidence: **high** for branch behavior, **unknown** for frequency on devices. Root accept-error handling has a similar pre-existing limitation; this new per-session path reproduces it.

### F09 — LOW: Native terminal snapshots are not permanent

**Invariant:** Native session comments distinguish CLOSED and INVALIDATED and state they do not transition into one another. Public Kotlin terminal state is separately latched.

**Location:** R:653–659 and R:593–595, bounded tombstone clearing; R:613–622, unknown → INVALIDATED fallback.

**Counterexample/reproduction:** Retain a native CLOSED handle, then close 128 additional sessions in the same Shared. At 129 tombstones the map is cleared. The retained handle's snapshot becomes INVALIDATED. `audit_native_closed_snapshot_must_remain_closed_after_churn` fails.

**Existing coverage / detection:** Close-idempotence checks only short histories. Existing tests do **not** detect the churn boundary. **The Kotlin wrapper remains CLOSED**, so this is an internal/native semantic inconsistency, not evidence of public wrapper resurrection. Bounded tombstones themselves are appropriate for memory safety; the snapshot contract must acknowledge the consequence or use a compatible terminal representation. Confidence: **high**.

### F10 — DOCS: No coherent explicit amendment resolves contract drift

**Invariant:** Implementation must follow the primary frozen contract, or explicit amendments must make the new contract coherent.

| Topic | Freeze | Implementation/report | Audit disposition |
|---|---|---|---|
| Creation signature | F:127–129: `createIsolationSession(timeout: Duration = 30.seconds)` | K:486 has no timeout parameter; I records no amendment | Unamended source/API mismatch; callers written to the freeze cannot compile |
| Session IDs | F:505–509: random Rust UUID | R:52–56: process-wide AtomicU64 formatted as `sess-` plus hexadecimal | No practical uniqueness failure found, but a counter is not the frozen UUID scheme |
| Cap | F:522–526: internal, chosen from target measurement | R:33–43: 32 justified by illustrative FD arithmetic | Internal/non-ABI placement correct; measurement evidence absent (F11) |
| ERROR definition | F:306–308 says immediate INVALIDATED; F §6 says PAUSED until subsequent shutdown/start | R retains PAUSED identity; I says ERROR-to-PAUSED preserved | Contradiction exists inside the normative source; no explicit coherent amendment |
| ERROR recovery start | Matrix says subsequent start invalidates | Same-config retained-root branch resumes sessions | Behavioral mismatch F06 |
| Registry re-entry | F §4 says callbacks never synchronously re-enter registry | Publication tests intentionally close/pause inside ACTIVE callbacks | Supported behavior should be stated consistently with the lock-free callback policy |
| Rebind retry | F §5 says next resume; implementation comment requires normal pause/resume cycle | Kotlin resume while already ready is a no-op | I candidly narrows retry to pause/resume or close/recreate, but primary wording remains inconsistent |

No amendment/revision entry was found in F resolving the signature, counter, or ERROR conflicts. Its R1–R10 references concern remediation of the earlier capability audit, not authorization for these implementation choices. Source comments and implementation reports do not silently amend a document explicitly declared normative.

Phase 2–4 fields/error kinds being absent is **NOT A BUG** in Phase 1; F §11 explicitly phases those additions later. Root/default traffic remains separate from sessions as required. Pause-during-bootstrap OFF semantics are consistent with the matrix and preserved tests.

**Existing coverage / detection:** No freeze-versus-API compatibility test or doc-consistency check covers these deviations. Confidence: **high**.

### F11 — DOCS: Cap evidence is arithmetic rather than measurement

**Invariant:** F §10 requires choosing the internal cap from target FD/memory measurements; the number itself must not become public API.

**Location:** R:33–43; F:516–530; I's limits and verification sections.

The constant is private and not exposed through Kotlin, UniFFI, or config. Concurrent RUNNING and PAUSED admission is genuinely atomic under the registry/engine locks. Tests prove rejection leaves the engine usable and that closing frees a slot.

However, no measured incremental memory, FD count, task cost, circuit pressure, or target process headroom is supplied. Source comments cite approximate platform FD ceilings without platform/version/process measurements. “One idle listener per ACTIVE session” does not bound accepted connections or circuit pressure; PAUSED sessions also have no listener FD. No concrete safety defect follows merely from choosing 32. This is **documentation/evidence debt, not a cap race**.

**Existing coverage / detection:** The cap tests check admission, not resource budgets. Confidence: **high** that reviewed evidence contains no measurement; **unknown** whether measurements exist outside the supplied repository/report.

### F12 — MEDIUM: Isolation integration test does not guard dispatch identity

**Invariant:** F §12 asks for actual session-client isolation and per-connection evidence that each accepted socket uses that session's client.

**Locations:** R:2446–2521, isolation tests; R:2552–2564, create-running registry check; R:1897–1928, actual dispatch.

`isolated_client_handles_are_distinct` proves different Arc objects, not their owner-token semantics. Token compatibility tests construct unrelated fresh synthetic owner tokens; they do not extract/use the owner identities of the wrapper's actual isolated clients. Dispatch probes count dispatches and read the greeting; they do not record the selected client identity.

A mutation selecting the root client at R:1910 instead of `entry.isolated` could satisfy the current dispatch counters, registry checks, greetings, and model-token tests. That mutation was **not executed in this audit**; this is the specific missing assertion, not a fabricated failed test result.

The production path itself is correct by inspection: exactly one `root.isolated_client()` at R:1239; R:1910 clones the corresponding entry's isolated client; `handle_socks` invokes plain `client.connect`. Installed `arti-client` 0.46.0 source `client.rs:1466–1472` explicitly assigns a fresh owner token and shares ClientShared. No `new_isolation_group`, `isolate_every_stream`, or raw isolation-token production API was introduced.

**Existing coverage / detection:** Current tests support the upstream conjunction model but would not reliably detect selecting the wrong client during wrapper dispatch. Live smoke verifies traffic and lifecycle, not circuit ownership. Confidence: **high** for the coverage gap and correct inspected implementation.

### F13 — MEDIUM: Shared SOCKS handler retains legacy framing defects

**Invariant:** Valid hostname/IPv4/IPv6 CONNECT requests must survive TCP fragmentation; malformed/auth-unsupported handshakes must not be accepted as valid no-auth negotiation.

**Location:** R:1973–2013, parser; R:2023–2036, reads/handshake. Compared with the same handler at HEAD, approximately lines 820–909.

**Counterexamples:**

- Valid hostname `a:80` is an 8-byte request `[5,1,0,3,1,97,0,80]`. The unconditional `n < 10` check rejects it. A two-byte hostname is also rejected.
- Greeting arriving one byte at a time fails on the first read. Request headers/bodies arriving in separate TCP reads are rejected rather than assembled with exact-length reads.
- The greeting checks only length ≥2, not version, declared method count, completeness, or whether no-auth was offered. It responds `[5,0]` even to an unsupported authentication offer. Reserved request-byte/hostname validity checks are also incomplete.

**Reproduction:** `audit_socks_valid_short_hostname_request_is_accepted` fails. The other framing problems follow directly from one `read` per message; no fragmented-wire probe was executed here.

**Regression classification:** These behaviors already existed at HEAD. Extraction into `parse_socks_request` preserves the previous root branch ordering, errors, replies, target forms, and connect/copy handling. This audit found **no session-refactoring regression in the parser**. These are inherited SOCKS defects now shared by session listeners, not a reason to claim the refactor newly broke root behavior.

Maximum hostname length 255 fits the 512-byte buffer (262 total request bytes). Complete IPv4, normal hostname, and IPv6 examples are tested; unsupported CONNECT alternatives get 0x07 on sufficiently long requests and unknown address type gets 0x08. Buffer-bound checks in the pure parser prevent the inspected indexing overruns. There is no local DNS-resolution call in the dispatch path; targets go to Arti.

**Existing coverage / detection:** Parser tests supply complete normal messages; wire test sends one complete greeting and BIND request. They do **not** detect short valid hostnames, fragmentation, or unsupported greeting methods. Confidence: **high** for inherited source behavior.

### F14 — LOW: Error-order test does not test its named order

**Invariant:** Typed `on_error` must precede matching `on_status(ERROR)`.

**Location:** R:2326–2346, `notify_error_delivers_typed_detail_before_status`, and `Recorder`'s separate vectors at R:2221–2243.

The body asserts one error with the right kind and one ERROR status with the right percentage/port, stored in **different arrays**. Reversing the two callback calls would leave all those assertions true. This is useful payload coverage but decorative evidence for *ordering*. Production currently invokes typed error first, as source inspection confirms.

**Existing coverage / detection:** This named test would **not** detect reversal. Confidence: **high**. A shared event sequence would be required to establish the claimed order; no test was changed here.

## 3. Ownership and resource audit

**NOT A BUG — exported native session ownership.** R:809–813 is exactly `{ id, generation, Weak<Shared> }`. There is no strong TorClient, runtime, listener, task, or replacement-engine pointer in `SocksSession`.

Strong ownership actually appears in these places:

| Owner/capture | Strong resources | Release behavior |
|---|---|---|
| `Inner` | Owned Tokio Runtime, worker JoinHandle, Arc<Shared> | Shutdown takes runtime/worker, aborts worker, replaces Shared |
| `Shared.client` | Root Arc<TorClient> | Cleared on shutdown/rebuild |
| `Shared.sessions` entries | Isolated clients, session callback refs, listener/connection JoinHandles and shutdown sender | Close removes one; shutdown/rebuild drains; pause retains isolated identity but aborts tasks |
| Cold/root worker | Root Arc and Arc<Shared> | Worker abort / runtime shutdown; cleanup is asynchronous |
| Initial/rebind blocked listener task | Socket, Arc<Shared>, callback Arc, session id/generation | Abort on failed admission/teardown; does not capture isolated client |
| Session accepted-connection task | That entry's isolated client; Weak<Shared> for error log | Tracked in entry and aborted on close/pause/shutdown |
| Root connection task | Root client and Arc<Shared> for log | Tracked in Shared.connections and aborted on pause/shutdown |
| `LOG_SINK` | Root StatusListener callback only | Shutdown clears only if pointer still identifies this instance's listener |

Thus “engine owns all strong resources” is accurate as **lifecycle responsibility**, not as a claim that only registry fields ever contain strong Arcs. Async tasks and in-flight calls necessarily hold clones. R:2690–2753 does more than assert an empty registry: Weak probes of the actual root and isolated clients must stop upgrading while native session handles remain retained. The independently rerun test passes. Runtime is removed from Inner and `shutdown_background()` is invoked; that is not a direct weak probe of the Tokio/Arti executor internals.

Kotlin wrappers retain `nativeSession` **and the owning ArtiTorClient** (K:191–199). Retaining a wrapper can therefore retain the facade/native engine object, but explicit shutdown empties its old resources. If the retained facade is intentionally restarted, it naturally owns the new engine resources. That is not old Weak-session attachment to the new Shared.

JoinHandle abort and Tokio background shutdown schedule asynchronous cleanup. The tests wait for root/client Weak probes and old-port refusal. They establish eventual release independent of retained session handles, not that every FD/runtime worker has physically disappeared at the instant shutdown/pause returns. In-flight create calls can also retain their root snapshot until they return. F04 is the distinct defect where old-client derivation is incorrectly registered after replacement teardown.

No hidden strong-client ownership in exported `SocksSession` was found. Confidence: **high** for its fields and teardown independence, **moderate** for the strongest literal “all runtime internals gone synchronously” wording, which is not what these tests measure.

## 4. Generation and stale handles

**NOT A BUG — shutdown/cold-start stale handle barrier.** Shutdown replaces Shared and increments generation (wrapping and avoiding zero). Handles retain only Weak references to the old Shared. With the old Shared artificially retained by a test, lookups find no live entries; without it, Weak upgrade fails. They cannot upgrade to a replacement Shared.

The global process counter avoids practical ID reuse across engines and shutdowns; it is unrelated to Tor identity. As a finite u64, its literal “never reused” statement has an overflow boundary, not a credible mobile-lifetime counterexample. No reuse-driven safety finding is made. UUID drift is F10.

`close_session_handle` checks the handle generation against the entry before removal. The fabricated same-id/wrong-generation test proves a stale close cannot remove the live entry. After shutdown and revival, the old session remains invalidated and cannot affect the new session. These tests pass.

Snapshot tombstone lookup is keyed only by ID once no live entry is found; generation is not checked against Shared there. Under actual process-unique IDs and a Weak tied to one Shared this does not attach a real old handle to a new session. Fabricated wrong-generation handles could see a terminal tombstone, but not a new live entry. F09 addresses terminal-history loss; F04 addresses native rebuild's missing replacement barrier.

## 5. Native lock graph

The normal production wrapper-lock graph is acyclic. This conclusion comes from lexical guard scopes, not from comments or test success.

| Held lock | Nested acquired lock | Sites / reason |
|---|---|---|
| `ArtiTor.inner` | `Shared.client` | `has_client`, start/resume predicates and root snapshot, setters during pause/shutdown/rebuild, spawn helpers |
| `ArtiTor.inner` | `Shared.listener` | start/resume listener installation, pause/shutdown callback-reference snapshots |
| `ArtiTor.inner` | `LOG_SINK` | listener installation and shutdown's conditional sink clear |
| `ArtiTor.inner` | `Shared.engine_state` | create's state reads; pause state assignments |
| `ArtiTor.inner` | `Shared.socks_shutdown` | pause/shutdown signal |
| `ArtiTor.inner` | `Shared.connections` | pause/shutdown/rebuild abort |
| `ArtiTor.inner` | `Shared.sessions` | create admission, list, demotion/invalidation |
| `ArtiTor.inner` → `Shared.sessions` | `Shared.tombstones` | invalidation helper at R:578–579 |
| `Shared.sessions` | `Shared.engine_state` | session dispatch predicate at R:1897–1906 |

A compatible order is:

```text
ArtiTor.inner
  before Shared.sessions
    before Shared.engine_state and Shared.tombstones
  before Shared.client / Shared.listener / Shared.socks_shutdown /
         Shared.connections / LOG_SINK
```

The sibling locks in the last line have no normal nested dependencies requiring an order among them. `Shared.set_listener`'s listener assignment ends at its semicolon before the LOG_SINK assignment. `report` releases engine_state before it acquires/releases listener and invokes `on_status`. `session_snapshot_for` releases its sessions guard when the `if let` scope ends before its tombstone lookup. Close releases sessions before acquiring tombstones. No listed lock is held across await.

**No ordinary production A→B / B→A inversion was found.** In particular, rebind's apparent `engine_state → sessions` is not a nested edge: `engine_state()` returns a copied enum and releases its guard before the sessions lock. That lack of atomicity causes F03, not a lock inversion. There is no production sessions→inner edge.

This graph excludes upstream Arti/Tokio internal locks and arbitrary foreign callback implementation locks. Panic-hook/tracing considerations below also mean it must not be advertised as a universal proof that *every exceptional callback* is lock-free.

## 6. Complete production callback-site inventory

The following lists every explicit production invocation of the four callback methods in R (excluding trait declarations, comments, and `cfg(test)` implementations). “None” means no listed wrapper mutex guard is lexically held by the invoking thread at that direct call site.

| Callback | Exact line(s) | Guard assessment |
|---|---|---|
| `on_status` | 468 | None: report's engine_state assignment and listener clone have completed |
| `on_status` | 1111, 1120 | None: the pause outcome block has ended and inner is dropped |
| `on_status` | 1162 | None: shutdown's mutation block, including LOG_SINK guard, has ended |
| `on_error` | 519 | None: `listener()` returns a cloned Arc |
| `on_session_status` | 532 | None: ERROR demotion helper has returned |
| `on_session_status` | 662 | None: close's sessions and tombstones blocks have ended; entry dropped |
| `on_session_status` | 971, 1009 | None: explicit `drop(inner)` precedes pending invalidation dispatch, including synchronous spawn errors |
| `on_session_status` | 1108, 1117, 1159 | None: pause/shutdown mutation blocks have ended |
| `on_session_status` | 1415, 1423 | None: create outcome block has ended |
| `on_session_status` | 1821 | None: rebind commit block has ended |
| `on_session_status` | 1883 | None: conversion-error registry mutation block and listener clone have ended |
| `on_log` | 100 | LOG_SINK guard released at line 90; tracing executes synchronously on the event emitter's stack |
| `on_log` | 122 | LOG_SINK guard released at line 120; panic hook executes before unwinding outer guards |
| `on_log` | 529 | None: listener clone complete, ERROR demotion complete |
| `on_log` | 1112, 1121 | None: pause mutation block ended |
| `on_log` | 1569, 1595, 1605, 1636, 1646 | None at cold-worker explicit sites; client setter itself completed |
| `on_log` | 1673, 1688, 1703, 1710 | None at root-loop / root-handler explicit sites |
| `on_log` | 1753, 1761, 1770 | None: candidate snapshot lock released before bind/rebind errors |
| `on_log` | 1881, 1917, 1943 | None: conversion mutation released; connection error runs in its spawned task; accept-error branch has no registry guard |

**Exceptional-path limit:** Cloning LOG_SINK before callback fixes its immediate lock retention. It cannot release mutex guards held by an upstream tracing emitter or a panicking caller. A panic while `ArtiTor.inner` is held invokes the hook before that guard unwinds; `on_log` re-entering `has_client` would try to reacquire inner. The absolute “NO callback under any lock” assertion is therefore not established for panic paths. No natural production panic or upstream tracing event under a wrapper guard was reproduced in this audit; this is an explicit limit, not a claimed routine deadlock finding.

Similarly, spawning while holding sessions can allow another worker to emit a callback concurrently while the first worker still holds that lock. This is not same-thread callback-under-guard nesting. The spawned error-log path itself acquires/releases its listener reference before invoking foreign code. The existing re-entry tests cover ordinary direct calls, not all panic/destructor/tracing paths.

## 7. Creation and resume/rebind stage audit

| Stage | Actual behavior | Verdict |
|---|---|---|
| Snapshot | inner locked; Shared, generation, root, runtime Handle cloned; RUNNING/PAUSED checked | Correct shutdown snapshot barrier; rebuild limitation F04 |
| Derive | exactly one root.isolated_client outside engine lock | Correct primitive |
| Bind | synchronous ephemeral std loopback socket, nonblocking; PAUSED skips binding | Correct ordinary failure cleanup; not an asynchronously enforced timeout |
| Spawn | listener closure retains socket/Shared/callback; waits oneshot | Barrier exists |
| Test gate | existing gate before final validation | Covers pause/shutdown/cap precommit schedules |
| Final check | Shared pointer + generation + copied engine state, then registry mutex | Atomic cap; ERROR check/use race F03; no root replacement check F04 |
| Registry insert | ACTIVE/port or PAUSED/null pair committed under sessions | Pair itself atomic |
| Publication | callback after lock release | Re-entry safe in ordinary paths; not ordered against other transitions F02 |
| Release accept barrier | only after ACTIVE callback returns | Correct fail-closed initial publication ordering |
| Return handle | id/generation/Weak only | Correct native ownership |

The four existing ACTIVE callback close/pause tests do force committed ACTIVE → callback mutation → barrier release, and deliberately retain the runnable listener so task abortion cannot explain zero dispatch. Both creation and rebind use the registry-selected isolated client and require ACTIVE plus native engine RUNNING before dispatch. I's barrier/dispatch recovery claims are supported for those exact schedules.

Resume root readiness is genuinely ordered: `run_socks` binds, calls `report(Running)`, then sends ready (R:1675–1679); `wait_root_then_rebind_sessions` starts rebind only if ready receives success. Root bind failure drops the ready sender and enters typed ERROR; the real occupied-port test proves no session ACTIVE after that failure. Session bind/nonblocking/local-address failures log and continue without poisoning the engine.

However, ready is only a signal that root RUNNING publication completed **at that moment**. It is not a lifecycle revision or cancellation epoch. Rebind is detached from `Inner.worker`; it rechecks a copied state, leading to F03. A shutdown removes entries and prevents ordinary old candidates from finding them, but pause retains exactly the PAUSED entries a stale rebind accepts. There is no deterministic existing optional-session bind-failure test. Creation first snapshotted PAUSED and overtaken by resume can also register PAUSED after resume's candidate collection; another cycle may be needed. That schedule was not reproduced here and is not elevated to a separate finding.

## 8. ERROR and replacement-path audit

Production `notify_error` callers are exactly the error continuations in `spawn_socks` (R:1485) and `spawn_cold` (R:1530). Cold config/client/bootstrap failures reach the latter. No separate production direct assignment to ERROR bypassing the helper was found. Test-only assignments to ERROR are not production error paths.

The helper does retain isolation clients and demote endpoints eventually. Its ordering defect is F05 and its admission race is F03. Typed async errors still precede corresponding ERROR status *in source*. All underlying connect failures continue to use the existing SOCKS 0x05 mapping; this is not a new error taxonomy.

Both replacement branches now save `spawn_cold`'s Result, release inner, deliver collected INVALIDATED callbacks, and return the saved Result. Thus runtime-creation or injected synchronous setup failure does not skip already-collected notifications. `cold_start` config construction happens asynchronously inside the worker, after this collection/dispatch path; its `?` errors go through typed ERROR notification. Existing registered-session synchronous-failure tests pass independently.

That repaired early-return problem is **NOT A BUG** in the current tree. It does not resolve old uncommitted creates crossing replacement (F04), nor make same-config retained-root ERROR recovery select the cold branch (F06).

## 9. Kotlin terminal states and snapshot API

**NOT A BUG — wrapper terminal latch.** `TorIsolationSessionImpl.acceptStatus` uses atomic StateFlow update and preserves any current CLOSED/INVALIDATED. `isClosed` is exactly CLOSED, not INVALIDATED. A CLOSED wrapper stays CLOSED across engine shutdown. The two current snapshot-interval tests and the independent stale-callback probe establish that value protection.

For initialization, the current code re-reads pending status after wrapper registration and calls `wrapper.acceptStatus(finalStatus)`. Terminal callback during older snapshot read therefore wins over returned older ACTIVE. Terminal after final reconciliation finds the registered wrapper and updates/removes it; its latch prevents later stale ACTIVE overwriting the terminal value. The gap is cleanup of initial-terminal and later unowned callbacks, F07, not loss of the existing final reconciliation repair.

**NOT A BUG — public sessions snapshot shape.** K:279–280 reads only `_sessionWrappers.value` and wrapper status values, filters ACTIVE/PAUSED, and returns a List. It has no FFI getter call, blocking mutex, coroutine wait, JVM-only primitive, root endpoint, or wrapper recreation. It preserves canonical wrapper identity while registered. As a snapshot over several mutable flows, concurrent transitions can race observation; it is not a transactional engine/whole-list state view. No Flow conversion is required by the freeze and none is recommended here.

The root convenience getter directly maps the current status port. With the actual native callback invariant it agrees with TorStatus; it does not independently enforce RUNNING for a malformed status injection. No valid native callback path intentionally supplies a port in non-RUNNING state, so no separate getter defect is claimed.

## 10. Test-quality audit of the specified names

All named test bodies below were read. “Detects” is a counterfactual source assessment unless an independently executed mutation is explicitly stated. Historical mutation outcomes in I are not relabeled as executions performed by this reviewer.

| Test(s) | Forced schedule / determinism | What the assertions prove | Broken behavior detected / remaining gap |
|---|---|---|---|
| `create_vs_pause_race_deterministic` | std thread waits at bound/precommit gate; independent thread pauses; release | PAUSED/null, one PAUSED event, one registry entry | Detects committing original RUNNING snapshot; misses F03 final-read race and F02 delayed publication |
| `create_vs_shutdown_race_deterministic` | same gate; shutdown changes Shared/generation before release | exact NotRunning, no events, empty registry | Detects missing shutdown generation validation; does not cover same-Shared rebuild |
| `concurrent_create_at_max_limit_is_atomic` | all 40 OS threads reach gate before any commit | 32 successes, 8 exact Runtime limit errors, 32 ACTIVE entries | Real competing final admissions; would catch removing final cap check |
| `concurrent_create_paused_at_max_limit_is_atomic` | same 40-thread gate after pause | 32 PAUSED entries and exact rejection counts | Real PAUSED admission check; not merely serial coroutine calls |
| `callback_reentry_does_not_deadlock` | real ACTIVE then CLOSED callback synchronously invokes engine getters | calls return and create gives ACTIVE | Detects callbacks under inner in those sites; no forced competing publication |
| `callback_reentry_pause_does_not_deadlock` | installed engine listener re-enters getters in actual PAUSED and pause-log callbacks | exact PAUSED history, expected log, PAUSED session | Detects ordinary pause callback under inner; not ERROR re-entry or panic logging |
| `callback_reentry_shutdown_does_not_deadlock` | installed listener re-enters getters in OFF callback | shutdown returns, OFF history, INVALIDATED session | Detects OFF-under-inner regression |
| `session_paused_callback_reentry_does_not_deadlock` | live session listener re-enters getters on pause | final event PAUSED/null, snapshot PAUSED | Detects direct session pause-under-inner callback |
| `session_invalidated_callback_reentry_does_not_deadlock` | same for shutdown invalidation | final INVALIDATED/null and terminal snapshot | Detects direct invalidation-under-inner callback |
| `session_listener_does_not_dispatch_before_active_commit` | precommit socket exposed; full greeting queued; actual polled oneshot receiver inspected | empty barrier, zero dispatches/registry/events before release; actual greeting and dispatch after | Detects early release and disabled-listener false positives; does not record which TorClient serves the handler |
| four `create/rebind_active_callback_close/pause_precedes_barrier_and_rejects_dispatch` tests | callback checks committed entry/pending barrier, queues traffic, removes task/shutdown sender from registry, then closes/pauses | exact histories, actual accept checked, zero dispatch despite runnable listener | Strong tests for release ordering and live dispatch predicate; do not cover publication delayed *before* application of ACTIVE |
| `resume_root_bind_failure_never_activates_sessions` | real root listener chooses port; joined abort; external bind owns exact configured port; actual resume worker joined | typed Bind with that port, ERROR, no later ACTIVE, all null endpoints | Strong root-failure gate; stale root-port mutation would fail; optional session failure untested |
| `rebuild_spawn_cold_failure_still_notifies_invalidated` | registered session paused; changed dataDir; function-entry synchronous failure injection | original Runtime returned, one INVALIDATED callback, empty registry | Detects former `spawn_cold(...)?` early return losing pending notifications; not uncommitted create crossing rebuild |
| `cold_error_recovery_spawn_cold_failure_still_notifies_invalidated` | same injection after explicitly removing root/bootstrap and modeling ERROR | same result/callback/registry assertions | Detects second early-return branch; fails to represent retained-root ERROR recovery |
| `unknown_and_stale_handles_are_noop_invalidated` | fabricated unknown, wrong-generation/same-ID, and dead-Weak handles | snapshots invalidated, live entry untouched | Good direct stale-close coverage; no competing work required |
| `notify_error_delivers_typed_detail_before_status` | sequential helper invocation | typed payload and ERROR payload | Cannot detect reversed order: separate vectors (F14) |
| KT `callbackBeforeWrapperRegistrationIsReconciled` | fake create emits ACTIVE synchronously before return | ACTIVE returned and canonical wrapper listed | Detects lost early ACTIVE; not a terminal race |
| KT `invalidatedCallbackDuringOlderActiveSnapshotIsReconciledAndLatched` and CLOSED counterpart | first native snapshot itself emits terminal and returns older ACTIVE | one read; terminal wrapper, no endpoint/list membership, pending consumed; stale events do not change wrapper | Strong final reconciliation tests; omission mutant should fail; terminal-at-initial-read and post-stale-callback cleanup absent |
| KL `asyncBootstrapFailureSurfacesTypedBootstrap` | fake schedules typed async error then ERROR after delay | public Bootstrap failure and typed lastError | Useful error routing, not deterministic scheduling coverage of native ERROR demotion |
| KL `syncConfigFailureStaysConfig` | fake native call immediately throws typed Config | mapped public Config | Detects missing sync mapping for start; says nothing about unmapped session call |
| KL `concurrentDoubleStartJoinsInflightWork` | coroutine A starts; delayed B queues behind lifecycleMutex; first fake bootstrap completes after delay | both callers succeed and client ready | Exercises facade serialization; native AlreadyRunning branch need not run because B can see already-ready status; no call-count assertion proves native in-flight join |

The native re-entry and gate tests generally hang rather than fail with a bounded timeout if a mutex-deadlock mutation is introduced. Their forced schedules are meaningful, but CI should not mistake an unbounded hang for a diagnostic assertion. Existing socket-close tests use bounded polling; they prove eventual refusal, not exact synchronous FD disposal. `close_aborts_only_own_connections` plants pending tasks and checks sibling/root survival after a sleep; it does not directly await/assert A's receiver cancellation. `abort_connections_drains_tracked_tasks` proves registry draining, not independently observed task completion. Strong ownership Weak probes provide separate resource-release evidence.

No test-count-based inference is used here. In particular, 52 passing native tests are compatible with every reproduced F01–F07 failure.

## 11. Mutation-evidence audit

The existing disposable scripts `/private/tmp/artitor_recovery_mutant.py`, `/private/tmp/artitor_kotlin_mutant.py`, and their Gradle source-routing init script were inspected. Their substitutions correspond to the claims in I:

| Mutation in I | Correspondence to defect | Assessment |
|---|---|---|
| Move creation/rebind barrier send before ACTIVE callback | Removes publication-before-dispatch ordering in both paths | Exact substitution matches the test's pending-receiver assertion |
| Remove session ACTIVE and engine RUNNING dispatch checks | Makes callback pause permit handler dispatch | Exact conjunction is removed; retained-listener technique prevents abort from masking it |
| Restore two `spawn_cold(...)?` returns | Skips collected invalidation callbacks on synchronous failure | Correctly models both historical notification-loss paths |
| Remove final `wrapper.acceptStatus(finalStatus)` in copied commonMain | Returns stale ACTIVE wrapper after terminal callback during snapshot | Corresponds exactly to the two terminal-interval tests |

The scripts establish that these are concrete mutations, not prose-only descriptions. **Their historical claimed failing outcomes were not independently rerun in this audit.** Existing tests were independently run on the current source, and their bodies are compatible with the stated mutation failures. I explicitly excludes the initial failed-compilation Kotlin harness and acknowledges shared-target cached-mutant contamination; that candor is appropriate. This audit uses separate package identity for its native probes and verifies the original binary afterward.

These four mutation classes do not prove publication serialization, atomic engine-state/registry admission, same-Shared rebuild safety, exception mapping, or bounded Kotlin pending memory. I's exact-schedule table mostly states its scope carefully; treating it as exhaustive Phase 1 concurrency proof would overstate the evidence.

## 12. 0.2 regression, KMP, and scope checks

| Requirement | Audit result |
|---|---|
| Pause during STARTING/BOOTSTRAPPING → OFF | Typed CancelledBootstrap branch discards partial client and aborts worker; current facade cancellation test passes. No new reversal found. Abort is asynchronous; no exhaustive mid-worker instruction-preemption proof is claimed. |
| Typed async errors | Existing discriminants/detail mapping retained; native source ordering is typed-first; common adversarial-message tests pass. Session synchronous mapping is the new gap F01. |
| `restart(null)` | Kotlin retains last successful config across facade shutdown and restores it on restart; existing tests pass. Native last_config clearing on shutdown remains consistent with facade ownership. |
| Config identity | data/state/cache/bridges comparison remains exact; port is excluded; bridge nonempty→empty tests pass in both layers. |
| Root SOCKS rebind | First full simulator script passes HTTP, pause/resume, and second cold start; a later repeat returns CONNECT code 5. No clean all-runs-success claim is made. |
| Cancellation epoch | Existing epoch updates and awaitReady logic preserved by diff. Facade bootstrap/shutdown/pause tests pass. |
| Shutdown release | Native retained-session Weak-client probes pass; root/isolated registry resources removed and owned runtime taken. Strong immediate physical executor/FD-release wording exceeds the measured evidence. |
| Pause root-port publication repair | `bound_port.store(0)` is synchronous at R:1084 before publication; exact occupied-port test passes and first live resume succeeds. The later code-5 failure is not AlreadyRunning. |
| Apple SQLite mitigation | `tor/build.gradle.kts` and `rust/hide-sqlite3-symbols.sh` unchanged against HEAD. Actual iOS device/simulator Cargo build tasks run the localization script successfully. No consumer SQLCipher/device-runtime revalidation was performed. |
| Single-instance documentation | Existing process-global last-writer-wins restriction retained in K and R; no multi-instance support claimed. |
| commonMain portability | Complete production file contains none of `java.*`, `android.*`, `platform.*`, `runBlocking`, `Thread`, or `synchronized`. iOS device compilation and Android test compilation succeed. Test-only runBlocking is not a production violation. |
| Generated types internal | No generated type appears in intended public value/session signatures; actual thrown session errors leak at runtime (F01). |
| Phase boundary | No bridge tri-state, new error taxonomy, onion/timeout config, dormant, PT, hosting, RPC, live reconfigure, or Cargo feature addition introduced. Cargo.toml/build script feature configuration unchanged against HEAD. |

## 13. Independent verification results

Commands were awaited to exit. The first sandboxed Rust attempt encountered loopback failures and was interrupted when gated tests could not progress; it is **not** counted as a production regression. The suite was then run with socket permission.

| Command / run | Actual result |
|---|---|
| `rtk proxy cargo test --manifest-path rust/arti-kmp-ffi/Cargo.toml`, permitted run | Exit 0; 52 passed, 0 failed; doc tests 0 |
| Same repository Cargo command after disposable probes | Exit 0; 52 passed, 0 failed; original package binary `arti_kmp_ffi-fb5cb2f09952b576` |
| `rtk ./gradlew :tor:iosSimulatorArm64Test :tor:compileKotlinIosArm64 :tor:assembleAndroidDeviceTest`, first run | Exit 0; BUILD SUCCESSFUL in 1m 58s; XML 54 tests, no failures/errors/skips; real root HTTP 200 and SECOND COLD START output |
| Same combined command after disposable probes | Exit 1; 54 executed, 53 passed, one live E2E failure `SOCKS CONNECT failed (code=5)`; initial root HTTP 200 already obtained |
| `rtk ./gradlew :tor:iosSimulatorArm64Test --tests 'com.yet.tor.TorIosE2ETest.bootstrapFetchPauseResume'`, isolated retry | Exit 0; one checked-in live root E2E test passed; BUILD SUCCESSFUL in 1m 14s; XML 0 failures/errors/skips |
| `rtk ./gradlew :tor:compileKotlinIosArm64 :tor:assembleAndroidDeviceTest`, separate run afterward | Exit 0; BUILD SUCCESSFUL in 20s |
| Disposable native audit command below | Exit 101; 8 executed, 8 failed assertions; 52 existing tests filtered out. These failures are successful falsification evidence, not an assertion that the original suite fails. |
| Disposable Kotlin audit command below | Exit 1; 3 executed, 3 failed assertions; exact FFI exception and two 160-record memory observations in XML |
| Disposable live two-session command below | Exit 0; one live test passed, 0 failures/skips; BUILD SUCCESSFUL in 1m 20s |
| `rtk proxy /Users/yet/Library/Android/sdk/platform-tools/adb devices -l`, with socket permission | Exit 0; no devices attached |
| `rtk git diff --check` | Exit 0 during verification; final check recorded after report creation |

The repeated live root failure originates in `handle_socks`'s `client.connect` error mapping to SOCKS 0x05 (R:2057–2064). The same XML already contains the initial HTTP 200. It does not contain the underlying Arti error or an unambiguous later-stage marker. An isolated retry of the unchanged checked-in root E2E passed. **Root cause of the intermittent failure is unknown**; it must not be relabeled as an environmental-only failure or as proven Phase 1 regression. The independent multi-session run uses a distinct data directory and passed. All three observations belong in the audit record.

Reproduction commands (temporary files remain available on this host; they are not repository production changes):

```sh
rtk proxy cargo test --offline \
  --manifest-path /private/tmp/artitor-independent-audit/Cargo.toml \
  --target-dir /Users/yet/development/Multiplatform/ArtiTor/rust/arti-kmp-ffi/target \
  audit_ -- --test-threads=1 --nocapture

rtk ./gradlew --init-script /private/tmp/artitor-independent.init.gradle \
  :tor:iosSimulatorArm64Test \
  --tests 'com.yet.tor.ArtiTorSessionConcurrencyTest.audit*'

rtk ./gradlew --init-script /private/tmp/artitor-independent-live.init.gradle \
  :tor:iosSimulatorArm64Test \
  --tests 'com.yet.tor.TorIosE2ETest.auditLiveTwoSessionsLifecycle'
```

Temporary native source: `/private/tmp/artitor-independent-audit/src/lib.rs`; Kotlin common-test source: `/private/tmp/artitor-independent-kotlin-tests`; live test source: `/private/tmp/artitor-independent-ios-tests`. Saved XML evidence is under `/private/tmp/artitor-independent-audit/kotlin-probe-results`, `live-session-results`, and `repeat-baseline-failure-results`. The initial scratch directory called `baseline-results` contains overwritten probe output and is **not** baseline evidence; the actual first-run baseline is supported by the completed command output and XML read before source rerouting.

The live independent test establishes: distinct root/A/B loopback ports; HTTP traffic through A and B; closing A leaves B and root usable; pause clears B; resume restores B traffic; shutdown invalidates B while CLOSED A remains CLOSED; cold start does not resurrect B; closing old B does not close a newly created session. It does **not** measure actual circuit IDs, require different exits, or prove unlinkability. Existing checked-in E2E is root-only; this new test remains a disposable audit probe, not an installed regression test.

**Android runtime: NOT VERIFIED**

**Live multi-session Tor traffic: VERIFIED** — iOS simulator only, one independent live lifecycle/traffic run. Circuit-ownership guarantee rests on the inspected upstream isolation construction and selected-client dispatch, not exit-IP comparison.

## 14. Claims I attempted to falsify but could not

The following positive conclusions are deliberately scoped to the inspected implementation and executed schedules:

1. **NOT A BUG:** Native SocksSession itself has no strong Tor resource field. Retained session handles do not explain old root/isolated client retention after explicit shutdown; actual Weak probes pass.
2. **NOT A BUG:** Shutdown changes Shared/generation and old Weak handles cannot attach to its replacement. Unknown/wrong-generation closes cannot remove a live entry in the tested cases.
3. **NOT A BUG:** Session derivation uses exactly `root.isolated_client()`; ordinary dispatch clones the corresponding entry's isolated client. No forbidden combination or direct root dispatch was found.
4. **NOT A BUG:** Creation/rebind's accept barrier is released only after ACTIVE callback completion. Existing polled-barrier/positive-continuation tests substantiate this; F02 is publication ordering between transitions, a different property.
5. **NOT A BUG:** Retained runnable listeners reject dispatch after ACTIVE callback close/pause through the live registry/engine-state gate. This does not repair the false ACTIVE state defects.
6. **NOT A BUG:** The final internal-cap admission is atomic for simultaneous RUNNING and PAUSED creates. Rejection preserves engine usability and closing frees a slot. Target measurement remains F11.
7. **NOT A BUG:** Root bind must succeed and root RUNNING publication must complete before normal session rebind begins. Real occupied-root-port failure prevents session activation in that test.
8. **NOT A BUG:** Already registered sessions receive INVALIDATED even when either cold replacement-start branch fails synchronously. The repaired saved-Result flow and tests agree.
9. **NOT A BUG:** Kotlin wrapper terminal states are latched, including terminal callbacks during the older snapshot interval. Pending-map memory cleanup is a separate defect.
10. **NOT A BUG:** sessions remains a List snapshot with canonical live wrappers, no root endpoint, no FFI getter or blocking primitive, and no wrapper recreation.
11. **NOT A BUG:** Pause retains isolated Arc identity in normal paths; close removes only its session's registry resources; independently tested live close/pause/resume/cold-start behavior is usable on iOS simulator.
12. **NOT A BUG:** Root parser extraction preserves the legacy root semantics. Inherited framing defects are reported separately rather than mislabeled as new regressions.
13. **NOT A BUG:** No Phase 2+ production surface, new Cargo features, platform-only commonMain primitives, or removal of the Apple SQLite build mitigation was introduced.

“Could not falsify” is not a universal formal proof. Outstanding exceptional callback paths, optional-listener error injection, Android runtime, and actual wrapper-level isolation mutation coverage remain explicitly limited above.

## 15. Recommendation

The implementation has meaningful ownership, barrier, admission, reconciliation, and recovery repairs. Those repairs coexist with reproducible public exception leakage, stale publication, non-atomic state/registry commit, replacement-epoch failure, and unbounded pending-state histories. Passing ordinary native/common tests and one live multi-session smoke do not neutralize those counterexamples. Normative contradictions also need an explicit coherent decision before any renewed completion claim.

No remediation was applied by this reviewer. No Phase 2 work was started.

**INDEPENDENT PHASE 1 AUDIT: REMEDIATION REQUIRED**


## Remediation-status addendum — 2026-09-30

The original independent findings, reproduced counterexamples, verification
history, and verdict above are preserved unchanged. Subsequent Phase-1
remediation adds public exception mapping; revisioned session publications and
bounded Kotlin creation bookkeeping; atomic lifecycle/session admission and
root-client replacement epochs; ERROR demotion/publication ordering and explicit
resume-versus-start recovery; current-listener fatal accept handling; dispatch
identity probes; exact shared SOCKS framing; and an ordered error-event test.
The freeze now resolves F09–F11 honestly: bounded native diagnostic tombstones
with permanent public terminal latches, coherent normative API/lifecycle
semantics, and an unmeasured private cap gated on Phase-5 device/resource audit.

Implementation-team completed evidence: native final suite **70 passed**;
combined simulator/device-compilation/Android-test-assembly command **66 tests,
0 failures/errors/skips** on its second full run, including checked-in live root
and two-session lifecycle tests. The earlier full run passed 65 tests. Epoch,
stale-revision, root-dispatch, and reversed error-order mutations fail their
regressions. Removing transition locking fails explicit final-transaction
ownership assertions (structural atomicity evidence, not a claimed stale-state
mutation reproduction). Detailed commands, assertions, and limitations are in
`ARTITOR_0_3_PHASE1_IMPLEMENTATION_REPORT.md`. The third targeted live repeat
also passed both root and two-session tests (2 tests, 0 failures/errors/skips;
BUILD SUCCESSFUL in 2m 45s). All three post-remediation live attempts completed
without observed failures; no safe SOCKS code-5 diagnostic occurred. The final
F04 targeted rerun additionally verifies empty registry, hasClient false, no
ACTIVE/PAUSED callback, and unchanged engine generation after failed rebuild.

This is implementation-team remediation evidence, not an independently rerun
acceptance verdict. The original one-off SOCKS code-5 cause remains unknown;
new safe diagnostics omit target/upstream display text. Android hardware runtime
and Phase-5 resource measurements remain release gates. No Phase 2 work began.

**REMEDIATION IMPLEMENTED — READY FOR INDEPENDENT RE-AUDIT**


---

## Independent Phase-1 re-audit — 2026-09-30

This section is the independent acceptance review, separate from the implementation-team remediation addendum above. The original findings, counterexamples, execution history, and original verdict are preserved. The remediation report was not used as proof of closure. Production was not modified, findings were not fixed, and Phase 2 was not started.

**The original HIGH counterexamples are repaired, but acceptance still requires remediation.** The replacement session `StateFlow` fails the supported `onSubscription` contract, a legal shutdown from the error callback is followed by stale ERROR publication, and a contradictory session-port promise remains in the freeze. The callback freshness defect is newly reproduced but has an antecedent in pre-remediation code; it is distinguished from new regressions below. Confidence: **high** for the reproduced failures and mutation-sensitive repairs; **moderate** for concurrency coverage beyond the executed schedules; **unknown** for the historical intermittent Tor CONNECT failure's cause.

### A. Immutable baseline, interruption, and evidence provenance

Before testing, `git rev-parse HEAD` returned exactly `0029ddefc17f4003f7a20937f7286288ec073b6d`. `git status --short` contained only the pre-existing untracked `docs/audit/ARTITOR_0_3_CAPABILITY_AUDIT.md`. No additional production modifications existed. Initial, post-probe, and resumed checks agree on these SHA-256 hashes:

```text
rust/arti-kmp-ffi/src/lib.rs
242cec889404cc841827ead0beee387e46979b9188d2e55f863023f727615e92
rust/arti-kmp-ffi/src/socks.rs
735324801f82a7e53f76a50f71256ef1e08e77d4198890882e442ad4336b521a
tor/src/commonMain/kotlin/com/yet/tor/ArtiTorClient.kt
a161c0a81bce0898a0cf2e4e4581246397323b3ace57312b76b6e1a09f001a52
```

The original disposable probes were present at the start and were reused before adding the new corpus. Native copies have a different Cargo package identity; Kotlin probes/mutations route temporary source directories through Gradle init scripts. Required revision arguments and internal pending-count access were adapted, while the original counterexamples and both **160-cycle** loops were preserved. No probe or mutation was applied to repository production files.

The user resumed after an approximately three-hour interruption. During that interval `/private/tmp` was cleared and HEAD advanced to `0e4952dc2788171c9d00ea72ed1bea7847c0e776`. `git diff 0029dde HEAD --stat` shows **only** `gradle/wrapper/gradle-wrapper.properties`, changing Gradle 9.6.1 to 9.8.0. Production hashes remain identical. The earlier production audit is attributed to the expected remediation SHA; later build runs are explicitly attributed to the newer wrapper, not silently represented as executions of its predecessor.

Kotlin and SOCKS probe/mutation sources and completed command-output records were recovered from this chat and the audit agents' session transcripts. The original native eight-probe temporary source/raw logs were cleared; their completed assertions/counts were recovered, but that entire temporary harness was not reconstructed before handoff. Fresh native ERROR mutation/reentry probes are retained separately. Historical assertions/counts remain observations from actual completed executions. Recovered transcripts are not new executions, and lost historical XML is not described as currently available. The second full simulator run's XML survived in the build directory and was saved before fresh testing. Recovered/current artifacts are under `/private/tmp/artitor-reaudit-results`, `/private/tmp/artitor-reaudit-kotlin`, `/private/tmp/artitor-reaudit-socks`, `/private/tmp/artitor-reaudit-native`, and `/private/tmp/artitor-reaudit-f05`; these remain temporary, not durable CI regression infrastructure.

### B. F01–F14 dispositions

| Finding | Original severity | Current disposition | Evidence |
|---|---|---|---|
| F01 | HIGH | CLOSED | Exact generated-NotRunning probe and all six generated classes map to exact public classes; removing the catch reproduces generated-type leakage. |
| F02 | HIGH | CLOSED, publication protocol CHANGED | Native delayed ACTIVE still arrives after PAUSED, but carries an older registry revision; public wrapper remains PAUSED/null. Removing Kotlin revision comparison reproduces ACTIVE resurrection. |
| F03 | HIGH | CLOSED | Original after-read/before-commit schedules now serialize; removing transition ownership reproduces both ERROR/create and pause/rebind failures at those same boundaries. |
| F04 | HIGH | CLOSED | Original failed replacement rejects old-root create, empty registry/no live publication; successful root-install variant also rejects it. Removing epoch comparison admits the old isolated handle. |
| F05 | MEDIUM | CLOSED | Original reentrant observers see only PAUSED/null; session demotions precede typed error and engine ERROR. Mutation details below. |
| F06 | MEDIUM | CLOSED | Same-config `start` after retained-root ERROR invalidates old sessions; explicit `resume` preserves identity. Reverting ERROR replacement predicate preserves PAUSED instead and fails the original probe. |
| F07 | MEDIUM | CLOSED | Both original 160-cycle histories now leave zero live/pending records; reverted retention/cleanup produces exactly 160 pending records in each. |
| F08 | MEDIUM | CLOSED | Deterministic current-listener accept failure demotes only that session; stale failures after rebind/close/invalidation do not mutate current state. Removing listener-revision validation fails. |
| F09 | LOW | CLOSED, diagnostic contract CHANGED | Original permanent-native-CLOSED assertion still fails after pruning, now explicitly permitted. Native terminal history remains bounded; public CLOSED remains latched. |
| F10 | DOCS | OPEN / CHANGED | The original timeout/UUID/ERROR/reentry/retry mismatches are amended, but §9 still promises session-port preservation during root port-only changes, contrary to production. |
| F11 | DOCS | CLOSED | Cap 32 is explicitly conservative/private and unmeasured; Phase-5 resource measurement/reassessment is an explicit acceptance gate. Measurement remains a release follow-up. |
| F12 | MEDIUM | CLOSED | Actual root/A/B selected handles are recorded at dispatch; substituting root for session dispatch makes all three handles equal and fails the test. |
| F13 | MEDIUM | CLOSED | Exact eight-byte one-character-domain probe and actual fragmented TCP corpus pass; restoring ten-byte minimum or single greeting read makes probes fail. |
| F14 | LOW | CLOSED | One ordered sequence asserts session PAUSED events → typed error → engine ERROR; reversing production error/status order fails that exact sequence. |

`CLOSED` here identifies the original observable defect, not a claim that every old assertion turned green. F02 and F09 deliberately changed internal protocols/contracts. Their unchanged raw original assertions were run and remain **red**; the revised public guarantee and permitted diagnostic loss were tested separately. They are not silently replaced by similarly named checked-in tests.

### C. Original counterexamples, repairs, and mutation evidence

**F01 — public exception boundary.** Original counterexample: `native.createSession` throws generated `ArtiException.NotRunning`; public `Result` contains that generated class. K:519–523 now catches `FfiArtiException` and calls the existing K:641–648 public mapper. The exact original probe passes. Additional executed checks cover AlreadyRunning, NotRunning, Config, Bind (including port), Bootstrap, and Runtime. A disposable removal of that catch fails both the original NotRunning and all-class tests. All synchronous session-native calls were inspected: create has the generated typed-error surface; id, snapshot/status, and close are declared infallible. Inventing a fake typed exception from an infallible binding would not establish a production exception leak. No generated typed exception was found crossing the public Result boundary.

**F02 — revisions rather than callback-delivery serialization.** Original schedule remains unchanged: ACTIVE callback is blocked before recording; pause completes and publishes PAUSED; old ACTIVE resumes. The original native recorder still ends `[PAUSED, ACTIVE]`, so its delivery-order assertion fails. Native snapshot nevertheless remains PAUSED/null, and publication carries ACTIVE rev1 versus PAUSED rev2. K:195–198 and K:240–245 retain revision/status in one CAS, accept only strictly newer `ULong` revisions, and latch terminals. The independent public schedule stays PAUSED/null; equal revisions are ignored. ACTIVE3 → CLOSED4 → PAUSED2 → ACTIVE3 → INVALIDATED5 → ACTIVE4 stays CLOSED, because the first terminal wins. Values above signed `Long.MAX_VALUE` order correctly. Removing only the newer-revision comparison makes both stale/unsigned tests fail with PAUSED becoming ACTIVE. No signed conversion was found.

Native revision audit: ACTIVE and PAUSED creation begin at 1 (R:1505/1527); ACTIVE→PAUSED advances under registry mutation (R:618); rebind advances inside its validated transaction (R:2033); close creates terminal revision `old+1` (R:736); shutdown/rebuild invalidation advances while draining (R:651); ERROR uses the same demotion helper; fatal current-listener demotion advances under the gate/registry (R:2072). Reassigning PAUSED/null to an already PAUSED/null entry is a no-change operation, not an omitted state/endpoint transition. No actual state/endpoint change lacking a revision advance was identified. Arithmetic overflow is addressed separately below.

**F03 — exact final-read transaction schedules.** The previous inserted barriers were immediately after the lifecycle read and before registry lock/commit. They remain at those same boundaries in the disposable copies. Under the repaired gate, a competing ERROR/pause thread cannot complete while that transaction is held. Waiting synchronously for it while retaining the gate would manufacture a harness deadlock, so the revised harness observes the blocked competitor, releases admission, joins both operations, and checks the resulting demotion. It does not move the barrier to an earlier precommit point. With transition ownership removed, the same competitor completes before release: ERROR completes but create inserts ACTIVE rev1; pause completes but rebind inserts ACTIVE rev2. Both original assertions fail. This is behavioral mutation evidence, not merely a `try_lock` ownership assertion.

**F04 — root-client epoch.** Original failed-rebuild schedule: create derives isolated A and blocks; production replacement invalidates/discards A, advances epoch, then synchronously fails; release create. It returns NotRunning, registry remains empty, hasClient is false, no ACTIVE/PAUSED callback occurs, and generation may remain unchanged. The successful root-install variant retains Shared/generation but installs root B through the authoritative root setter; old A admission is rejected. This is a deterministic successful-install test, not a claim that a full live bootstrap was interleaved with blocked create. Removing epoch admission comparison fails the successful-install assertion. Source inspection covers every root install/discard via `set_client_under_gate` (R:523–525), including cold installation, rebuild, ERROR recovery, bootstrap cancellation, and shutdown.

Normal pause/resume does not advance root epoch. Existing session isolated Arc identity survives; creating a sibling does not advance it; port-only rebind preserves isolation identities despite listener replacement by source inspection. The executed false-positive/revision probe covers sibling creation, retained-client pause and manual successful session rebind; checked-in identity tests also hold. A separately prepared actual resume/port-start epoch extension was launched before interruption but its result was not retrieved, so it is not counted as executed evidence. Epoch is a root-identity fence, not the engine worker's cancellation revision.

**F05 — ERROR observation.** The exact old on_error/on_status(ERROR) reentry probe now reads PAUSED/null at both observation points. R:570–603 commits native ERROR, clears root endpoint, and demotes/aborts sessions under one gate before any callback. Pending session demotions are delivered first, then typed error, then matching engine ERROR, then log. A two-session ordered extension requires `[PAUSED, PAUSED, typed, ERROR]`. All async failure routes were inspected for use of this helper; no separate production ERROR mutation/report bypass was identified. This guarantee is scoped to observing that ERROR transaction without initiating a new lifecycle transaction from its callbacks; intervening shutdown is N04 below. The demotion-removal mutation initially failed to compile because its empty vector lacked a type; that compile failure is **not** mutation-sensitivity evidence. The corrected parent mutation uses `Vec::<PendingSessionNotification>::new()` and fails the original observer assertion: both snapshots remain ACTIVE/endpoint/rev1. Restoring the byte-identical baseline passes with both PAUSED/null/rev2. Mutation exit101; restored exit0.

**F06 — explicit resume versus replacement start.** F §5 and matrix now expressly allow ERROR+resume to preserve a retained root/session identity, while ERROR+start replaces even for identical config. R:1061–1064 includes ERROR in `needs_new_client`; invalidation and epoch advance happen before replacement startup. The exact original retained-root/same-config probe passes with INVALIDATED. Reverting that predicate leaves the old session PAUSED and fails. `resume` does not call replacement startup; normal recovery tests retain root/session identity. Normative recovery is coherent; the separate port wording contradiction remains F10.

**F07 — pending lifetime.** Original terminal-before-wrapper and post-close-late-callback probes each run 160 cycles and now finish with zero live wrappers and zero pending records. Additional checked-in checks inspect cleanup after individual completed operations. K:336–342 ignores unknown callbacks outside creation; K:537 and K:546 clear the entire pending map on reconciliation and on success/failure completion. A native-create failure after unrelated callbacks leaves no orphan history. Reverting unknown retention plus removal of cleanup makes each original probe retain exactly 160 records.

A single `creating` Boolean cannot authenticate an unknown ID. The independent callback-injection probe retained one stale ID plus 1,000 arbitrary unknown IDs during an in-flight creation, then cleared everything when creation failed. This is **bounded lifetime, not a proven fixed cardinality bound**. The callback originates from trusted in-process native code; artificial arbitrary IDs do not by themselves demonstrate an externally reachable memory attack or historical leak. F07's persistent growth is closed. Claiming a hard transient bound from the Boolean alone would overstate the proof.

**F08 — current-listener fatal failure.** The deterministic fault injection enters the real current-listener failure path. R:2058–2091 validates generation, ACTIVE state, and listener's status revision together under transition/registry ownership, aborts only that session, sets PAUSED/null, and advances revision. Root remains usable/unaffected. Failure from pre-rebind listener cannot change the new ACTIVE endpoint; failure after close or invalidation cannot resurrect/demote anything. Removing the listener-revision condition reproduces stale demotion: ACTIVE rev3 becomes PAUSED rev4. The test-only injection does not redefine the production listener-identity predicate.

**F09 — bounded diagnostics.** The original churn assertion requiring native CLOSED forever still observes INVALIDATED after tombstone pruning. F §5 now states this degradation explicitly and separately preserves public terminal latching. Native tombstones clear above `MAX_SESSIONS*4` (128), rather than retaining unbounded history. Kotlin ignores all later statuses after CLOSED, including larger-revision INVALIDATED. Native churn and public terminal tests pass their amended contracts. A disposable document reversal of the pruning clause fails the contract audit; no unbounded native retention was introduced. This intentional native diagnostic loss does not reopen the original low finding.

**F10/F11 — full freeze comparison.** The complete freeze was searched/read, not just its amendment: create has no timeout parameter; UUID is expressly unnecessary; ERROR retains PAUSED identities until shutdown/start while explicit resume can recover them; callbacks may re-enter after locks release; optional-session retry is pause→resume or close/recreate, and RUNNING resume is a no-op. The private cap is described as conservative resource accounting, not empirical optimum; Phase-5 Android/iOS FD/memory/task measurement is explicit. Five disposable text reversals (creation timeout, UUID mandate, pre-demotion ERROR observers, measured-cap claim, permanent-native-CLOSED promise) fail the document predicates. These are document checks, not substitutes for runtime tests.

However, F §9:503 still says `root; sessions keep ports unless colliding`, and §9:512 calls a port change root-only rebind. K:419–422 implements port-only start by pausing; R pause demotes all sessions; R:1096–1117 expressly rebinds root **and** live PAUSED sessions with no port-stability promise; R:1931–2055 binds fresh ephemeral session sockets. Consequently callers can see ACTIVE→PAUSED/null→ACTIVE/new-port on a root port-only change even without a collision. The freeze's port preservation promise is stale and contradictory. **F10 remains OPEN at DOCS severity**; the original timeout/UUID/ERROR mismatches themselves are corrected. No document edit was made.

**F12 — actual selected handles.** The recorder sees the already-selected production client immediately before spawning the handler. Root dispatch records root; session dispatch clones `entry.isolated`. The recorder neither supplies nor changes the selected handle. The actual root/A/B test passes. Mutating session selection to the root makes all recorded pointers equal and fails the expected root/A/B vector. This supports selected-handle isolation together with `isolated_client()` construction; no inference is based on exit-IP differences.

**F13 — exact SOCKS framing and upstream compatibility.** The original request `[5,1,0,3,1,b'a',0,80]` passes. Restoring the global ten-byte minimum fails that exact original probe. The shared parser uses a four-byte header followed by per-address-type lengths. Actual loopback TCP tests write greeting/header/body byte-by-byte; `duplex(1)` tests also deterministically force individual AsyncRead boundaries. NMETHODS consumption, no-auth offered in a later slot, auth-only→05 FF, exact version/RSV, one-character and fragmented domains, IPv4/IPv6, BIND/UDP→code7, unknown ATYP→code8, and empty/invalid-UTF8/malformed domains→code1 all pass. Pipelined application bytes remain unread after successful domain/IPv4/IPv6 parsing. Changing greeting `read_exact` back to a single `read` makes the actual fragmentation probe fail. Restored independent SOCKS suite: 15 passed.

Actual `arti-client` 0.46 `IntoTorAddr` comparison, not a surrogate hostname validator:

| Target form | Upstream | Current SOCKS parser |
|---|---|---|
| Trailing-dot FQDN | rejects | accepts, later rejected upstream |
| Punycode / uppercase / numeric labels | accepts | accepts |
| 63-byte label | accepts | accepts |
| 64-byte label | rejects | rejects |
| Valid 253-byte domain | accepts | accepts |
| 255-byte encoded domain | rejects | accepts, later rejected upstream |
| Valid v3 onion / single-label `a` | accepts | accepts |

No required upstream-valid hostname was newly rejected. Text IPv6 placed in DOMAINNAME ATYP=03 is less permissively accepted than before; RFC1928 assigns IPv6 to ATYP=04 and the conforming IPv6 frame passes. This is recorded as permissive-input compatibility, not a demonstrated conforming-target defect. [RFC1928 §5](https://www.rfc-editor.org/rfc/rfc1928.html#section-5).

No local DNS call was added: host strings go to Tor connect; numeric addresses are formatted directly. Successful CONNECT and upstream failure retain the previous ten-byte replies with codes 0 and 5 respectively, with zero bound-address placeholders. The generic upstream code5 categorization is inherited behavior.

**F14 — order sensitivity.** The checked-in ordered event test passes. The independent two-session sequence also passes. Reversing production engine status and typed error yields `[PAUSED, PAUSED, ERROR, typed]` and fails the same ordered assertion. Shared failure routing makes this stronger than two separate unordered vectors, without claiming exhaustive arbitrary reentrant lifecycle schedules.

### D. NEW FINDINGS INTRODUCED BY REMEDIATION

**N01 — MEDIUM / OPEN: the custom public StateFlow skips onSubscription actions.** Location: K:223–230, specifically K:229. Production implements StateFlow directly and forwards collection through `accepted.map { it.status }.distinctUntilChanged().collect(collector)`. The independent simulator test runs a supported MutableStateFlow control, then:

```kotlin
var calls = 0
val initial = session.status.onSubscription { calls++ }.first()
assertEquals(1, calls) // actual: 0
```

Initial replay arrives; the subscription action never runs. The independent 28-test class executes 27 passes and this one failure. Upstream onSubscription wraps collection in a `SubscribedFlowCollector`; the real StateFlow implementation recognizes that immediate collector to invoke the action. The intervening map/distinct collectors hide it. Consumers using a subscription initializer can therefore silently omit setup even though ordinary value/collection appears healthy. This is a public behavior defect, not an aesthetic objection or a HIGH isolation allegation. Confidence: **high**.

The supported extension's action contract is documented by [StateFlow API](https://kotlinlang.org/api/kotlinx.coroutines/kotlinx-coroutines-core/kotlinx.coroutines.flow/-state-flow/); the mechanism is corroborated by the pinned [Share.kt implementation](https://github.com/Kotlin/kotlinx.coroutines/blob/1.11.0/kotlinx-coroutines-core/common/src/flow/operators/Share.kt#L438-L446) and [StateFlow.kt collector handling](https://github.com/Kotlin/kotlinx.coroutines/blob/1.11.0/kotlinx-coroutines-core/common/src/flow/StateFlow.kt#L366-L369). The renewed two-test simulator run reproduces the same expected1/actual0 failure and separately passes the stronger nested-CAS probe. Fresh XML: `/private/tmp/artitor-reaudit-kotlin/renewed-flow-cas-results/TEST-iosSimulatorArm64Test.com.yet.tor.ArtiTorSessionConcurrencyTest.xml`.

**N02 — LOW / maintenance risk: unsupported third-party StateFlow inheritance.** The same object opts into ExperimentalForInheritanceCoroutinesApi and InternalCoroutinesApi. Upstream explicitly does not promise inheritance stability; future interface changes can require adaptation. This is a source/binary maintenance risk for a published library, separately from the concrete N01 failure. Ordinary replay/value equality, cancellation, never-normal-completion behavior, and terminal latching passed the executed probes. Supported MutableStateFlow/asStateFlow and stateIn implementations exist, but a replacement must preserve synchronous value/revision ordering; naively introducing a second publication can reintroduce F02, and stateIn adds scope/asynchrony considerations. No replacement was implemented.

**N03 — LOW / non-blocking horizon: native session revision overflow has profile-dependent behavior.** Session revision is u64, but R:618/651/2033/2072 use ordinary `+=1`, and close uses ordinary `+1` at R:736. A forced `u64::MAX` session demotion panics in a debug build. The release profile does not opt into overflow checks. With default Cargo release settings, ordinary arithmetic wraps; a production release overflow run was not executed. Kotlin's strict unsigned comparison would reject wrapped lower revisions. No explicit impossible-lifetime overflow policy was found. This horizon is practically unreachable and is not a release blocker. An intentional checked/wrapping/saturating policy or documented lifetime assumption is still preferable to accidental profile dependence. The forced-overflow probe catches the expected debug panic; its first version also overflowed during cleanup and was corrected before counting the final result.

**Registry-update analysis — no independent correctness finding reproduced.** Every outer registry transform operates on an immutable snapshot; wrapper acceptance has a separate atomic monotonic revision/status CAS. Re-running a transform with the same incoming revision cannot publish a second acceptance. Projection conflates equal public state/endpoint pairs even when revision changes. Terminals never change; removing membership uses the accepted wrapper state, so an old ignored terminal cannot remove a newer live wrapper. Native IDs are not reused, so a different/new wrapper under the same ID is not a production case. Creation reconciliation retries against newer pending records when a callback wins the outer CAS.

The broad assertion “callbacks never occur inside the transform” is **false if it includes public collectors**: an Unconfined collector can run synchronously during wrapper status update and re-enter close, which invokes native code. StateFlow updates resume collectors outside their own locks, so no registry mutex is being held. The renewed forced nested collector-close schedule observes exactly ACTIVE→PAUSED→CLOSED, count3 even after stale callback, zero pending entries and empty membership, without deadlock/resurrection. The outer transform must retry after nested close changes the registry. Multi-coroutine highest-revision checks and same-state suppression also pass. This establishes the tested retry/idempotence behavior, not a linearizable multi-object snapshot for observers midway through the transform or a universal proof for arbitrary consumer code. [The update API](https://kotlinlang.org/api/kotlinx.coroutines/kotlinx-coroutines-core/kotlinx.coroutines.flow/update.html) permits lambda re-evaluation.

### E. Transition-gate lock audit and native evidence addendum

All production lock sites were inspected, including scopes across helper calls and temporary guards, rather than deriving lock lifetime from graph edges/comments alone. The resulting partial order is:

| Held lock | Further locks acquired while held | Sites / observations |
|---|---|---|
| ArtiTor.inner | transition gate; client; listener; LOG_SINK; sessions | Lifecycle/create transactions; read-only accessors and session listing. No inverse acquisition while retaining those locks was found. |
| Transition gate | engine_state; client; sessions; listener; connections; socks_shutdown; LOG_SINK | Atomic mutation/admission/dispatch; listener refs cloned before notification; shutdown sink cleanup. |
| Shared.sessions | tombstones | Invalidation drains live entries and records terminal diagnostics. |
| Shared.listener | none of inner/transition | Getter clones the Arc and releases its mutex; set_listener takes listener and LOG_SINK sequentially, not nested. |
| Shared.engine_state / client / connections / tombstones | no reverse acquisition of transition | Actual inspected mutex scopes; no X→transition cycle found. |
| LOG_SINK | no inner/transition while its guard is retained | Tracing and panic hook clone the listener and release the sink mutex before calling it. |

Rebind's candidate snapshot holds sessions alone, releases it, then later acquires the transition gate. Snapshot lookup holds gate→sessions and releases sessions before tombstone lookup. Close keeps the gate across removal/tombstone recording but releases it before its callback. Runtime shutdown uses shutdown_background, not a join under inner/gate. Root/session connection tracking and aborts do not wait for handlers. No `.await`, synchronous FFI callback, or joined worker occurs under the inspected transaction locks. This is source/ownership evidence, not an assertion that every conceivable upstream synchronous tracing path was modeled.

Final creation validates Shared identity, generation, root epoch and lifecycle while holding inner+transition before insertion. Rebind validates lifecycle, candidate generation/state/revision under transition+sessions. Pause and ERROR commit lifecycle and demotion in the same gate; shutdown/root replacement invalidate and discard in their transaction. All ordinary explicit callback dispatches occur after those gates/registry scopes end. Reentry into hasClient/socksPort, pause, session close and legally reentrant shutdown passed the native suite and the independent initial-ACTIVE→shutdown extension. Callbacks returning without a deadlock do not establish freshness of later publications: N04 is the independently failing example.

The original F03 competitor adaptation waited 150ms for competing completion before releasing the after-read barrier, then joined both threads. A scheduler-delay-only explanation is weakened by the matching gate-removal control, where both competitors complete before release and both original failures recur. It remains a tested schedule, not a universal timing proof. In F04, the first successful-installation harness constructed B without a Tokio runtime context and failed; the corrected five-probe extension passes, and a subsequent corrected-harness epoch-comparison mutation fails. The earlier uncorrected mutation run is not counted. Exact exception text was suppressed by its installed panic hook; the failing assertion output is less diagnostic than F03's printed state/endpoint counterexamples.

**N04 — MEDIUM / OPEN: inherited stale ERROR publication after legal callback shutdown.** The new final reentry probe calls the production `notify_worker_error` with a matching current worker revision. Its `on_error` callback synchronously invokes shutdown. It returns without deadlock; the authoritative new native Shared is OFF, hasClient=false, and the session is INVALIDATED. Nevertheless the captured listener receives `[OFF, ERROR]`: R:594–602 unconditionally sends the old transaction's ERROR after on_error returns. The assertion requiring completed shutdown to remain the last public state fails (exit101). K:348–368 accepts that stale ERROR with no engine-event revision guard, so the native observer sequence can put the facade out of agreement with native OFF.

This is **not labeled a remediation-introduced regression**. The predecessor commit `0ac8867fda0cec5fe47166ff9cb7ab17b226cfab` already has on_error followed unconditionally by report(ERROR); inherited origin is inferred from that inspected source, not a separately rerun predecessor runtime. It is newly falsified here under the requested legal reentry audit. Location/reproducer: `/private/tmp/artitor-reaudit-f05/reentry-probe.rs`, `reaudit_error_callback_shutdown_final_publication_is_off`. No production fix or publication guard was added. Confidence: high for current failure; moderate for the historical origin inference.

### F. Verification matrix and every live attempt

| Command / attempt | Observed result |
|---|---|
| Initial sandboxed native attempts | Socket-dependent tests denied loopback operations; observed run 33 passed/37 failed. These are permission failures, not product regressions. |
| Permitted repository `cargo test --manifest-path rust/arti-kmp-ffi/Cargo.toml` before interruption | Exit0, 70 passed, doc tests0. |
| Same repository native suite with `-- --test-threads=1` | Exit0, 70 passed, doc tests0. |
| Fresh permitted repository native suite after resumption | Exit0, 70 passed, doc tests0; original production package binary. |
| First combined simulator/device-compilation/Android-assembly invocation, Gradle9.6.1 | XML:66 tests, zero failures/errors/skips; both live tests pass. Aggregate command exit was not retained after oversized tool output, so XML proves tests, not every build task. |
| Accidental overlapping second Gradle invocation | Explicitly interrupted exit130 during build before live execution; no separate live result claimed. |
| Final pre-interruption combined matrix, Gradle9.6.1 | Surviving XML:66 tests, zero failures/errors/skips, both live tests pass; aggregate terminal exit unavailable after interruption. |
| Fresh `:tor:compileKotlinIosArm64 :tor:assembleAndroidDeviceTest`, Gradle9.8.0 | Exit0, BUILD SUCCESSFUL52s; compilation/assembly only. |
| Fresh `:tor:iosSimulatorArm64Test`, Gradle9.8.0 | Exit0, BUILD SUCCESSFUL3m27s;66 tests, zero failures/errors/skips, both live tests pass. |
| Independent Kotlin28-test class | Exit1,27 pass/1 fail: N01 subscription action expected1/actual0. |
| Kotlin exception/revision/pending mutations | Three runs, each exit1 with2 intended failures; exact original pending histories reach160 each. |
| Kotlin no-init checked-in restoration before interruption | Exit0,20 checked-in session tests pass. |
| Renewed onSubscription + strengthened nested-CAS probes, Gradle9.8.0 | Exit1,2 tests/1 failure: onSubscription expected1/actual0 again; nested CAS exact ACTIVE→PAUSED→CLOSED/count3/empty registry/pending0 passes. Fresh XML retained. |
| Renewed no-init checked-in restoration | Exit0,20 tests pass, BUILD SUCCESSFUL26s; normal source routing restored. |
| Independent original native eight-test corpus | Six pass/two red: raw F02 delivery order and raw F09 permanent diagnostic claim, explicitly changed contracts. |
| Native extended epoch/revision/reentry/ordering/overflow corpus | Corrected run5 passed; earlier2 harness errors corrected and not counted as production defects. |
| Independent SOCKS corpus |15 passed initially and restored; four mutation categories produce intended failures. |
| Corrected native demotion-removal mutation / baseline restoration | Mutation exit101: ERROR observers see ACTIVE twice; restored exit0: PAUSED/null twice. |
| Final legal ERROR-callback→shutdown reentry probe | Exit101: native OFF, no client, session INVALIDATED, but delivered OFF→ERROR (N04). No deadlock. |
| `adb devices -l`, permitted checks | No devices attached before or after interruption. |
| `git diff --check` | Exit0 after appending this section; no whitespace errors. |

All completed live attempts in this re-audit, including repeat history:

| Attempt | XML UTC start | TorIosE2ETest class time | Root bootstrapFetchPauseResume | liveTwoSessionsLifecycle |
|---|---|---|---|---|
|1, Gradle9.6.1 |2026-09-30T08:23:19.720Z |114.267s |PASS |PASS |
|2, Gradle9.6.1 |2026-09-30T08:32:56.547Z |126.035s |PASS |PASS |
|3, Gradle9.8.0 |2026-09-30T11:15:13.756Z |93.965s |PASS |PASS |

The interrupted overlapping build did not execute an additional live attempt. No retry-until-green filtering was used. No CONNECT code5 recurrence was observed in these three paired runs. The older intermittent failure's cause remains **unknown**; three passes do not establish an environmental-only cause or disprove a reliability defect.

**Safe diagnostic privacy.** `safe_connect_diagnostic` (socks.rs:139–145) formats only payload-free categorical `HasKind::kind()`, not upstream Error Debug/Display or target/configuration. Independent marker tests include hostname/onion, bridge-like text, auth-key and raw-config markers; output is exactly the categorical InvalidStreamTarget diagnostic. Mutations formatting upstream Error Debug or Display fail. The selected validation error itself does not retain the sensitive hostname, so marker absence alone is not sufficient proof; exact categorical-output assertions and the payload-free enum source establish this formatter's absence guarantee. This conclusion is scoped to the new formatter, not every unrelated upstream log path.

**Android runtime: NOT VERIFIED**

**Live multi-session Tor traffic: VERIFIED** — checked-in iOS simulator lifecycle/traffic test, three completed attempts. Android assembly is not Android runtime; these tests are not measurements of circuit IDs, unlinkability, or device resource capacity.

### G. Claims re-falsified and still holding

1. Public session Result uses the existing exact public error classes, including generated synchronous create failures.
2. Delayed/lower/equal revisions cannot resurrect paused/terminal public wrappers; terminal state/endpoint stays atomic.
3. Final session admission and demotion share a serialized lifecycle transaction; removing it recreates both original read/commit races.
4. Old isolated A cannot enter B's epoch, including synchronous replacement failure; ordinary pause/resume/sibling creation do not redefine root identity.
5. ERROR demotes sessions before ordinary engine observers; typed error precedes its corresponding ERROR status in the tested routes.
6. Root/A/B dispatch selects the corresponding constructed client handles; no exit-IP comparison is used.
7. Current fatal session-listener failure is local; stale listener failures cannot demote a newer instance.
8. Completed Kotlin operations do not retain terminal/orphan history; native tombstones remain bounded and public CLOSED remains permanent.
9. Shared SOCKS parsing respects exact frame lengths and leaves application payload for the relay, without local DNS.
10. Scope comparison of remediation against its predecessor found no bridge tri-state, public TorErrorKind, onion/timeout config, dormant/PT/hosting/RPC, Cargo feature, or live-reconfigure implementation. Cargo.toml/config surface and Apple SQLite mitigation remain unchanged. The separate later Gradle wrapper change is disclosed above.

### H. Remaining release follow-ups and verdict

Actual Phase-1 acceptance defects: **N01 StateFlow subscription semantics**, **N04 stale engine ERROR publication after legal reentrant shutdown** (inherited, newly reproduced), and **F10 stale normative session-port preservation wording**. Unsupported inheritance and unreachable revision overflow are low maintenance/horizon issues, not HIGH isolation findings. No original HIGH counterexample remains reproducible in the repaired public/transaction contracts tested here.

Separate release follow-ups: Android hardware runtime; Phase-5 empirical Android/iOS session FD/memory/task measurement and cap reassessment; monitoring/investigation of the historical unexplained CONNECT code5 with the safe categorical diagnostic if it recurs. These are not relabeled Phase-1 correctness failures.

Only this report was changed by the reviewer. Production hashes remain unchanged; no findings were fixed and no Phase 2 work began.

**INDEPENDENT PHASE 1 RE-AUDIT: REMEDIATION REQUIRED**


### Final narrow remediation addendum — 2026-09-30 (implementer)

The independent reviewer's findings and REMEDIATION REQUIRED verdict above
are preserved verbatim. N01/N02 now use supported MutableStateFlow/asStateFlow
with internal revision convergence; N04 now fences lifecycle publication after
foreign callback reentry using a dedicated engine publication revision; F10's
§9 port-preservation/root-only claims have been replaced with the accepted
normal listener pause/rebind and ephemeral-session-endpoint contract.

Permanent subscription/replay/cancellation/equal-state revision/nested-CAS
regressions and production ERROR callback/session-callback shutdown, legal
pause, and ordinary-order tests were added. Disposable mutations reproduce
subscription action count 0, stale ACTIVE resurrection, and OFF→ERROR.
Restored matrix: native 75/75 default and serial; simulator 72/72 including
both live tests (two recorded attempts); iOS device compilation and Android device-test
assembly exit 0. Android hardware runtime remains unverified (no device).
N03, empirical resource/cap measurements, and historical unexplained CONNECT
code5 monitoring remain non-blocking follow-ups. Full attempt/failure/mutation
evidence and counter roles are in the implementation report's
“Final Acceptance Remediation” section. No Phase 2 work began. This addendum
records implementation evidence only, not independent acceptance or Phase 1 PASS.


## Final independent Phase-1 acceptance check — 2026-09-30

**Reviewed committed baseline:** `55ca6d1eb42557b893e59a135c4fccd822ba4cb5`.
Fresh `git rev-parse HEAD` returned exactly that SHA. Initial `git status
--short` showed only modified `docs/audit/ARTITOR_0_3_PHASE1_INDEPENDENT_AUDIT.md`
and untracked `docs/audit/ARTITOR_0_3_CAPABILITY_AUDIT.md`. These are pre-existing
reviewer/audit documents, not committed production changes. No production,
build, or test source relevant to acceptance was dirty. This section is an
uncommitted acceptance addendum; earlier findings/verdicts are preserved
byte-for-byte. No production fix, repository test edit, or Phase 2 work occurred.

SHA-256 values, independently computed from working files and checked against
`git show 55ca6d1:<path>` contents:

| File | SHA-256 |
|---|---|
| rust/arti-kmp-ffi/src/lib.rs | 48c39ce5422c6fa97eea04df09bcaf6567d53db7f7152a600d9a1fe08d0f6e7e |
| rust/arti-kmp-ffi/src/socks.rs | 735324801f82a7e53f76a50f71256ef1e08e77d4198890882e442ad4336b521a |
| tor/src/commonMain/kotlin/com/yet/tor/ArtiTorClient.kt | 1377e63dfbdc2291aaf3085a82202b321de7c07c05d41660b6e42a8997bf99b4 |
| docs/design/ARTITOR_0_3_API_FREEZE.md | 82f0aed4c785f7314d1dd88b87b74272263e829284f48c67e73ee304bf3cd53f |

The hashes were checked again after every disposable mutation and at final
restoration. Repository production was never mutated: experiments used
`/private/tmp/artitor-final-acceptance` copies. Kotlin source routing was changed
only by per-invocation temporary Gradle init scripts, not repository build files.
Native experiments used a separately named package with copied dependencies,
source and lockfile, sharing the dependency target cache. Scratch mutations
were restored in finally blocks and checked against their saved bytes.

### Disposition of the requested findings

**N01: CLOSED.** Session status is backed by a real
MutableStateFlow(initialStatus.status).asStateFlow(), with a separate internal
MutableStateFlow of revisioned records. Production no longer directly
implements StateFlow and has no session-status use of
ExperimentalForInheritanceCoroutinesApi, InternalCoroutinesApi or FlowCollector.
The exact old onSubscription/first counterexample passes in the fresh committed
26-test session class, including direct MutableStateFlow control count 1.
Value/replay, cancellation, same public state with newer revision without a
necessary duplicate, and rejection of a later lower revision pass.
An additional independent simulator probe confirms collection stays active
after CLOSED, first/replayCache return CLOSED, and cancelAndJoin runs cleanup.
This closes the supported-StateFlow defect, not the separate publication-window
correctness defect below.

**N02: CLOSED.** Unsupported custom StateFlow inheritance and both coroutines
API opt-ins are absent from the production session implementation.

**N04: CLOSED for the reviewed callback-reentry schedules.** Production
notify_worker_error was exercised with the matching current worker revision.
The committed typed-error→shutdown test finishes native OFF, no client,
INVALIDATED session, observer last OFF and no ERROR after OFF. The committed
session-PAUSED→shutdown, typed-error→pause, ordinary ERROR ordering and
multi-session ERROR→pause tests all pass. A new independent three-session probe
forces shutdown from the first PAUSED callback after all demotions commit;
its exact history is:

```text
session Paused
session Invalidated
session Invalidated
session Invalidated
Off
```

All three handles are INVALIDATED/null, live registry empty, native OFF with
no client, and neither typed error nor stale ERROR is emitted after shutdown.
Shutdown, rather than PAUSED snapshot replay, completes these INVALIDATED
notifications; ERROR→pause replay covers the separate pause schedule.
The actual Kotlin facade's scripted-native typed-error collector test passes:
OFF, lastError null, empty sessions, old session INVALIDATED, pending zero.
This facade test is not relabeled an integrated native fault-injection test;
real-native freshness is established by the production-route Rust tests.

Ordinary ERROR still orders session PAUSED/null → typed on_error → engine ERROR.
Legal typed-error pause finishes engine PAUSED, with current PAUSED snapshots
replayed at the same session revisions and no stale final ERROR.

Source review found a distinct engine_publication_revision, advanced under the
transition gate by worker STARTING/BOOTSTRAPPING reports, RUNNING root bind,
ERROR, pause/bootstrap cancellation, shutdown/OFF, cold/recovery spawn,
set_client_under_gate replacement, and SOCKS recovery spawn. Its role differs
from generation (Shared lifetime), client_epoch (root identity), session
status_revision (session ordering), and worker_revision (worker cancellation).
ERROR releases its transition guard before callbacks and checks publication
freshness before/after session callbacks, after on_error, and after ERROR status
before logging. No callback is protected by retaining inner/transition locks.
Old ERROR compares with its own old Shared, whose fence shutdown advances
before replacement. Shutdown's pending OFF checks the actual replacement
Shared's initial revision, so subsequent lifecycle mutation supersedes it.
Pause keeps valid session deliveries separate from stale engine publication;
shutdown finishes terminal sibling notifications. No new lock inversion was
found in these scopes.

**F10: CLOSED.** Read the entire current freeze and searched all port/rebind/
collision/endpoint references. No normative session-port preservation or
root-only-rebind claim remains. §§1, 5, 6, 9 and 12 consistently permit new
session endpoints. §9 preserves TorClient/isolation identities without bootstrap,
requires normal root/session pause/rebind, states additional ports can change
without collision, requires observing session.status and rebuilding proxy-bound
clients on endpoint change, and excludes root socksPort from additional-port
selection. Root-only readiness wording in §6 describes root isReady, not
session-port stability, and is not a contradiction.

### N05 — MEDIUM / OPEN: public terminal resurrection inside revision convergence

**Concrete reproduced acceptance blocker. Confidence: high.** The replacement
publisher at ArtiTorClient.kt:238–241 reads the accepted record, then assigns
its cached public status before rechecking acceptance. Another caller can
publish a newer terminal between that read and assignment:

```text
A accepts ACTIVE rev2, reads latest ACTIVE rev2, pauses before public assignment
B closes the session, accepts/publishes CLOSED rev3, removes membership, returns
A resumes, assigns ACTIVE rev2 with old endpoint
A observes accepted CLOSED rev3 and repairs public status to CLOSED
```

The independent simulator test
acceptanceOldWriterAfterCompletedCloseNeverPublishesActive failed (exit 1,
1 test/1 failure). Exact collector history and captured public value:

```text
[ACTIVE, CLOSED, ACTIVE, CLOSED]
TorIsolationSessionStatus(state=ACTIVE,
  socksEndpoint=TorSocksEndpoint(host=127.0.0.1, port=21001))
```

Failure: `terminal latch after close returns. Expected <CLOSED>, actual <ACTIVE>`.
Final convergence, empty membership and pending count zero all passed before
that assertion. Thus this is an observable transient resurrection after
completed close, not a final stale state, registry leak, or unsupported
StateFlow-inheritance issue. It violates the permanent public terminal latch
required by freeze §5 and ArtiTorClient.kt:178–180, including a second CLOSED
emission separated by ACTIVE. The analogous INVALIDATED case follows from the
same assignment path, but no separate INVALIDATED runtime result is claimed.

Reproduction did not change production semantics to manufacture the stale
value: the isolated copy adds only a nullable scheduling hook immediately after
accepted.value is read and an observation hook immediately after assignment.
The former blocks A until completed close; the latter captures the real public
value before the loop repairs it. All accepted/public-status assignments and
comparisons remain the committed implementation. An Unconfined collector also
observes the four-state history before assertions. The repository production
file and its hash were untouched. Test-only runBlocking is a scheduling barrier
in this disposable probe, not a production addition.

Reproduction/evidence:

- Source-routing init script: /private/tmp/artitor-final-acceptance/terminal-probe.init.gradle
- Source instrumentation diff: /private/tmp/artitor-final-acceptance/terminal-probe-instrumentation.diff
- Isolated test: /private/tmp/artitor-final-acceptance/kotlin-tests/com/yet/tor/ArtiTorSessionConcurrencyTest.kt
- Command: rtk ./gradlew -I /private/tmp/artitor-final-acceptance/terminal-probe.init.gradle :tor:iosSimulatorArm64Test --tests '*acceptanceOldWriterAfterCompletedCloseNeverPublishesActive*' --console=plain
- Log: /private/tmp/artitor-acceptance-terminal-probe.log
- XML: /private/tmp/artitor-final-acceptance/terminal-probe-xml

The original delayed callback and synchronous nested collector tests remain
green: a rejected incoming revision is not directly republished, and nested
close/pause completes correctly. Those schedules do not stop a publisher
between reading latest and writing its cached status. Eventual lock-free
convergence therefore does not establish the claimed permanent public latch.
No infinite loop was reproduced; the failing schedule actually converges.
This new MEDIUM public-publication defect requires remediation before acceptance.
It is related to F02/F09 guarantees; it does not relabel the still-green
original HIGH final-state/native-ownership counterexamples as reproduced.
No production fix was attempted.

### Fresh verification, mutation evidence, and every live attempt

| Command / probe | Exact observed result |
|---|---|
| rtk proxy cargo test --manifest-path rust/arti-kmp-ffi/Cargo.toml | exit 0: 75 passed, 0 failed; doc tests 0 |
| rtk proxy cargo test --manifest-path rust/arti-kmp-ffi/Cargo.toml -- --test-threads=1 | exit 0: 75 passed, 0 failed; doc tests 0 |
| rtk ./gradlew :tor:iosSimulatorArm64Test :tor:compileKotlinIosArm64 :tor:assembleAndroidDeviceTest --console=plain | exit 0: BUILD SUCCESSFUL 2m15s; simulator 72 tests, zero failures/errors/skips; iOS device compile executed; Android assembly UP-TO-DATE/successful |
| Isolated native acceptance_ probes, offline / separate package | exit 0: 2 passed; three-session shutdown and engine counter no-wrap |
| Isolated native error_ production-route corpus | exit 0: 13 passed, 0 failed, including new multi-session shutdown probe |
| Independent delayed-read terminal publication probe | exit 1: 0 passed/1 failed; N05 CLOSED→ACTIVE→CLOSED |
| Independent supported-flow terminal non-completion/cancellation probe | exit 0: 1 passed, 0 failed |
| Mutation A: previous custom map/distinctUntilChanged StateFlow, isolated Kotlin copy | exit 1: 0 passed/1 failed; onSubscription expected 1, actual 0 |
| Mutation B: direct publicStatus.value = incoming.status, isolated Kotlin copy | exit 1: 26 tests, 20 passed/6 failed; stale ACTIVE resurrection and revision/reentry failures |
| Mutation C: remove ONLY check immediately after on_error, isolated native copy | exit 101: 0 passed/1 failed; session Paused → typed error → session Invalidated → Off → Error |
| Restored isolated native exact typed-error→shutdown test | exit 0: 1 passed, 0 failed |
| Restored committed session class, without any init script | exit 0: 26 tests, zero failures/errors/skips; BUILD SUCCESSFUL 34s |
| SDK adb devices -l | exit 0: no attached devices; no Android runtime executed |
| rtk git diff --check after this addendum | exit 0, no whitespace errors |

Mutation B failures: delayedActiveAfterPausedIsRejectedByRevision;
newerNativeSnapshotWinsOverEarlierPendingCallback;
statusFlowProjectsRevisionChangesWithoutDuplicatePublicStatuses;
concurrentCallbacksKeepHighestRevisionWithoutSecondaryPublicationRace;
latestReplayAndSameStateRevisionFence;
activePublicationCollectorReentersPauseWithoutResurrection. Its observed reentry
history includes PAUSED→ACTIVE→PAUSED→ACTIVE→PAUSED. Mutation C failed on the
required real Off→Error suffix, not on compilation/harness setup.

One live simulator attempt ran in this acceptance check; no live retry ran.
UTC XML start 2026-09-30T12:44:26.286Z, local Asia/Tbilisi 16:44:26.286.
Both live tests passed: liveTwoSessionsLifecycle 41.159s;
bootstrapFetchPauseResume 53.142s; live class total 94.301s.
All independent probes, mutations and restoration runs selected deterministic
session/native tests and executed no additional live tests. Their expected red
results are retained, not hidden. The historical intermittent CONNECT code5
cause is still unknown; this pass is not proof of its environmental origin.

Evidence logs use /private/tmp/artitor-acceptance-*.log. Baseline full-platform
XML is preserved under /private/tmp/artitor-final-acceptance/baseline-platform-xml;
mutation A/B XML, terminal-probe XML, supported-flow XML and restored-session
XML are separate subdirectories so later runs do not overwrite earlier results.

### F01–F14 regression smoke and separate release follow-ups

All existing permanent smoke tests remained green in the fresh repository
native/default/serial and full simulator runs: F01 generated exception mapping;
F02 delayed/equal/lower revision rejection; F03 create/rebind transaction-gate
races; F04 failed-root-rebuild/client-epoch admission rejection; F05 demotion
before ERROR observers; F06 ERROR replacement-start/retained-resume paths;
F07 pending-history churn/cleanup; F08 current/stale accept-listener handling;
F09 bounded tombstones and ordinary terminal latch tests; F12 actual root/two-
session dispatch handle identity; F13 exact SOCKS framing/application-payload
preservation; F14 typed error before engine ERROR. F10 wording is closed above.
F11 cap wording remains honest and unmeasured. Green existing smoke does not
override N05's newly forced public latch failure.

Non-blocking release follow-ups, not new correctness failures:

- Android hardware runtime (no device attached); assembly is not runtime proof.
- Phase-5 Android/iOS FD/memory/task measurements and cap reassessment.
- N03 session u64 revision-exhaustion horizon remains LOW/non-blocking;
  session arithmetic is unchanged. The separate new engine publication counter
  uses checked_add; an independent max-1→max increment succeeds, the next
  increment raises the expected caught exhaustion panic, and its stored value
  stays max rather than wrapping into an old valid token.
- Historical intermittent CONNECT code5 monitoring/investigation; cause unknown.

Confidence: high for recorded runtime outcomes, hashes, the N05 counterexample,
and scoped synchronous native callback freshness; no universal arbitrary-
scheduling or network-isolation measurement claim is inferred from these runs.
Production/build/test sources remain the reviewed committed SHA. Only this
addendum was appended to the existing audit state.

**INDEPENDENT PHASE 1 ACCEPTANCE: REMEDIATION REQUIRED**


## Final Independent Phase-1 Acceptance — 2026-09-30

This actionable addendum continues the completed acceptance check; no audit or
live test was restarted. The saved logs, XML, scheduling probe and baseline
hashes remain available. A01 below is the reporting ID for the same single
defect previously recorded as N05, not a second defect. The earlier bare final
response omitted the evidence already written in this document. The verdict
is evidence-supported by the retained failed simulator probe.

### Baseline

HEAD: `55ca6d1eb42557b893e59a135c4fccd822ba4cb5`, independently rechecked during
this follow-up. Working tree: modified independent-audit document and untracked
capability-audit document only. The former contains pre-existing reviewer edits
and appended acceptance sections; the latter is untouched. Production/build/
checked-in test source is clean. Production files modified by reviewer: **NO**.
All four requested SHA-256 hashes still match those recorded in the preceding
acceptance section and the reviewed committed contents. No production fix or
Phase 2 work occurred. Only this document is appended in this follow-up.

### ACCEPTANCE FINDINGS

**ID: A01**

**Severity: MEDIUM. Status: OPEN. Confidence: HIGH.**

Title: stale cached ACTIVE publication resurrects a public CLOSED session.

**Contract / invariant.** Once session.close() has published CLOSED/null and
returned, the public session status must latch that terminal state forever.
It must not emit ACTIVE, restore an endpoint, or emit CLOSED a second time after
an intervening live state. This is the public terminal promise in
ArtiTorClient.kt:178–180 and API freeze §5. It also violates F02's requirement
that older ACTIVE publication cannot undo a newer lifecycle publication.

**Actual behavior.** The internal accepted record latches CLOSED correctly,
but a concurrent publisher can still assign an older cached ACTIVE record to
the separate public MutableStateFlow. The loop subsequently repairs it. The
public collector observes ACTIVE → CLOSED → ACTIVE → CLOSED, including the old
loopback endpoint after completed close. Final convergence to CLOSED does not
undo the already-observed terminal resurrection.

**Deterministic reproducer.** Start with a public session ACTIVE/rev1. Publisher
A accepts ACTIVE/rev2 at endpoint 127.0.0.1:21001, reads accepted.value into its
local latest, and is paused immediately before publicStatus assignment.
Publisher B invokes session.close(): native callback delivers CLOSED/rev3,
acceptance/publishing completes, registry membership is removed, close returns.
Release A. A writes cached ACTIVE/rev2 to publicStatus, then detects accepted
CLOSED/rev3 and writes CLOSED again. An Unconfined collector captures both
transitions. The scheduling gate is in a disposable source copy, not repository
production; it controls only the read-to-write interleaving.

**Observable failure.** Exact observed history:

```text
[ACTIVE, CLOSED, ACTIVE, CLOSED]
```

The observation hook immediately after the old write captures:

```text
TorIsolationSessionStatus(state=ACTIVE,
    socksEndpoint=TorSocksEndpoint(host=127.0.0.1, port=21001))
```

Failed assertion: `terminal latch after close returns. Expected <CLOSED>,
actual <ACTIVE>`. Final CLOSED, empty membership and pending count zero passed
before this failure; this finding is specifically transient public resurrection,
not a claim of a final stale value or native-resource resurrection.

**Production location.**
`tor/src/commonMain/kotlin/com/yet/tor/ArtiTorClient.kt`,
TorIsolationSessionImpl.acceptStatus() → publishLatestAccepted():
line 238 reads latest; line 239 assigns its cached status; line 240 checks for
newer acceptance only after that assignment. The internal terminal check at
line 227 cannot prevent a stale record already read by another publisher from
reaching publicStatus.

**Evidence: INDEPENDENT TEMP PROBE.**
Test: `acceptanceOldWriterAfterCompletedCloseNeverPublishesActive`.
Command exit: **1**. XML: **1 test, 1 failure, 0 errors/skips**.
This is not a mutation that injects a stale assignment: the committed assignment
and convergence logic are preserved, with only before-assignment scheduling
and after-assignment observation hooks. Diff and source are retained.

Exact reproduction command (runs scratch source/test routing only):

```text
rtk ./gradlew -I /private/tmp/artitor-final-acceptance/terminal-probe.init.gradle :tor:iosSimulatorArm64Test --tests '*acceptanceOldWriterAfterCompletedCloseNeverPublishesActive*' --console=plain
```

Evidence paths:

- /private/tmp/artitor-acceptance-terminal-probe.log
- /private/tmp/artitor-final-acceptance/terminal-probe-xml/TEST-iosSimulatorArm64Test.com.yet.tor.ArtiTorSessionConcurrencyTest.xml
- /private/tmp/artitor-final-acceptance/terminal-probe-instrumentation.diff
- /private/tmp/artitor-final-acceptance/terminal-probe-main/com/yet/tor/ArtiTorClient.kt
- /private/tmp/artitor-final-acceptance/kotlin-tests/com/yet/tor/ArtiTorSessionConcurrencyTest.kt

**Would the current checked-in suite detect this defect? NO.** In the actual
independent run the full 72-test simulator suite and restored 26-test session
class both passed. Existing delayed-callback tests delay before acceptance,
synchronous nested collectors reenter during assignment, and concurrent
highest-revision tests check the final value after publishers finish. They do
not force a stop between reading accepted.value and assigning publicStatus or
assert this concurrent collector history. Only the independent added scheduling
probe detects the demonstrated window.

**Did the final remediation introduce any NEW correctness defect? YES: A01.**
Commit 55ca6d1 introduces the separate publicStatus and cached-record convergence
write. Its predecessor's custom implementation reads public value directly
from accepted and has no separate cached publicStatus write. This origin is
confirmed by the commit's source diff; no predecessor runtime result is claimed.
No other concrete new blocker was found in engine freshness, ERROR→pause
replay, replacement-Shared token handling, callback lock scopes or sibling
notification delivery. The independent three-session shutdown probe completed
all INVALIDATED notifications and final OFF, with no stale typed error/ERROR.

### Explicit N01 / N04 / F10 status

**N01: CLOSED.** The exact onSubscription/first counterexample now executes its
action once; real MutableStateFlow/asStateFlow replay/value, cancellation,
non-completion after terminal and equal-state revision fencing pass.

**N04: CLOSED.** Matching-current-revision production notify_worker_error tests
for typed-error shutdown, session callback shutdown, typed-error pause and normal
ordering pass; the independent three-session shutdown finishes OFF after all
INVALIDATED deliveries without stale typed error or ERROR.

**F10: CLOSED.** The entire freeze was read/searched and consistently preserves
TorClient/isolation identities while allowing normal root/session pause/rebind
and ephemeral endpoint changes, with the app rebuild obligation stated.

N02 remains CLOSED: unsupported custom inheritance/opt-ins were removed.
These closed items are not the justification for REMEDIATION REQUIRED.

### Previous F01–F14 status after final acceptance

| Finding | Status after final acceptance |
|---|---|
| F01 | CLOSED |
| F02 | REOPENED — public publication-order guarantee, concrete A01 sequence above |
| F03 | CLOSED |
| F04 | CLOSED |
| F05 | CLOSED |
| F06 | CLOSED |
| F07 | CLOSED |
| F08 | CLOSED |
| F09 | CLOSED — original bounded native-diagnostic/tombstone finding |
| F10 | CLOSED |
| F11 | CLOSED — unsupported measured-cap wording corrected; empirical measurement remains a release follow-up |
| F12 | CLOSED |
| F13 | CLOSED |
| F14 | CLOSED |

This makes the status implication explicit: A01 reopens F02's prohibition on
older public publication undoing newer lifecycle state. The original indefinitely
wrong final-state counterexample remains repaired; A01 reproduces a transient
stale public emission after completed close, with eventual correct convergence.
F09's original native CLOSED→INVALIDATED-after-pruning finding stays closed
under its explicitly amended bounded-diagnostic contract. The public terminal
latch, separately promised, fails as A01. No unrelated prior finding is reopened
because of incomplete proof. All existing permanent smoke tests remained green;
they lack the newly forced publication window.

### Independently observed verification results

| Requested check / executed command | Exact result |
|---|---|
| cargo test default: rtk proxy cargo test --manifest-path rust/arti-kmp-ffi/Cargo.toml | exit 0; 75 passed, 0 failed; doc tests 0 |
| cargo test serial: rtk proxy cargo test --manifest-path rust/arti-kmp-ffi/Cargo.toml -- --test-threads=1 | exit 0; 75 passed, 0 failed; doc tests 0 |
| iosSimulatorArm64Test | combined platform command exit 0; 72 tests, 0 failures/errors/skips |
| compileKotlinIosArm64 | task executed successfully in combined command, exit 0 |
| assembleAndroidDeviceTest | task UP-TO-DATE/successful in combined command, exit 0; assembly only |
| git diff --check | exit 0 after acceptance section and again after this actionable addendum |

Executed combined platform command:

```text
rtk ./gradlew :tor:iosSimulatorArm64Test :tor:compileKotlinIosArm64 :tor:assembleAndroidDeviceTest --console=plain
```

Aggregate BUILD SUCCESSFUL in 2m15s. These are independently observed acceptance
results, not reused implementation-team numbers. Restored unmodified session
suite: 26 passed, exit 0. Independent supported-flow terminal non-completion/
cancellation probe: 1 passed, exit 0. Independent native no-wrap/three-session
shutdown probes: 2 passed, exit 0; targeted native error corpus: 13 passed,
exit 0. A01's separate independent probe is red despite the green matrix.

Mutation A: exit 1, 1 test/1 failure, subscription action expected 1/actual 0.
Mutation B: exit 1, 26 tests/6 failures, including stale ACTIVE resurrection.
Mutation C: exit 101, 1 test/1 failure, exact Off→Error suffix after typed-error
shutdown. Restored C test: exit 0, 1 passed. All mutation work was confined to
scratch copies; repository hashes remained unchanged after every run.

**Live test attempts: 1** during the completed acceptance check.
**Live test failures: 0.** Both tests passed on that attempt:
liveTwoSessionsLifecycle 41.159s; bootstrapFetchPauseResume 53.142s.
UTC start 2026-09-30T12:44:26.286Z (Asia/Tbilisi 16:44:26.286).
No live retry or new live attempt occurred in this evidence-recovery follow-up.

**Android runtime: NOT VERIFIED.** SDK adb returned no attached devices.
Assembly is not hardware/runtime proof.

### PHASE-1 CORRECTNESS BLOCKERS

- **A01 — OPEN / MEDIUM:** observed public CLOSED→ACTIVE→CLOSED after completed
  close. Fixing subscription semantics did not preserve permanent terminal
  publication ordering. This alone justifies REMEDIATION REQUIRED.

### NON-BLOCKING RELEASE FOLLOW-UPS

- Android hardware runtime.
- Phase-5 Android/iOS FD/memory/task measurements and cap reassessment.
- N03 session revision-exhaustion horizon, LOW/non-blocking; new engine counter
  checked arithmetic was independently verified not to wrap.
- Historical intermittent CONNECT code5 monitoring/investigation; cause unknown.

These follow-ups do not justify the failing acceptance verdict. A01 does.
No production code was modified or fixed. All preceding audit/re-audit/history
sections remain preserved.

**INDEPENDENT PHASE 1 ACCEPTANCE: REMEDIATION REQUIRED**

## A01 remediation implementation evidence — 2026-09-30

This is implementation evidence, not a new independent acceptance verdict.
The preceding reviewer A01 finding, F02 status and acceptance verdict remain
unchanged pending final independent recheck. Baseline:
`55ca6d1eb42557b893e59a135c4fccd822ba4cb5`.

Publication now revalidates the accepted target before conditional public
MutableStateFlow CAS, retries after competing writes, and rechecks acceptance
after CAS for synchronous collector reentry. Supported asStateFlow, internal
strict revision acceptance and permanent accepted terminal latch are preserved.
The only test seam is a null-by-default internal scheduling callback immediately
before CAS. No native production/lifecycle/registry change occurred.

Permanent CLOSED and INVALIDATED regressions gate the cached ACTIVE publisher
until close/shutdown publishes terminal rev3 and completes. Both failed before
the fix (exit 1; 2/2 failures) with terminal→ACTIVE→terminal histories. Both pass
after the fix with ACTIVE→terminal only, plus terminal/null final value, empty
membership and zero pending checks. A disposable unconditional-write mutation,
keeping prevalidation and convergence, fails the same permanent tests (exit 1;
2/2 failures) with exactly those forbidden histories. Repository production was
never mutated. Final builds run normal source sets.

The targeted session class passed all 30 tests, including finite coordinated
contention with completion joins, ACTIVE collector close/pause, F02 delayed
revision rejection and N01 subscription/replay/cancellation/equality checks.
An initial new collector-close fixture incorrectly used equal CLOSED revision3;
it was corrected to revision4 without weakening production acceptance.

Final combined simulator/device/Android-assembly command exited 0:
76 simulator tests, no failures/errors/skips; iOS device compile and Android
test assembly executed successfully. Native default: exit0, 75 passed/0 failed,
0 doc tests. Initial sandbox socket-binding denial required an allowed native
rerun. Native source hashes are unchanged. git diff --check exited0.

Live attempts: 2 suite runs / 4 live test executions / 1 failed live test.
Attempt1: two-session test passed; bootstrapFetchPauseResume failed with
SOCKS CONNECT code5 (cause unknown). The single permitted retry passed both.
The first full run was therefore 75 passed/1 failed, exit1; the final full run
was 76 passed/0 failed, exit0. Both attempts remain recorded, not hidden by retry.
Android runtime remains NOT VERIFIED (adb reported no attached devices).

Exact commands, test names, timestamps, durations, scope and retained evidence
are in the appended A01 section of ARTITOR_0_3_PHASE1_IMPLEMENTATION_REPORT.md;
logs/XML are under /private/tmp/artitor-a01/. A focused read-only code review
found no actionable issues. Confidence HIGH for deterministic A01 repair and
mutation detection; independent acceptance is pending.

Unchanged non-blocking follow-ups: Android hardware; Phase-5 FD/memory/task/cap
measurements; N03 exhaustion horizon; intermittent CONNECT code5 investigation.
No Phase2 work or self-declared Phase1 PASS.

**A01 REMEDIATION IMPLEMENTED — READY FOR FINAL INDEPENDENT RECHECK**

## Independent Phase 1 final recheck — 2026-09-30

Independent recheck of the A01 remediation. No production code was modified,
no Phase 2 was begun, and no unrelated improvement was sought. Only this
section was appended.

Baseline: `git rev-parse HEAD` = `86da3c8dc71025278df66d588126cc0bef475b49`
(expected remediation SHA). `git status --short` shows only the pre-existing
modified `docs/audit/ARTITOR_0_3_PHASE1_INDEPENDENT_AUDIT.md` (prior re-audit
edits) and untracked `docs/audit/ARTITOR_0_3_CAPABILITY_AUDIT.md`; no
production/test/build modification. Production
`tor/src/commonMain/kotlin/com/yet/tor/ArtiTorClient.kt` SHA-256
`394c401a1c539f86424f928f9ad8e214d4a5987f774d4de6b1ffe821d73f92a9`
matches `git show HEAD:...`, so the reviewed tree is the remediation commit.

A01 CLOSED probe: re-ran the original stale-writer schedule independently
(session ACTIVE rev1; publisher A accepts ACTIVE rev2 and blocks immediately
before public publication; publisher B runs session.close() to CLOSED rev3,
publishes CLOSED/null, removes membership, returns; publisher A resumes) via
a temporary independent test class (4 tests, since removed) plus the
checked-in `a01CachedActiveCannotPublishAfterCompletedClose`. Collector
history is exactly `[ACTIVE, CLOSED]`; first CLOSED implies every later event
is CLOSED/null; final `status=CLOSED`, `socksEndpoint=null`,
`client.sessions` empty, `pending==0`. No `CLOSED → ACTIVE` observed.

A01 INVALIDATED probe: same schedule with shutdown/rebuild publishing
INVALIDATED rev3. History is exactly `[ACTIVE, INVALIDATED]`; first
INVALIDATED implies every later event is INVALIDATED/null; same final
conditions. No `INVALIDATED → ACTIVE` observed.

CAS fix: `publishLatestAccepted()` (ArtiTorClient.kt:233-250) reads
`target=accepted.value` and `observedPublic=publicStatus.value`, returns early
only when both pairs match, revalidates `accepted.value==target`, invokes the
null-by-default pre-CAS seam, then
`publicStatus.compareAndSet(observedPublic, target.status)`, and rechecks
`accepted.value==target` after success for synchronous collector reentry.
Once another writer has completed publishing CLOSED/INVALIDATED, public is
terminal/null while the stale writer's expected value is the pre-terminal
ACTIVE status, so its CAS necessarily fails; it reloads to the latched
terminal and returns without writing. Confirmed against the implementation,
not comments, and by the green probes plus the red mutation below.

Read→CAS boundary: the newer completed terminal publication changes the
expected public value, so an older writer blocked at the pre-CAS seam cannot
succeed after resuming. A terminal that is accepted-but-not-yet-published
while the old writer is still blocked is concurrent (not yet completed) work;
both writers then retry through the same loop and converge to the latched
terminal, which the probes assert. No post-completion resurrection schedule
was found.

F02 smoke: `delayedActiveAfterPausedIsRejectedByRevision` passes;
ACTIVE-rev1 delayed after completed PAUSED-rev2 leaves PAUSED/null observable.
Revision ordering was not weakened.

N01 smoke: public session status remains
`MutableStateFlow(...).asStateFlow()` (ArtiTorClient.kt:214-217); the exact
`onSubscription { calls++ }.first()` snippet asserts `calls==1`. No custom
StateFlow inheritance returned.

Reentrant collector smoke: `activePublicationCollectorReentersClose…`,
`activePublicationCollectorReentersPause…`, and
`auditNestedCollectorCanCloseWithoutDeadlock` all pass: no deadlock, no stale
resurrection, correct final membership, `pending==0`. The post-CAS
accepted-state check preserves these paths (publish holds no lock).

Mutation check (disposable copy only): in worktree
`/tmp/artitor-final-recheck-mut` (detached HEAD 86da3c8), replaced only
`publicStatus.compareAndSet(observedPublic, target.status)` with
`publicStatus.value = target.status`, leaving revalidation and convergence
intact. Both checked-in A01 regressions failed (2 tests/2 failures) with the
exact forbidden histories
`[ACTIVE(20001), CLOSED, ACTIVE(21001), CLOSED]` and
`[ACTIVE(20001), INVALIDATED, ACTIVE(21001), INVALIDATED]`.
Worktree removed afterward; main-repo production hash unchanged
(`394c401a…`), so the new tests prove the fix. Repository production was never
mutated.

Narrow source review (changed Kotlin publication logic + tests only): no
concrete new defect reproduced for stale-writer-after-terminal, CAS ABA with
observable change (equal statuses conflate without emission; terminal latch in
`accepted` blocks live re-acceptance), collector reentry, infinite
retry/livelock under finite writers (8-writer/256-callback test converges and
joins), or accepted/public permanent divergence (same-state revision advance
is intended conflation). No hypothetical race is reported without a schedule.

Verification (all at HEAD 86da3c8, normal source sets except the noted
disposable worktree): independent probe class 4/0 pass (temporary, removed);
targeted 7-test filter (A01×2, F02, N01, reentrant×3) 7/0;
`ArtiTorSessionConcurrencyTest` 30/0; full
`:tor:iosSimulatorArm64Test :tor:compileKotlinIosArm64
:tor:assembleAndroidDeviceTest` BUILD SUCCESSFUL in 2m26s with 76/0
(30 session, 24 lifecycle, 8 config, 6 invariants, 6 error mapping, 2 live;
live E2E 103s passed on the single attempt, no retry); `cargo test`
75 passed/0 failed; `git diff --check` exit 0. Production/build inputs are
byte-identical to the remediation commit for the reused build conclusion, as
verified by the hash above.

CONNECT code5: no SOCKS CONNECT failure was observed in this recheck's single
full run. The previously recorded intermittent `SOCKS CONNECT failed (code=5)`
with a successful retry remains of unknown cause and is recorded as a
reliability/release follow-up, not classified as fixed, environmental, caused
by A01, or a Phase-1 isolation defect.

Result:

```text
A01: CLOSED
F02: CLOSED
N01: CLOSED
N04: CLOSED
```

N04 remains CLOSED: this remediation touches only Kotlin session publication;
native typed-error/error-ordering paths are unchanged, and the native corpus
(75/0) plus `typedErrorCollectorShutdownFinishesFacadeOffWithNoLastError`
(pass, in the 30) show no regression. Remaining items (Android hardware
runtime; Phase-5 resource/cap measurements; N03 revision-exhaustion horizon;
CONNECT code5 investigation) are FOLLOW-UPS, not Phase-1 correctness
blockers. No production modification, no Phase 2.

```text
INDEPENDENT PHASE 1 FINAL RECHECK: PASS WITH FOLLOW-UPS
```


## Phase-1 closure record — 2026-09-30

ArtiTor 0.3 Phase 1: **CLOSED**

Accepted production SHA: `86da3c8dc71025278df66d588126cc0bef475b49`

Independent result: **PASS WITH FOLLOW-UPS**

The independent final recheck above is the acceptance evidence. Earlier
REMEDIATION REQUIRED verdicts remain historical evidence and are not rewritten.
The remaining items are follow-ups, not Phase-1 correctness blockers:

- Android hardware runtime verification.
- Phase-5 FD/memory/task measurement and session-cap reassessment.
- N03 session-revision exhaustion horizon.
- Intermittent SOCKS CONNECT code5 reliability investigation.

The subsequent Rust module extraction is a refactor-only pass. Phase 2 has
not started; bridges and lifecycle redesign are outside its scope.
