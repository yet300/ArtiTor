# ArtiTor 0.3 Phase 1 Implementation Report

Date: 2026-09-30 (Asia/Tbilisi)
Status: REMEDIATION IMPLEMENTED — READY FOR INDEPENDENT RE-AUDIT
Scope: Narrow recovery of the current Phase 1 working tree. No Phase 2 work.

The historical recovery evidence below predates the independent audit and is
retained as a record of that pass. It is not final Phase-1 acceptance. The
independent audit's reproduced F01–F14 findings are authoritative; the current
remediation section supersedes any historical completion claim.

## Changes and preserved behavior

The recovery preserves typed `PauseOutcome`, ERROR-to-session-PAUSED callbacks,
session invalidation on rebuild/cold recovery, the session listener barriers,
the ACTIVE/RUNNING dispatch gate, and atomic MAX_SESSIONS admission. Kotlin's
existing final wrapper reconciliation and terminal latch are preserved.

Two production defects were repaired in `rust/arti-kmp-ffi/src/lib.rs`:

1. **Synchronous replacement-start failure lost INVALIDATED callbacks.** Both
   the TorClient-defining rebuild branch and the cold/error recovery branch
   now save `spawn_cold()`'s result, release `ArtiTor.inner`, dispatch the
   collected INVALIDATED notifications, then return that result. A synchronous
   error therefore reaches the caller without leaving observers at PAUSED.
2. **Pause retained a stale published root endpoint.** The typed pause path
   now clears `bound_port` synchronously under the engine lock. The real
   occupied-port test exposed the stale port; the first live simulator run
   independently failed at resume with `AlreadyRunning`. Callback ordering
   and the typed outcome remain intact.

Test recovery in Rust:

- One `cfg(test) TestCommitGate` owns ARRIVED, RELEASE, and the bound port.
  The create hook publishes the port, signals ARRIVED, waits RELEASE, then
  performs the real registry commit.
- Deliberately blocked synchronous creates use `std::thread::spawn`.
  The test thread can release the gate independently of the Tokio runtime.
- `TestListenerProbe` observes the actual oneshot barrier receiver after the
  listener task polls it. An Empty receiver and a zero dispatch counter prove
  the barrier is closed; a TCP connection alone is never used as that proof.
- Engine re-entry listeners are installed with the real internal
  `Shared::set_listener` path before pause/shutdown. Both `on_status` and
  `on_log` synchronously call `has_client()` and `socks_port()`.
- The root-bind test occupies the exact port used by the real root listener,
  calls public `resume()`, joins its worker, and verifies typed Bind/ERROR.
  It uses no root-bind failure injection or Tor bootstrap.
- Publication tests keep a listener task and its shutdown sender under
  test ownership while the ACTIVE callback closes/pauses the session.
  This forces the resumed accept loop to exercise the dispatch gate, instead
  of allowing task abortion alone to explain a zero dispatch count.
- The cap tests hold all 40 creates after their initial cap check and before
  any final commit, then release them together. They use real OS threads.

Test recovery in Kotlin:

- `FakeSessionNative.sessionStatusBehavior` allows the snapshot read itself
  to emit CLOSED or INVALIDATED synchronously while returning older ACTIVE.
- Both terminal cases verify the returned wrapper, live registry removal,
  pending-status cleanup, and persistence of the terminal status after later
  stale live/terminal callbacks.
- The native sequential `initialization_terminal_race_deterministic` was
  removed. The decorative Kotlin terminal race was replaced. The root-bind
  and listener-before-commit tests were rewritten under their existing names.
  The close-only test is named `closedSessionNeverAppearsInSessions`.

No sleeps or elapsed-time assumptions were introduced in these recovery race
tests. Existing unrelated tests retain their existing bounded polling.

## Exact forced interleavings and assertions

For already-preserved production fixes, the last column describes the unsafe
behavior the test detects; it does not claim the starting working tree still
contained every historical production defect.

