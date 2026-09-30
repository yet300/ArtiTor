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
