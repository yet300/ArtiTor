# ArtiTor 0.3 Android request reliability disposition

Latest disposition: **pause teardown REMEDIATION PASS; Android hardware gate PASS WITH RELIABILITY FOLLOW-UP; release blockers NONE; ArtiTor 0.3 READY TO TAG.** See the final remediation disposition appended below. The original investigation text and findings are preserved as history.

Date: 2026-10-01, Asia/Tbilisi.

**ANDROID REQUEST RELIABILITY DISPOSITION: LOCAL WRAPPER DEFECT FOUND**

**ARTITOR 0.3 ANDROID HARDWARE GATE: REMEDIATION REQUIRED. NOT READY TO TAG.**

Confidence: **high** that pause can return before the old root TCP listener is destroyed. The hardware diagnostic observed this once; a controlled offline native reproducer demonstrates the missing completion barrier in 10/10 schedules. Confidence: **unknown** for the underlying causes of the historical remote CONNECT rejection and TLS EOF. The new teardown finding is not claimed to explain those historical failures.

## Baseline and device

Required and observed HEAD: `e75336411a7eaadda4a2c828d668cd373dc6b0e1`.

Initial status contained only these pre-existing untracked files, preserved without edits:

```text
?? docs/audit/ARTITOR_0_3_FINAL_INDEPENDENT_REVIEW.md
?? docs/audit/ARTITOR_0_3_PHASE2_INDEPENDENT_AUDIT.md
?? tor/src/iosTest/kotlin/com/yet/tor/IndependentReplacementAuditProbe.kt
```

Device: physical Google Pixel 6a (`bluejay`), serial `26101JEGR21458`, Android 17 / API 37, arm64-v8a. The original paired wireless transport was `adb-26101JEGR21458-ZDzyCZ._adb-tls-connect._tcp`. Reconnection temporarily added a manual TCP transport to the same device, which was removed before the instrumentation regression to avoid duplicate execution.

All accepted R8/JNA, minified callback/StateFlow and integrated release-artifact evidence remains accepted. This investigation did not reopen the FieldOrder issue or change the accepted asynchronous session-restoration contract.

## Local hardware diagnostic

The standalone minified consumer still resolves only Maven coordinate `io.github.yet300:tor:0.3.0`; there is no project dependency, native override or additional keep rule. The new diagnostic opens explicit loopback sockets and sends only `05 01 00` in local phases. A successful published endpoint must immediately return `05 00`. No local phase sends CONNECT, resolves a target, or requires a new Tor circuit.

Root readiness comes from public engine StateFlow RUNNING/non-null port; session readiness comes from StateFlow ACTIVE/non-null endpoint. There is no sleep between the awaited snapshot and opening its greeting socket. Resume waits for root and then the retained session's own current snapshot. Negative old-port probes occur immediately after pause returns, before resume can allocate another endpoint. Only `ECONNREFUSED` (Android errno 111) passes a negative probe; timeouts do not pass.

| Observation | First completed run, PID 30423 | Separate confirmation, PID 30958 |
|---|---:|---:|
| Root RUNNING greetings | 80/80 PASS | 102/102 PASS |
| Session-create ACTIVE greetings | 100/100 PASS | 100/100 PASS |
| Resumed retained-session ACTIVE greetings | 30/30 PASS before stop | 50/50 PASS |
| Cold-start RUNNING greetings, included in root count | 1/1 PASS before stop | 3/3 PASS |
| Old root/session ports correctly refused while PAUSED | 60 probes | 100 probes |
| Old root TCP connection unexpectedly succeeded | **1, pause cycle index 30** | 0 |
| Published RUNNING/ACTIVE endpoints refused greetings | 0 | 0 |
| External requests | 0, stopped at local failure | 4/4 PASS after that run's local phases |
| Outcome | **GATE_FAILED** | GATE_COMPLETED,PASS |

The initial 50 root observations use one bound listener; subsequent resume observations exercise fresh root bindings. These are observations, not 50 independent cold bootstraps. Across the two completed runs there are 182 successful root greetings, 200 successful session-create greetings, 80 successful resumed-session greetings and four cold-start root greetings. These totals do not erase the negative-probe failure.

### Hardware failure preserved

At local time approximately 14:06:37, after `pause()` returned and the diagnostic asserted root PAUSED/retained client and session PAUSED:

```text
LOCAL_PASS,resume_root_29,port=41957,...
LOCAL_PASS,resume_session_29,port=32865,...
DIAGNOSTIC_PUBLIC_LOG,SOCKS shutdown signal
DIAGNOSTIC_PUBLIC_LOG,SOCKS paused; TorClient retained
GATE_FAILED,java.lang.IllegalStateException,old listener still accepts: pause_root_30 port=41957
```

`Socket.connect(127.0.0.1:41957)` succeeded inside the old-port negative probe. No resume, external CONNECT or replacement start occurred between pause and this probe. The run stopped and shut down in `finally`. Its remaining resume cycles, cold restarts and external matrix did not execute.

This is **TCP connection acceptance by a still-bound socket/backlog**. It does not establish that a SOCKS greeting or Tor stream could complete while PAUSED. The first probe closed the connected socket without attempting a greeting; the confirmation adds snapshot/history/greeting capture if the same failure recurs, but that run did not reproduce it.

The separate successful confirmation is preserved in full. It shows the problem is scheduling dependent; it is not used to replace the first failure or claim release acceptance.

### Contract and narrow source evidence

The frozen API contract, `docs/design/ARTITOR_0_3_API_FREEZE.md:342`, says pause transitions sessions synchronously before return and **“Old ports MUST stop accepting (listener dropped, backlog drained via abort).”** Its lifecycle table at line 407 repeats the old-port guarantee.

The narrow relevant path is:

1. `tor/src/commonMain/kotlin/com/yet/tor/ArtiTorClient.kt:655`: `doPauseLocked()` calls native pause and completes public PAUSED/null fallback publication. It does not await listener destruction.
2. `rust/arti-kmp-ffi/src/engine/lifecycle.rs:248`: `ffi_pause()` signals root shutdown, clears `bound_port`, aborts connections, calls `w.abort()`, demotes sessions and publishes PAUSED. It does not wait for the root worker to finish.
3. `rust/arti-kmp-ffi/src/engine/root_socks.rs:60`: `run_socks_worker()` owns the Tokio TCP listener as a local variable. The listening socket is dropped when that worker unwinds/exits. A synchronous callback can delay that completion after the cancellation request.

The root worker's revision/state checks prevent stale dispatch after pause. The observation therefore concerns completion of physical listener teardown; no post-pause traffic forwarding or isolation failure is claimed.

### Deterministic offline reproducer

Durable source: [offline_pause_reproducer.rs](evidence/0.3/android-request-reliability-2026-10-01/offline_pause_reproducer.rs). Output: [offline-reproducer-output.txt](evidence/0.3/android-request-reliability-2026-10-01/offline-reproducer-output.txt).

The diagnostic was injected only as a test module into `/tmp/artitor-android-reliability/offline-crate`, a disposable copy of the unchanged Rust crate. It uses the existing offline `running_engine()` helper with an unbootstrapped retained client. It runs the real `run_socks_worker()` and calls the real public native `pause()`; it introduces no production seam or behavioral change.

It holds the root worker in a synchronous RUNNING status callback after native publication/bind, allowing another thread to pause while worker destruction is delayed. This is a controlled scheduling test, not an estimate of natural failure frequency. After pause returns, each iteration verifies native PAUSED, `bound_port=0`, retained client, session PAUSED/null, and worker revision `1 -> 2`. TCP connect to the old root port succeeds. A greeting on that established socket times out while the worker is held; **no CONNECT is sent**. Releasing the callback allows cancellation to finish and the old port then refuses connections.

All ten iterations produced `DEFECT_REPRODUCED` followed by `AFTER_CALLBACK_RELEASE,...old_port_refused=true`. The diagnostic test intentionally passes when it reproduces the current defect; its PASS is **not a product acceptance PASS**. Reproducer command/setup are in [README.md](evidence/0.3/android-request-reliability-2026-10-01/README.md).

This establishes a deterministic completion-barrier defect independently of Tor/remote reliability. It has not been fixed in this task.

## Legacy HTTP helper review and historical failure stages

`bootstrapFetchPauseResume` reads a fresh root port after initial start, resume and restart. Every `assertTorExit(port)` creates a new SOCKS Proxy and a new OkHttp client. No Proxy, SocketFactory, client or connection pool is carried across lifecycle transitions. The integrated `httpSuccess()` also creates a fresh explicit proxy and connection pool per call and evicts/shuts it down in `finally`.