| Test | Forced interleaving | Exact assertion | Failure detected / weakness replaced |
|---|---|---|---|
| `create_vs_pause_race_deterministic` | Create binds and reaches ARRIVED with an empty registry; the independent test thread pauses; RELEASE permits final admission. | Creation succeeds as PAUSED/null; exactly one PAUSED callback, no ACTIVE callback; one live registry entry. | Committing the original RUNNING snapshot would publish ACTIVE after pause. The former Tokio-spawn/Condvar harness could block the thread needed to release it. |
| `create_vs_shutdown_race_deterministic` | Create binds and reaches ARRIVED; shutdown replaces the engine generation before RELEASE. | Exact `NotRunning` error; no callbacks from the uncommitted create; empty live registry. | Admission without generation revalidation could resurrect a session after shutdown. The former harness could deadlock before checking it. |
| `callback_reentry_does_not_deadlock` | Install ReentrantSessionListener through create_session; ACTIVE and later CLOSED callbacks synchronously call both engine getters. | Create returns an ACTIVE handle; close and shutdown return. | Calling either session callback under the engine lock deadlocks on getter re-entry. This is callback re-entry coverage, not an initialization terminal race. |
| `callback_reentry_pause_does_not_deadlock` | Install ReentrantEngineListener as the active listener; call pause; its status callback and pause log callback each synchronously call both engine getters. | Pause returns; status history is exactly PAUSED; the exact pause log is present; session snapshot is PAUSED. | A callback under `ArtiTor.inner` deadlocks on either getter. A locally constructed unused listener exercised none of this. |
| `callback_reentry_shutdown_does_not_deadlock` | Install ReentrantEngineListener; call shutdown; OFF callback synchronously calls both getters. | Shutdown returns; engine callback history is exactly OFF; retained session snapshot is INVALIDATED. | OFF publication under the engine lock deadlocks. The old unused listener could not detect it. |
| `session_paused_callback_reentry_does_not_deadlock` | Create with the active ReentrantSessionListener; pause emits PAUSED, whose callback calls both engine getters. | Pause returns; last session callback is exactly PAUSED/null; snapshot is PAUSED. | Dispatching session PAUSED while holding the engine lock deadlocks. |
| `session_invalidated_callback_reentry_does_not_deadlock` | Create with the active ReentrantSessionListener; shutdown emits INVALIDATED, whose callback calls both getters. | Shutdown returns; last callback is exactly INVALIDATED/null; handle snapshot is INVALIDATED. | Dispatching INVALIDATED while holding the engine lock deadlocks. |
| `resume_root_bind_failure_never_activates_sessions` | Run a real root SOCKS listener; freeze its chosen port as the configured resume port; create a session; pause and join root socket teardown; externally bind that exact port; call resume and join its worker. | Pause clears root endpoint; engine reaches ERROR; typed error is Bind for the exact port; no session ACTIVE callbacks after pause; session is PAUSED/null and root port is null. | An early session rebind could publish ACTIVE despite root failure. The stale root endpoint actually caused `AlreadyRunning` before bind. The former test never resumed or occupied a port. |
| `session_listener_does_not_dispatch_before_active_commit` | Create binds and blocks at ARRIVED before insertion; connect to its exposed port and send a full SOCKS greeting; inspect the polled, still-empty publication barrier; then RELEASE and join create. | Before release: zero dispatches, empty registry and callback history. After release: ACTIVE(endpoint) callback, SOCKS response `[0x05, 0x00]`, exactly one dispatch. | Early barrier release violates the barrier assertion; a disabled listener cannot satisfy the positive continuation. A connect after ACTIVE return proved no precommit property. |
| `create_active_callback_close_precedes_barrier_and_rejects_dispatch` | During creation's ACTIVE callback, verify registry ACTIVE/port and pending barrier, queue SOCKS bytes, keep listener task runnable, and close the session. Return permits barrier release and accept. | Callback sees committed ACTIVE and unreleased barrier; after accept: zero dispatches, CLOSED/null, exact ACTIVE→CLOSED history. | Release-before-publication fails the receiver assertion. Dispatch without live registry validation could accept traffic for a closed session. |
| `create_active_callback_pause_precedes_barrier_and_rejects_dispatch` | Same creation callback interleaving, but synchronously pause the engine and retain the listener task. | Callback sees committed ACTIVE and pending barrier; accepted traffic is checked with zero dispatches; PAUSED/null; exact ACTIVE→PAUSED history. | Release-before-publication fails. Removing ACTIVE/RUNNING validation produces one dispatch after pause. |
| `rebind_active_callback_close_precedes_barrier_and_rejects_dispatch` | Create PAUSED; arm the running harness and invoke real rebind; ACTIVE callback observes committed endpoint and pending barrier, queues traffic, and closes while retaining the listener task. | Zero dispatches after accept; CLOSED/null; exact PAUSED→ACTIVE→CLOSED history. | Rebind barrier release before ACTIVE publication fails. A missing-entry dispatch could resurrect closed-session traffic. |
| `rebind_active_callback_pause_precedes_barrier_and_rejects_dispatch` | Same rebind interleaving, but ACTIVE callback pauses the engine while keeping the listener runnable. | Zero dispatches after accept; PAUSED/null; exact PAUSED→ACTIVE→PAUSED history. | Early release fails. Removing ACTIVE/RUNNING validation produces one dispatch after callback pause. |
| `concurrent_create_at_max_limit_is_atomic` | All 40 OS-thread creates reach the gate after their initial cap check against an empty registry; only then RELEASE permits final commits. | Exactly 32 successes and eight `Runtime("session limit reached")` errors; registry has exactly 32 ACTIVE entries. | Check-before-insert without a final atomic cap check admits more than 32. The old default-runtime async tasks executed synchronous creates serially. |
| `concurrent_create_paused_at_max_limit_is_atomic` | Pause first; all 40 creates reach the same gate before any insertion; release all final commits. | Exactly 32 successes, eight exact limit errors, and 32 PAUSED registry entries. | A PAUSED insertion without atomic admission exceeds the cap; the old async runner did not force competing final admissions. |
| `rebuild_spawn_cold_failure_still_notifies_invalidated` | Create and pause; change data_dir; inject synchronous spawn_cold Runtime error at function entry before any replacement work is spawned. | Original error is returned; old handle is INVALIDATED/null; registry is empty; exactly one INVALIDATED/null callback is recorded. | The original `?` returned before dispatching pending notifications, leaving the last callback PAUSED/null. |
| `cold_error_recovery_spawn_cold_failure_still_notifies_invalidated` | Create and pause; model ERROR with absent root client/bootstrap flag false; inject the same synchronous cold-start error. | Same exact error, terminal handle, empty registry, and exactly one INVALIDATED/null callback. | The second `?` path lost notifications independently of the rebuild branch. |
| `closedCallbackDuringOlderActiveSnapshotIsReconciledAndLatched` | Native create returns a handle without an initial callback; inside the first sessionStatus read, emit CLOSED and then return the saved older ACTIVE snapshot; facade constructs and finally reconciles the wrapper. | One snapshot read; returned wrapper CLOSED/null; absent from client.sessions; pending status consumed; later ACTIVE/PAUSED/INVALIDATED callbacks cannot change the wrapper. | Omitting final acceptStatus leaves the returned wrapper ACTIVE despite registry removal. Sequential create→close never exercised this interval. |
| `invalidatedCallbackDuringOlderActiveSnapshotIsReconciledAndLatched` | Identical snapshot interval, emitting INVALIDATED instead of CLOSED. | One snapshot read; wrapper INVALIDATED/null; absent from live registry; pending status consumed; terminal state persists after stale callbacks. | Omitting final acceptStatus leaves ACTIVE. The previous sequential test could not establish initialization reconciliation. |
| `callbackBeforeWrapperRegistrationIsReconciled` | Fake createSession emits ACTIVE synchronously before it returns the native handle and before wrapper construction. | Returned wrapper is ACTIVE and is contained in client.sessions. | Discarding early callbacks would lose initial status publication. This test covers an early ACTIVE callback; it is not claimed as a terminal snapshot race. |

