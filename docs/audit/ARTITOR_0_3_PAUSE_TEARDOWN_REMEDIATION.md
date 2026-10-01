# ArtiTor 0.3 synchronous listener teardown remediation

Date: 2026-10-01, Asia/Tbilisi. Baseline: `e75336411a7eaadda4a2c828d668cd373dc6b0e1`.

**PAUSE TEARDOWN REMEDIATION: PASS. IMPLEMENTATION BLOCKERS: NONE. RELEASE BLOCKERS: NONE. ARTITOR 0.3: READY TO TAG.**

Confidence is **high** for the deterministic native, Pixel local and final minified artifact results recorded here. Remote historical failure causes remain **unknown**. No tag or remote publication was created.

## Original failure and inspected ownership

The preserved reliability investigation observed TCP acceptance on old root port 41957 after pause returned with root/session PAUSED, root publication cleared and client retained. No resume occurred between pause return and the probe. Its disposable callback-held offline diagnostic reproduced that gap in **10/10** schedules; releasing the callback finally allowed the listener to drop. These historical results remain in [the reliability disposition](ARTITOR_0_3_ANDROID_REQUEST_RELIABILITY_DISPOSITION.md) and its original evidence directory. They establish a physical listener destruction gap, not successful paused Tor forwarding, and are not attributed as the cause of earlier remote HTTPS failures.

Source inspection before the ownership change covered all synchronous teardown paths:

| Path | Previous listener owner | Previous cancellation | Physical-close completion before return/publication? |
|---|---|---|---|
| Root pause | Root worker's local Tokio TcpListener | Shutdown channel + worker abort | No: held synchronous callback prevents stack destruction |
| Session pause | Listener task's captured std/Tokio listener, including pre-publication barrier | Session shutdown + listener-task abort | No: runtime scheduling can postpone destruction |
| Session close | Same session task | Remove registry entry, abort listener/connections | No: removing a JoinHandle does not drop its task stack |
| Shutdown | Root/session task stacks | Abort all, runtime shutdown_background | No synchronous socket destruction proof |
| Restart/replacement | Shutdown plus start; replacement invalidates old sessions | Same abort helpers; old root already paused/error-demoted in accepted paths | Session task-stack destruction could lag invalidation |
| Session listener failure | Current listener task | Exact generation/state/revision match, abort_all, PAUSED callback | Socket lived until task unwind, potentially past callback |

The frozen contract requires TCP connect itself to fail after synchronous teardown returns. A failed SOCKS greeting on an accepted socket is insufficient. Resume remains root-readiness based, with optional session activation asynchronous.

## Small ownership change and proof

`ListeningSocket` owns the **only** physical Tokio listener in `Mutex<Option<TcpListener>>`. Root Shared and each live SessionRuntime retain an Arc to this controller. The accept future uses `poll_accept` under its leaf mutex, releases that mutex at each poll return, and retains no borrowed TcpListener across Pending. No duplicate socket/FD is created.

`close()` takes and drops that sole listener synchronously. Method return is the destruction acknowledgement: there is no task join, block_on, runtime-progress dependency, completion timeout, sleep, poll-until-finished loop, or connection retry in production. Old worker/task Arcs retain only an empty controller. A root task-exit guard also closes its exact controller on ordinary exit/cancellation, preserving the previous stack-drop cleanup even while Shared retains the controller.

Root bind and controller registration are indivisible against teardown: under transition_gate the worker first checks its revision, binds the numeric loopback address with std::net, sets nonblocking, registers it with Tokio and installs the controller. There is no await or callback in that short transaction. Only then does the unchanged listening-log callback run outside the gate; the original second revision check subsequently governs RUNNING publication. This preserves log-before-status order and prevents cancellation from missing a newly rebound known fixed port.

Root shutdown signalling first closes the root controller. Session `abort_all` first closes its controller. Existing channel signalling and abort of listener/worker/tracked connection tasks remain. Those central helpers cover pause, close, shutdown, ERROR demotion, invalidation/replacement and current-listener failure. Admission/rebind create the controller before ACTIVE publication and close rejected/uncommitted sockets explicitly. The std-to-Tokio conversion runs in the existing runtime's entered context before handing the controller to the task.

### Reentry, ordering and locks