The legacy exit helper lacks equivalent client/pool cleanup, and the outer bootstrap test lacks a `finally` for failure cleanup. These are test hygiene limitations, but there is no evidence that they caused the historical failures. No speculative helper fix was made. OkHttp 4.12.0 is explicitly selected in the device-test build, independently of the catalog's newer version. Its resolved source `RouteSelector.kt` uses `InetSocketAddress.createUnresolved` for SOCKS targets. Hostname resolution is delegated through SOCKS.

The exit helper consistently uses HTTPS to `check.torproject.org/api/ip`; it does not mix HTTP and HTTPS internally. The integrated helper uses HTTP for ipify/onion traffic. The separate direct-IP comparison in the bootstrap test is not a Tor fallback and cannot satisfy its Tor assertions. The bootstrap test's initial, resumed and restarted traffic assertions all depend on the same Tor Project check service and its `IsTor:true` response.

| Preserved historical failure | Established stage | Limits |
|---|---|---|
| A's first request: `SOCKS: Connection refused`, `SocksSocketImpl.java:574` | **SOCKS CONNECT rejection, code 5**, after proxy TCP connection and negotiation | The original native safe category was not retained; this does not prove destination-level refusal or explain why Arti returned 5 |
| Post-resume `SSLHandshakeException: connection closed`, Conscrypt/OkHttp `connectTls` | TLS handshake EOF after SOCKS socket establishment/CONNECT succeeded | Original reply bytes were not separately logged; ordering is established by the TLS stage, not a new packet capture |
| Initial request timeout, `SocksSocketImpl.java:510`, `readSocksReply` | Waiting for the **CONNECT reply** after local TCP connection and negotiation | No reply code was received; original native category unavailable |

The installed Android 37.2 SDK source matches both historical Java line numbers exactly: line 510 reads the four-byte CONNECT reply; line 574 constructs the exception for `CONN_REFUSED`. `SocksConsts.java:54` defines that value as 5. Android's [primary libcore source](https://android.googlesource.com/platform/libcore/+/13ff49ea4f7ea513a714d13231d85552cfebd421/ojluni/src/main/java/java/net/SocksSocketImpl.java) corroborates the reply-code branch; its line numbering differs from the installed SDK source.

Thus the historical “Connection refused” is independently disposed as a SOCKS reply, **not loopback ECONNREFUSED**. No deterministic wrapper cause of those historical external failures was reproduced. The new pause TCP teardown defect is a separate blocker.

## Minified fixture comparison and external matrix

The accepted minified integrated fixture uses a fresh explicit loopback socket for every request, a single no-auth SOCKS greeting and domain-name CONNECT, then raw HTTP on port 80. It has no HTTP pool, target DNS lookup, redirects or direct fallback. The legacy HTTPS helper uses Java's SOCKS implementation, which offers its own negotiation methods and handles reply codes internally, followed by OkHttp/Conscrypt TLS. The baseline minified HTTP successes alone did not prove the legacy HTTPS path or check-service availability.

The new diagnostic layers a fresh certificate/hostname-verified `SSLSocket` onto its already-established SOCKS socket. It sends unresolved domains to Tor at port 443, records the SOCKS reply before TLS, and records TLS completion before reading the HTTP status. It has no direct socket/DNS branch or retry loop. The fixed 90-second stream-read diagnostic bound is not a change to the legacy test's defaults or production Arti settings.

The confirmation's matrix used the same already-bootstrapped root endpoint `127.0.0.1:39499`, RUNNING, bootstrap 100, `hasClient=true`. It made two requests per existing repository target, using the HTTPS form of the already-approved ipify host:

| Target / request | Identity | SOCKS CONNECT | TLS | HTTP |
|---|---|---:|---|---:|
| check.torproject.org `/api/ip`, 1 | root | 0 | TLSv1.3 PASS | 200 |
| api.ipify.org `/`, 1 | root | 0 | TLSv1.3 PASS | 200 |
| check.torproject.org `/api/ip`, 2 | root | 0 | TLSv1.3 PASS | 200 |
| api.ipify.org `/`, 2 | root | 0 | TLSv1.3 PASS | 200 |

No matrix request failed; no external-failure native category was emitted. The matrix checks transport/TLS/HTTP status, not the `IsTor` JSON assertion. It does not prove target-specific fragility or justify changing the legacy gate structure. No endpoint, timeout, retry or pass/fail structure was changed in `TorE2ETest`.