## Verification evidence

Commands below were awaited to process exit before counts or outcomes were
recorded. Rust socket tests require permission to bind loopback sockets outside
the filesystem sandbox; its `Operation not permitted` setup failure is not
counted as a successful regression reproduction.

### Production regressions before repair

- `rtk proxy cargo test --manifest-path rust/arti-kmp-ffi/Cargo.toml spawn_cold_failure -- --test-threads=1 --nocapture`: exit 101, **0 passed, 2 failed**. Both injected synchronous failures lost the INVALIDATED callback.
- `rtk proxy cargo test --manifest-path rust/arti-kmp-ffi/Cargo.toml resume_root_bind_failure -- --nocapture`: exit 101, **0 passed, 1 failed**. The deterministic pause assertion observed `Some(port)` where `None` was required.
- First `rtk ./gradlew :tor:iosSimulatorArm64Test`: exit 1, **54 tests, 1 failed**. Live root SOCKS HTTP succeeded, then resume failed with `AlreadyRunning`. This failure was repaired, not excluded from the final full run.

### Checks that the new tests catch the intended defects

Disposable copies under `/private/tmp` were mutated; no preserved working-tree
production fix was reverted:

| Deliberate mutation | Command/filter | Finished outcome |
|---|---|---|
| Move both creation and rebind barrier release before ACTIVE callback. | `rtk proxy cargo test --manifest-path /private/tmp/artitor-recovery-mutant/Cargo.toml --target-dir rust/arti-kmp-ffi/target active_callback -- --test-threads=1 --nocapture` | Exit 101; **0 passed, 4 failed** with “listener barrier released before ACTIVE callback completed.” |
| Remove session ACTIVE and engine RUNNING dispatch checks. | Same disposable-crate command, filter `active_callback_pause`. | Exit 101; **0 passed, 2 failed**; actual dispatch counter 1, expected 0, for both create and rebind. |
| Restore both early-return `spawn_cold(...)?` paths. | Same disposable-crate command, filter `spawn_cold_failure`. | Exit 101; **0 passed, 2 failed**; both last callbacks explicitly recorded as PAUSED/null. |
| Remove `wrapper.acceptStatus(finalStatus)` from copied Kotlin source while retaining generated UniFFI sources. | `rtk ./gradlew --init-script /private/tmp/artitor_kotlin_mutant.init.gradle :tor:iosSimulatorArm64Test --tests 'com.yet.tor.ArtiTorSessionConcurrencyTest'` | Exit 1; **9 executed, 2 failed**: expected CLOSED/INVALIDATED, actual ACTIVE. |