Chosen order: close physical listeners during the authoritative lifecycle transaction, preserve tracked-task cancellation and state/revision mutations, release transaction/registry locks, then deliver existing callbacks with existing freshness checks. Thus PAUSED/CLOSED/INVALIDATED callbacks see the completed physical stop. The required guarantee remains method-return completion; no all-session resume barrier is added.

Lock order is existing inner → transition_gate → session/state locks → socket controller. The controller is a **leaf**: its critical section only takes/drops the listener or performs nonblocking `poll_accept`; it never invokes callbacks, acquires an authority/registry lock, awaits, or waits for task destruction. The accept result releases the controller lock before dispatch takes transition_gate/registry. Therefore there is no reverse controller → authority edge and no completion dependency on a held engine lock. A detach/unlock/join phase is unnecessary because closure is direct resource destruction, not waiting for another task to acquire lifecycle locks.

A callback can synchronously call native pause: the frozen surface permits reentry and has no prohibition on lifecycle calls. That callback's stack does not own the listener anymore. Even a single runtime worker held inside a callback cannot prevent another thread from closing the controller. Callback-triggered pause, shutdown and session close therefore cannot self-join. Original callback publication/revision fences, worker_revision, engine_publication_revision, client_epoch, generation and session listener revision checks remain unchanged. Accepted connections remain tracked and aborted; stale dispatch still checks lifecycle/revision authority.

Public Kotlin/UniFFI API, ArtiConfig, TorErrorKind, Cargo features, Android consumer rules, bridge/onion policy, timeouts and Apple deployment metadata: **no delta**. No production logging message or policy was added. No tag, push or remote publication is performed.

## Permanent native regressions and mutation

The new offline tests use a retained unbootstrapped client, real loopback listeners, actual lifecycle methods, explicit callback latches/release and bounded watchdogs. A single-worker runtime ensures listener-task abort cannot silently finish elsewhere while the root callback is held. Test fixture Drop releases the callback and shuts down even after assertion failure.

Before production edits, **all five** initial invariants failed with `TcpStream::connect_timeout` returning **Ok** after teardown returned: root pause, session pause, close, shutdown and restart. This RED output is preserved. The permanent tests assert immediate ConnectionRefused, rather than passing when a defect is reproduced.

After the change:

- Root callback-held pause: **10/10 immediate TCP refusals**, zero premature accept-capable returns; callback is still held at each assertion.
- Session callback-held-runtime pause: old A/B TCP refusal, PAUSED/null, identical retained isolated-client Arc.
- Close: old A refuses; root and B still connect, root RUNNING/B ACTIVE, A CLOSED/null; second close is a no-op.
- Shutdown: old root/A/B refuse, engine OFF, no client, old sessions INVALIDATED.
- Restart: teardown refusal checked before new binds, new generation/id and fresh endpoint work, stale handle remains invalid and cannot affect fresh session. No assumption that OS ephemeral ports can never be reused is made.
- Root/session callbacks reenter getters; root callback invokes pause/shutdown/close; CLOSED session callback invokes shutdown. All complete under bounded watchdogs.
- Normal root worker exit: join confirms actual task completion, and old TCP refuses even with its controller retained. This test first failed before the task-exit guard was added.

The four existing pre-publication callback/dispatch tests remain. Their deliberately retained task harness now also detaches/retains the physical controller, allowing queued traffic to reach the dispatch fence despite close/pause. This artificial ownership bypass is confined to tests; zero dispatch, ACTIVE-before-callback, callback order and revision assertions are retained. Ordinary teardown regressions separately require physical refusal.

Disposable mutation empties `ListeningSocket::close` while retaining channel/abort cancellation. Root and session pause, close, shutdown and restart regressions fail again. Final-source isolated-cache mutation results are recorded in the evidence bundle. No authoritative source is mutated.

### Independent review and additional RED schedule

A focused independent review using the requesting-code-review skill found one Important issue in the first controller implementation: its listening-log callback still preceded registration in Shared. Public resume onto the previous configured root port, with that log callback held, allowed pause to return while that known old endpoint accepted TCP. The new `fixed_port_resume_log_held_pause_and_shutdown_close_tcp` regression reproduced the finding before its production correction (`root-log-held-red.txt`).

Bind/registration now occur in the same revision-checked transaction described above. Both pause and shutdown refuse the old configured TCP port while the log callback remains held. Another test invokes pause/shutdown directly inside the listening-log callback, checks refusal before callback return, reenters getters and rejects any stale RUNNING publication. The reviewer rechecked the change and reported the finding resolved, with no remaining actionable Critical/Important/Minor findings (read-only code review, not an independent rerun of hardware evidence).