Expected native `early eof` log entries during greeting-only probes occur because the diagnostic closes without sending CONNECT. They are not classified as remote stream errors. Historical `RemoteNetworkTimeout`, `RemoteStreamError`, `RemoteStreamClosed` or exit-failure categories remain unavailable; none is invented from exception prose. Existing accepted InvalidStreamTarget/ForbiddenStreamTarget negative-test evidence and permanent mapping tests stand.

The current public API exposes per-target traffic through SOCKS. No public per-target `connect(target)` operation exists, so a thrown public TARGET_REJECTED exception is not required; accepted SOCKS rejection plus safe categorical native transport and permanent mapping tests remain sufficient. No public API was invented for this investigation.

## First-process Logcat and redaction closure

Logcat was cleared before each intentional minified launch. A threadtime capture was started **before construction**, then selected by the PID in the fixture's harmless `CONTROL,ARTITOR_FIRST_PROCESS_CAPTURE` marker. All tags for that PID are retained; unrelated device PIDs are excluded from durable evidence. The marker is emitted before `ArtiTorClient()` construction.

The synthetic malformed bridge was exercised immediately after construction and before bootstrap, with three distinct fake markers: bridge IP `192.0.2.231`, fingerprint `F00DF00DF00DF00DF00DF00DF00DF00DF00DF00D`, and transport `SYNTHETIC_RELIABILITY_TRANSPORT_SECRET`.

| Capture | App PID lines | Control present | Config exception redacted | IP/fingerprint/transport occurrences |
|---|---:|---|---|---|
| Interrupted first process, PID 17070 | 42 | YES | PASS | 0 / 0 / 0 |
| First completed diagnostic, PID 30423 | 610 | YES | PASS | 0 / 0 / 0 |
| Complete confirmation, PID 30958 | 1072 | YES | PASS | 0 / 0 / 0 |

The public exception is CONFIG / `configuration error: invalid bridge configuration`; collected public log-flow records also contain none of the markers. The nonempty process captures close the earlier empty-filtered-Logcat gap. Counts and scans are recorded in `evidence-summary.json`.

## Attempts, provenance and regressions

The first wireless install returned a running shell session; an early launch reached the previous consumer before replacement completed. That unintended integrated run was stopped and preserved separately; it is not counted as a new completed integrated PASS. The first correctly installed diagnostic then reached construction/redaction/bootstrap, but wireless capture ended and the user-0 installation/data disappeared before its result could be recovered. Its partial PID 17070 capture is retained; no listener result is inferred. After reconnecting to the paired mDNS service, the existing package code was restored for user 0 and the two completed runs above used unique evidence names.

The first `connectedAndroidDeviceTest` command executed **zero tests**: UTP install-multiple timed out at its 360000 ms installer bound. A non-streaming install attempt subsequently ended with ADB EOF. Neither is an ArtiTor test failure or external Tor retry. Hardware source/result evidence was retained before later intentional runs; the installer failure text is in `tooling-first-failures.txt`. The later recovered connected-test result is recorded below and in the gate append.

Artifact hashes/native provenance are in `artifact-provenance.json`. Both diagnostic APKs contain arm64 Arti SHA-256 `8dab245aede46d9ef52f6e8cd3b1831b74f92e2f6c4295663afe2620addd334a`, the already-accepted stripped release-native bytes. Maven Android AAR SHA-256 is `ba9c3d9b502d107e2a3a60c110ee26e5129487b5db2e4528b83427a764065454`; its unstripped arm64 library is `59e4acebef0d584c6d41e221c1d8d150d8533e6865f244466baedd5f3c776ffb`. No new remote or Maven-local publication occurred.

Regression results:

- `cargo test`: initial sandbox 52 PASS / 45 socket-permission failures; host-permitted run **97 PASS**.
- `cargo test -- --test-threads=1`: host-permitted **97 PASS**.
- Required release bindings / iOS arm64 Kotlin compile / Android device-test assembly: **BUILD SUCCESSFUL**.
- Standalone minified diagnostic assembly: **BUILD SUCCESSFUL**, including the enhanced failure capture.
- Disposable native controlled reproducer: one diagnostic test PASS, ten defect observations, 97 unrelated tests filtered. This is confirmation of a defect, not remediation.
- Connected suite after installation recovery: **3 tests, 1 PASS / 2 FAIL**, zero skipped/errors, XML duration 107.706 s. `resumePublicationAndAsyncBindError` passes. `bootstrapFetchPauseResume` fails its initial check-service request at line 65 with TLS handshake EOF. `integratedIsolationOnionAndTransactionalLifecycle` reaches the bounded post-resume ACTIVE wait and fails its resumed B request at line 162, waiting for CONNECT reply at Java line 510. No further retry was made. XML and a parsed result summary are preserved separately.