The first Kotlin mutation harness mistakenly omitted generated common sources
and failed compilation; that attempt is not counted as test evidence. The
corrected source routing compiled and produced the two assertion failures above.
Sharing Cargo's target directory also caused one subsequent working-tree command
to run a cached mutant binary (50 passed, 2 failed). The source was unchanged;
the final native result below followed an explicit working-tree source rebuild.
The final Gradle run used normal source routing without the mutation init script.

### Final working-tree results

| Command | Completed result |
|---|---|
| `rtk proxy cargo test --manifest-path rust/arti-kmp-ffi/Cargo.toml` | Exit 0 after explicit source rebuild; **52 passed, 0 failed, 0 ignored** under the default parallel runner; doc tests: 0. |
| `rtk ./gradlew :tor:iosSimulatorArm64Test` | Exit 0, **BUILD SUCCESSFUL in 1m 14s**; final XML: **54 tests, 0 failures, 0 errors, 0 skipped**. |
| `rtk proxy rustfmt --edition 2021 --check rust/arti-kmp-ffi/src/lib.rs` | Exit 0, no formatting differences in the edited native file. |
| `rtk git diff --check` | Exit 0, no patch whitespace errors. |

Final simulator counts from the XML produced by that completed run:

| Suite | Executed | Failed / skipped |
|---|---:|---:|
| ArtiTorClientLifecycleTest | 24 | 0 / 0 |
| ArtiTorSessionConcurrencyTest | 9 | 0 / 0 |
| ConfigIdentityTest | 8 | 0 / 0 |
| ErrorMappingTest | 6 | 0 / 0 |
| LifecycleInvariantTest | 6 | 0 / 0 |
| TorIosE2ETest | 1 | 0 / 0 |
| **Total** | **54** | **0 / 0** |

The final live E2E output contains a real SOCKS HTTP 200 response and
`SECOND COLD START ok`. The test also asserts HTTP after resume, readiness
after both starts, and final OFF/no-client state. Kotlin reconciliation tests
are included in the nine session tests, not counted separately again.