The final reviewed-source skip-close mutation produces **8 failures / 1 pass / 97 filtered**: all physical teardown, normal root exit and both log schedules fail. The callback control may finish its worker naturally and pass; that does not waive the deterministic held-callback failures. Its target cache is isolated from the authoritative crate.

Preparation failures are preserved: the first new restart test used the async-client fixture outside a Tokio context, then was corrected to a Tokio test; a callback test initially expected a native CLOSED tombstone after allowing the old Shared to die, then retained Shared explicitly (public permanent terminal latching remains separately tested). An early serial invocation reused the mutation's shared target binary, producing its five expected failures; only crate artifacts were cleaned and authoritative source rebuilt. The final mutation uses a separate copy-on-write target cache to prevent this tooling collision.

## Native and cross-platform verification

- Cargo fmt check and source/report diff whitespace check: PASS. Raw captured logs intentionally retain tool-emitted trailing spaces/blank final lines. Scoped evidence attributes preserve XML CRLF bytes so committed evidence matches its hashes.
- Final Rust parallel: **106 PASS**, zero fail/ignored, 0.17 s test execution.
- Final Rust serial: **106 PASS**, zero fail/ignored, 0.74 s test execution.
- First required release bindings/iOS-arm64 compile/Android assembly/iOS simulator command: BUILD SUCCESSFUL; **103 iOS tests PASS**, including both live tests and the untouched pre-existing four-test independent audit probe. No unchanged remote retry was needed.
- Final-source rebuild after adding normal root-exit cleanup: BUILD SUCCESSFUL; **103 iOS tests PASS** again. This is source-change verification, not replacement of a first external failure.
- Reviewed-source rebuild after closing the listening-log window: BUILD SUCCESSFUL; **103 iOS tests PASS**, including both live tests, zero failures/skips. Earlier 104/105-test Rust and platform passes remain historical evidence; final 106-test logs are `rust-parallel-reviewed-complete.txt` and `rust-serial-reviewed.txt`.

## Pixel local gate before external CONNECT

Physical Google Pixel 6a, serial 26101JEGR21458, Android 17/API 37, arm64-v8a, paired wireless ADB. `AndroidPauseTeardownProbe` sends only local SOCKS greetings and TCP negative probes; bootstrap traffic is necessary to retain the client, but no target CONNECT/DNS is sent by the local phases.

The first invocation produced JUnit initializationError (the Kotlin expression inferred an Int return from Log.i). No lifecycle probe executed. Explicit Unit return corrected the test; both results remain separate.

Corrected final-native result: **one test PASS**, 100/100 pause cycles; **200/200 paused old root/session ECONNREFUSED**, **100/100 closed-session old-port refusals**, **2/2 shutdown old-port refusals**, **zero accepted old endpoints**. Root greetings 151/151, created-session greetings 100/100, resumed-session greetings 100/100, fresh-restart session greeting 1/1. State/client/null-endpoint and terminal-handle assertions pass. Resume awaits the session's own ACTIVE/current endpoint.

Pause diagnostic latency across 100 calls: min **2.156372 ms**, median sample **2.394327 ms**, max **3.548096 ms**. These include facade/native execution and are observations, not a performance guarantee.

Reviewed-source repeat: **100/100 cycles PASS**, again **200 paused old refusals / 100 close refusals / 2 shutdown refusals / zero accepted old ports**, and the same 151/100/100/1 successful greeting counts. Its one hardware test passes; evidence is separate in `android-local-reviewed/` and `android-local-reviewed-logcat.txt` (PID 10911). Final-source pause latency: min **2.035604 ms**, median sample **2.609579 ms**, max **10.782959 ms**. The previous pass above is preserved as an intermediate-source result.

## Maven consumer, connected suite and release decision