The recovered legacy TLS failure's root was published RUNNING/bootstrap 100 on port 36603. Its previous `hasClient` assertion passed. The resumed B failure occurred after the test's own root RUNNING/isReady assertions and successful ACTIVE/current-endpoint wait. The legacy helper does not emit the failing B port, a contemporaneous complete snapshot or raw SOCKS reply bytes. No safe native category was retained for either failure; these fields remain unavailable rather than inferred. This observability limit does not affect the separately captured four-request raw SOCKS matrix or the deterministic local teardown reproducer.

No iOS live suite was repeated; no production source changed.

## Changes and final release disposition

**Production changes: NONE.** Authoritative Rust/Kotlin lifecycle, configuration, policies, timeouts, retries, R8 rules and dependency versions are unchanged.

**Test/release-verification changes:** pre-construction harmless control marker and opt-in finite diagnostic mode in the existing minified Maven fixture; new `ReliabilityDiagnostic.kt`; source-only offline reproducer in the evidence directory, executed only in a disposable copy; reports and small process-scoped evidence. Legacy Android test behavior is unchanged. Pre-existing untracked review/iOS files are untouched.

The full accepted minified Maven integrated flow remains **previously PASS**; no JNA/UniFFI/StateFlow regression appeared in the new diagnostics. A fresh full integrated consumer acceptance run was not substituted for this local defect investigation. Its previous PASS cannot waive the newly reproduced frozen pause-contract violation.

**IMPLEMENTATION BLOCKER:** native/public pause completion is not a root-listener destruction barrier: an old root port can accept TCP after PAUSED publication and return. Hardware occurrence plus deterministic scheduling reproducer supplied. Remediate separately, without weakening the old-port contract or adding a sleep to its test.

**RELEASE BLOCKER:** that local listener-teardown violation. Historical external request reliability remains a separate follow-up with no deterministic wrapper cause established for those specific failures.

**ARTITOR 0.3 ANDROID HARDWARE GATE: REMEDIATION REQUIRED.**

**ARTITOR 0.3: NOT READY TO TAG.** No fix, tag, commit, push, remote publication or release was made.

## Final synchronous teardown remediation disposition — 2026-10-01

**PAUSE TEARDOWN REMEDIATION: PASS. ANDROID HARDWARE GATE: PASS WITH RELIABILITY FOLLOW-UP. IMPLEMENTATION BLOCKERS: NONE. RELEASE BLOCKERS: NONE. ARTITOR 0.3: READY TO TAG.**

Baseline `e75336411a7eaadda4a2c828d668cd373dc6b0e1`; production `5b4d29954d47bd0a73eb7ebe8312a02d5948f902`; tests `6883845169b96cd22145e73f5de744cbc843ccd3`. [The remediation report](ARTITOR_0_3_PAUSE_TEARDOWN_REMEDIATION.md) supplies ownership/reentry/lock proof and durable evidence. Ten held-callback schedules now yield immediate TCP refusal while the callback remains held, versus ten accept-capable returns before the fix. Rust 106/106 parallel and serial; iOS 103/103; Pixel 100 cycles / 200 paused old root/session refusals / zero accepts. Final fresh Maven-only R8 consumer verifies all 17 surviving FieldOrder structures, passes another 100-cycle local diagnostic and its first full integrated flow (86 assertions / 36 immediate refusals). Final connected suite first run passes 3/3, zero failures/skips, 4m 25s. Installed APK/native-library provenance is verified. Confidence: **high**.

The old pause failure and all earlier remote outcomes below are historical evidence, preserved rather than overwritten. Historical remote SOCKS code 5, TLS EOF and CONNECT-reply timeout have **unknown** underlying causes and remain a **NON-BLOCKING RELIABILITY FOLLOW-UP**. There is no fresh causal proof that the pause fix explains them. No tag, push or remote publication occurred.