The crate-wide `rtk proxy cargo fmt --manifest-path rust/arti-kmp-ffi/Cargo.toml -- --check`
also found pre-existing formatting differences in `examples/host_poc.rs`.
That unrelated example was left intact; the edited Rust source passes its
scoped formatting check. Gradle emits existing warnings about disabled Android
host tests, deprecated Gradle features, and deprecated `CStructVar.Type` use
in the live iOS test.

## Limits and follow-ups

- Native race tests use real loopback sockets and unbootstrapped Arti clients.
  They verify registry, callback, barrier, admission, and dispatch behavior
  without a Tor network bootstrap.
- The simulator E2E exercises the root endpoint through Tor, including
  pause/resume and a second cold start. It does not establish distinct live
  circuits for multiple isolation sessions.
- Android device execution and live multi-session Tor traffic remain release
  follow-ups; this recovery pass does not claim those results.
- Existing Phase 1 constraints remain: one supported live client per process,
  ephemeral session ports, and rebind retry through a normal pause/resume or
  close/recreate cycle.

Confidence: **high** for the completed native/Kotlin recovery checks and
their forced interleavings; Android device and live multi-session behavior
remain unverified in this pass.

Historical recovery verdict (superseded by independent audit):
`PHASE 1: PASS WITH FOLLOW-UPS`


## Independent Audit Remediation

Date: 2026-09-30 (Asia/Tbilisi). Scope remains Phase 1. The original independent
findings are preserved in `ARTITOR_0_3_PHASE1_INDEPENDENT_AUDIT.md`; that report
receives only a short remediation-status addendum. No Phase-2 features,
Cargo features, or new public error taxonomy are authorized by this pass.

The completed results below describe remediation evidence, not final Phase-1
acceptance. Native deterministic regressions passed in both default-parallel
and serial runs; the final native run passed 70 tests. Kotlin session regressions
passed 20/20. Additional live repeats are recorded separately so individual failures cannot be hidden.