Intermediate-source artifact results are preserved: release-only Maven-local publication PASS, fresh source/config-only consumer build PASS, FieldOrder **17 surviving / 18 original**, construction/callback local diagnostic PASS with **100 cycles / 200 old-port refusals / zero accepts**, roots 152, created sessions 100, resumed sessions 100, cold starts 3. It sends no external CONNECT matrix in local-only mode. Its full integrated **first run PASS**, 86 assertions including **36 immediate ECONNREFUSED** probes. Cold 8373 ms, resume 4 ms, restart 7310 ms; pause interval 11 ms includes assertion/probe overhead. All accepted traffic/onion/config/shutdown/restart/error phases execute. The first legacy connected suite after that source passes **3/3**, zero failures/skips, Gradle 4m; no unchanged diagnostic retry.

The independent review correction changes production afterward, so those passes are not substituted for final-source artifact verification. Final reviewed-source release Maven-local publication: **PASS** (31 s). Fresh Maven-only minified consumer: **PASS**, 48 executed tasks, 44 s; JNA FieldOrder **17 surviving / 18 original**, all surviving metadata verified. No additional keep rule, dependency override or checkout-native injection.

Final artifact local-only first run: **PASS**, 100 cycles, **200 immediate old root/session ECONNREFUSED / zero accepts**, root greetings 152, created sessions 100, resumed sessions 100, cold starts 3. No external CONNECT matrix was run in this phase. Final integrated first run: **PASS**, **86 assertions / 36 immediate ECONNREFUSED** probes, covering cold start, root/A/B traffic, close A, pause/resume and B ACTIVE traffic, onion, invalid-config preservation, shutdown, restart/fresh traffic, policy negatives and asynchronous callbacks. Cold 8556 ms, resume 5 ms, restart 7451 ms; measured pause interval 8 ms includes assertion/probe overhead. These are diagnostic observations, not latency guarantees.

Installed APK SHA256: `67cfb9aa9de1b88cec794813774d5c76214958bf54d2de6e250ce2923e4bb014`, identical to the freshly built APK. Maven Android AAR SHA256: `149a34bd965a7ec86c56732fadc81311ec6393224bd7666a1d72b6bb124ba15d`. AAR native library matches the reviewed release build (`316da860ccf7b6b711f9b2a091f19cd57928608060f39af48a51852ba85200bd`); APK native library matches that build after standard strip (`ca9ab4fad658830a8616f038e0d98a08fafe32804725d94092460158beaa1c17`). Full hash chain is in `artifact-provenance.json`.

Short final artifact Logcat smoke: public first-process control present, nonempty PID 11736 diagnostic-tag capture; synthetic bridge IP/fingerprint/transport markers absent. Sixty first/last lines are retained with counts in `redaction-smoke-reviewed.json`. Earlier first-process privacy closure remains preserved. Logging content and policy did not change.

Final-source legacy connected suite: **3/3 PASS**, zero failures/errors/skips, first and only final-source run, Gradle **4m 25s**. `AndroidGateResumeProbe.resumePublicationAndAsyncBindError` and both `TorE2ETest` live tests pass. Results and test-case timing are retained in `connected-reviewed-first/` and its summary. No unchanged diagnostic retry was used.

**Release disposition:** synchronous old-listener teardown PASS; Android hardware gate PASS with historical reliability follow-up; implementation blockers NONE; release blockers NONE; ArtiTor 0.3 READY TO TAG. The original pause defect is remediated and the independent review finding resolved. Historical remote SOCKS code 5, TLS EOF and CONNECT-reply timeout remain **NON-BLOCKING RELIABILITY FOLLOW-UP**, with unknown underlying causes. This task supplies no causal evidence linking them to pause and changes no remote stream policy.

Durable evidence is in [pause-teardown-2026-10-01](evidence/0.3/pause-teardown-2026-10-01/README.md), with a complete SHA256 manifest. Earlier first outcomes, RED failures and historical reports remain preserved.

## Commit identities

Baseline: `e75336411a7eaadda4a2c828d668cd373dc6b0e1`.

Production: `5b4d29954d47bd0a73eb7ebe8312a02d5948f902` — `fix(lifecycle): await SOCKS listener teardown on pause`.

Tests: `6883845169b96cd22145e73f5de744cbc843ccd3` — `test(lifecycle): enforce synchronous old-port teardown`.

Local review fixups were consolidated into these two focused commits before the report commit. The report commit/final HEAD is resolved with `git log -1 --format=%H -- docs/audit/ARTITOR_0_3_PAUSE_TEARDOWN_REMEDIATION.md` (avoids a self-referential hash), and is supplied explicitly in the final response.

The three unrelated original untracked independent-review/iOS files are preserved without edits or staging.