| Finding | Fix / explicit defer | Test evidence | Mutation evidence | Final disposition |
|---|---|---|---|---|
| F01 — generated FFI exception leak | Map synchronous session creation errors through existing public mapper. | Exact public class assertions for every generated variant; targeted 20-test session suite passes. Earlier 13-test baseline had four expected F01/F07 failures. | Exact-class assertions reproduce the leak in the baseline. | Implemented; ready for independent re-audit. |
| F02 — stale session publication | Revision every registry state/endpoint transition; internal callback carries revision; atomic wrapper record rejects older/equal revisions and latches terminal. | `delayed_active_publication_carries_older_revision_than_pause`; Kotlin `delayedActiveAfterPausedIsRejectedByRevision`, concurrent revision/StateFlow and terminal regressions pass. | Removing wrapper revision guard fails: expected PAUSED, actual ACTIVE. Restored 20-test suite passes. | Implemented; ready for independent re-audit. |
| F03 — lifecycle check/use race | Shared transition gate makes authoritative lifecycle/client epoch validation and registry mutation atomic; callbacks run after unlock. | `final_create_decision_rejects_completed_error` and `final_rebind_decision_rejects_completed_pause` pass at final transaction boundary. | Removing transition locking fails explicit final-transaction gate-ownership assertions. This is structural atomicity evidence, not a claimed stale-state mutation reproduction. | Implemented; ready for independent re-audit. |
| F04 — same-Shared root replacement | Internal root-client epoch advances on replacement/discard; old-root admission fails existing NotRunning. | `audit_create_crosses_failed_rebuild_without_generation_barrier` passes: failed rebuild rejects old-root create; strengthened final assertions verify empty registry, hasClient false, no ACTIVE/PAUSED callbacks, and unchanged engine generation. Targeted rerun passed after final 70-test suite. | Removing epoch validation fails: old-root create returns Ok/PAUSED. | Implemented; ready for independent re-audit. |
| F05 — ERROR publication ordering | Demote sessions under gate; after unlock publish session PAUSED, typed error, engine ERROR, diagnostic log. | `audit_error_callbacks_must_observe_demoted_sessions` passes for reentrant typed-error/status observers. | Ordered publication is additionally covered by F14's reversal mutation. | Implemented; ready for independent re-audit. |
| F06 — ERROR recovery contradiction | Explicit resume may retain identity; start from ERROR invalidates sessions and advances epoch even for identical config and retained bootstrapped root. | `audit_start_after_error_must_invalidate_retained_sessions` passes. | No additional mutation claimed. | Implemented and freeze amended; ready for independent re-audit. |
| F07 — historical Kotlin pending statuses | Pending events only within serialized in-flight creation; reconciliation consumes current event and clears orphan ids; late unknown callbacks ignored. | 160 terminal-before-wrapper and 160 late-after-close churn, orphan/failure cleanup, stale terminal/new-wrapper and snapshot reconciliation cases pass in 20-test session suite. | Baseline reproduces F07 failures; no separate pending-map mutation claimed. | Implemented; ready for independent re-audit. |
| F08 — fatal session accept error | Current-listener identity guards ACTIVE→PAUSED/null on unexpected accept error; stale listener cannot alter newer state or root. | `fatal_accept_error_demotes_only_current_listener` injects actual accept branch; `stale_listener_failure_cannot_demote_rebound_closed_or_invalidated_session` passes. | No additional mutation claimed. | Implemented; ready for independent re-audit. |
| F09 — terminal tombstones | Bounded native diagnostics may degrade stale CLOSED snapshot to INVALIDATED after pruning; public wrapper terminal latch is permanent. | `native_tombstones_are_bounded_diagnostics` and Kotlin CLOSED-survives-pruning regression pass. | Not applicable to accepted contract amendment. | Contract and comments amended; regression verified. |
| F10 — normative mismatch | Freeze now has no-timeout production signature, opaque non-UUID ids, coherent ERROR paths, callback reentry, and pause→resume/close-recreate retry. | Freeze cross-check against production surface and lifecycle tests completed. | Not applicable to documentation amendment. | Contract synchronized. |
| F11 — unmeasured cap | 32 stays private, conservative resource-accounting safety cap; no target measurement claim. | Atomic cap regressions pass; Phase-5 incremental FD/memory/task measurement on Android device and iOS device/simulator where available is normative. | Not applicable to accepted evidence correction. | Empirical cap assessment deferred to Phase-5 release gate; bounded Phase-1 admission verified. |
| F12 — isolated dispatch identity | Test-only actual dispatch handle probe distinguishes root, session A, session B. | `actual_root_and_two_session_dispatches_select_distinct_client_handles` passes alongside audited upstream isolated_client semantics. | Substituting shared root client for entry.isolated fails: all three dispatch pointers become root. | Implemented; ready for independent re-audit. |
| F13 — inherited SOCKS framing | Shared CONNECT-only handler reads exact frames, validates no-auth/header fields, preserves Tor hostname dispatch. | Six `socks::tests` wire tests plus the safe-diagnostic test pass, covering fragmentation, one-char hostname, IPv4/IPv6, unsupported auth, malformed headers/commands/hostnames. | No additional mutation claimed. | Implemented; ready for independent re-audit. |
| F14 — ineffective error-order test | One ordered event sequence replaces separate status/error vectors. | `notify_error_delivers_typed_detail_before_status` requires Error(kind) immediately before Status(ERROR). | Reversing typed/status calls fails with `[Status(Error), Error(Bootstrap)]`. | Implemented; ready for independent re-audit. |

F11's empirical cap selection is a Phase-5 device/resource acceptance item,
not a deferral of bounded Phase-1 admission. Each session consumes a listener,
accept task, connection tasks/FDs, handle memory, and partitioned circuit demand;
resource accounting supports a conservative private bound without proving that
32 is optimal on a device. Release remains gated on recorded incremental
measurements and reassessment of that value. F09 and F10 resolve contract
contradictions explicitly rather than postponing them.

### Completed remediation verification

| Command / execution | Completed result |
|---|---|
| `rtk proxy cargo test --manifest-path rust/arti-kmp-ffi/Cargo.toml` with loopback permission | Final run: 70 passed, 0 failed. Earlier 69-test default-parallel and serial runs both passed. |
| Targeted `:tor:iosSimulatorArm64Test --tests 'com.yet.tor.ArtiTorSessionConcurrencyTest*'` | 20 passed, 0 failed; after stale-revision mutation restoration, full 20-test session suite passed again. |
| `rtk ./gradlew :tor:iosSimulatorArm64Test :tor:compileKotlinIosArm64 :tor:assembleAndroidDeviceTest` | Exit 0; BUILD SUCCESSFUL in 2m 12s. XML: 65 tests, 0 failures/errors/skips (20 session, 24 lifecycle, 8 config, 6 invariant, 6 error, 1 live). |
| Repeated full platform command with checked-in live two-session test | Exit 0; BUILD SUCCESSFUL in 3m 46s. XML: 66 tests, 0 failures/errors/skips; `liveTwoSessionsLifecycle` 87.304s and `bootstrapFetchPauseResume` 68.206s both passed. Saved XML: `/private/tmp/artitor-remediation-results/full-platform-live-run-2`. |
| `rtk ./gradlew :tor:iosSimulatorArm64Test --tests 'com.yet.tor.TorIosE2ETest*'`, targeted live repeat 3 | Exit 0; BUILD SUCCESSFUL in 2m 45s. XML: 2 tests, 0 failures/errors/skips; `liveTwoSessionsLifecycle` 68.262s and `bootstrapFetchPauseResume` 54.605s both passed. Saved XML: `/private/tmp/artitor-remediation-results/live-repeat-run-3`. |
| `audit_create_crosses_failed_rebuild_without_generation_barrier`, strengthened final assertions | Targeted rerun passed after final 70-test native suite: empty registry, no retained root client, no ACTIVE/PAUSED callback, unchanged engine generation. |
| Native epoch/gate/dispatch/error-order mutation crates | Expected regression failures above; distinct package identities under `/private/tmp/artitor-remediation-mutations` share dependency target cache. |
| Kotlin stale-revision mutation | Expected single failure: PAUSED versus ACTIVE; guard restored and 20-test suite green. |

The first full remediation live root run passed, including HTTP 200 and
second cold start. The second full run passed root and two-session live tests.
The third targeted repeat passed both root and two-session live tests. All
three post-remediation live attempts completed without observed failures. No
SOCKS code-5 safe diagnostic was emitted during these successful runs.
The independently observed earlier SOCKS code 5 retains **unknown** root cause; a passing retry
cannot relabel it as environmental noise or proven regression.

Safe connect diagnostics record the Arti error category rather than target
hostname or sensitive configuration.
`socks::tests::safe_diagnostic_omits_target_and_upstream_display` passes using a
real Arti InvalidStreamTarget error and asserts omission of target/upstream
display text. Android device runtime and Phase-5 cap measurements remain
unverified release follow-ups. Original Apple SQLite mitigation is preserved.

`rtk git diff --check` passes. The remediation SHA is supplied in the completion
response, avoiding embedding a circular commit identity in its own content.
The scoped remediation files are:

- `rust/arti-kmp-ffi/src/lib.rs`
- `rust/arti-kmp-ffi/src/socks.rs`
- `tor/src/commonMain/kotlin/com/yet/tor/ArtiTorClient.kt`
- `tor/src/commonTest/kotlin/com/yet/tor/ArtiTorClientLifecycleTest.kt`
- `tor/src/commonTest/kotlin/com/yet/tor/ArtiTorSessionConcurrencyTest.kt`
- `tor/src/iosTest/kotlin/com/yet/tor/TorIosE2ETest.kt`
- `docs/design/ARTITOR_0_3_API_FREEZE.md`
- `docs/audit/ARTITOR_0_3_PHASE1_IMPLEMENTATION_REPORT.md`
- `docs/audit/ARTITOR_0_3_PHASE1_INDEPENDENT_AUDIT.md` (addendum only)

The pre-existing untracked capability audit is unedited and outside the
remediation commit. This report does not declare final Phase-1 acceptance.
Confidence: **high** for completed deterministic remediation regressions and
listed mutation evidence; **unknown** for the earlier intermittent live root
failure's cause and unexecuted device/resource follow-ups.

**REMEDIATION IMPLEMENTED — READY FOR INDEPENDENT RE-AUDIT**
