# ArtiTor 0.3 Android hardware release gate

Latest disposition (remediation run appended below): **ARTITOR 0.3 ANDROID HARDWARE GATE: REMEDIATION REQUIRED**. The R8 blocker is fixed and the minified integrated flow passes; the mandatory complete Android instrumentation suite still fails after one unchanged diagnostic retry. Earlier attempts remain below as historical evidence.

Date: 2026-10-01 (Asia/Tbilisi).

**ANDROID HARDWARE RELEASE GATE: NOT VERIFIED**

Confidence: **high** that the successful ADB enumeration returned no connected devices; **unknown** for all Android runtime acceptance checks.

## Exact baseline

`git rev-parse HEAD` returned the required production commit:

```text
628c6ea13c7286c9a83fb4c028e2bbcbb66b58fb
```

Initial `git status --short`:

```text
?? docs/audit/ARTITOR_0_3_FINAL_INDEPENDENT_REVIEW.md
?? docs/audit/ARTITOR_0_3_PHASE2_INDEPENDENT_AUDIT.md
?? tor/src/iosTest/kotlin/com/yet/tor/IndependentReplacementAuditProbe.kt
```

These are pre-existing, untracked independent-review reports and an iOS replacement audit probe. They were preserved without edits or commits. No tracked production changes were present.

`git log --oneline -15`:

```text
628c6ea docs(audit): record iOS 15 remediation evidence
42c00fa build(apple): raise minimum iOS version to 15
3f5365a docs: finalize ArtiTor 0.3 implementation report
6ea7f0b test(release): complete ArtiTor 0.3 hardening
c00397a feat(config): add onion policy and stream timeouts
4bd683b feat(errors): add stable Tor error classification
50a62a4 fix(config): validate replacement before lifecycle teardown
695990a docs(audit): record Phase 2 bridge hardening verification
8bb0d95 feat(config): harden bridge configuration
dd66474 docs(audit): record Rust modularization verification
54eeeca refactor(rust): modularize arti-kmp-ffi
ad83254 docs(audit): add ArtiTor 0.3 capability audit
5c506b9 docs(audit): record final Phase 1 acceptance
86da3c8 Fix stale session status publication with conditional CAS
55ca6d1 Fix final Phase 1 acceptance defects
```

## Device discovery and stop condition

The initial `rtk proxy adb devices -l` failed because `adb` was absent from PATH. `local.properties` configured `/Users/yet/Library/Android/sdk`; its `platform-tools/adb` exists.

The SDK ADB invocation initially failed inside the sandbox with `could not install *smartsocket* listener: Operation not permitted`. The same command was then run with host permission:

```text
rtk proxy /Users/yet/Library/Android/sdk/platform-tools/adb devices -l
```

Exit code: **0**. Output:

```text
List of devices attached

```

No physical device or emulator was listed. This is device discovery, not an instrumentation test result or network diagnostic rerun. Per the explicit no-device stop condition, no builds, publication, instrumentation, launcher creation, or runtime tests followed.

## Evidence ledger

| Required evidence | Result |
|---|---|
| Device serial, manufacturer/model | Unavailable: no ADB device |
| Android version / API level | Not measured |
| ABI / physical versus emulator | Not measured |
| Published artifact | Intended coordinate `io.github.yet300:tor:0.3.0`; fresh Maven-local publication not performed |
| Release native packaging / loaded ABI | Not verified |
| Minified consumer | Not built or run |
| Instrumentation command / first test result | Not run; no test count, passes, failures, skips, or duration |
| Runtime rerun | None |
| Cold bootstrap / pause duration / resume timing | Not measured |
| Root HTTP / session A and B traffic | Not verified |
| Close isolation / endpoint invariants | Not verified |
| Pause/resume / retained-client evidence | Not verified |
| Positive onion request / onion policy rejection | Not verified |
| Invalid replacement preservation | Not verified |
| Shutdown / cold restart / stale handles | Not verified |
| JNA construction, version, native start, load errors | Not exercised |
| UniFFI status, log, error, session callbacks | Not exercised |
| R8/minified runtime callback delivery | Not exercised |
| Public status/log/session StateFlow delivery | Not exercised |
| Typed Config / TARGET_REJECTED error transport | Not exercised |
| Asynchronous error category transport | Not exercised |
| Synthetic bridge secrecy in runtime logs/errors | Not exercised |
| Proxy-bound fail-closed HTTP infrastructure | Not inspected or exercised in this gate |
| Idle sessions 0/1/8/16/32 and 33rd-session rejection | Not measured or exercised |
| Loaded-traffic root/1/8-session resource samples | Not measured |
| FD, RSS/PSS, thread and listener counts / recovery | Not measured |
| Native/task ownership and listener teardown | Not verified |
| RemoteNetworkTimeout / RemoteStreamError / SOCKS 5 | No observations: no runtime requests were made |
| Runtime-failure Logcat | Not captured: no application runtime occurred |
| Optional cargo / iOS simulator / Android compile regressions | Not run after the required stop |

## Release decision

**IMPLEMENTATION BLOCKERS:** No defect reproduced in this attempt; Android implementation correctness remains unverified.

**RELEASE BLOCKERS:** The required Android hardware acceptance and minified release consumer JNA/UniFFI runtime gate remain open. A usable ADB device is required to execute them.

**NON-BLOCKING FOLLOW-UPS:** None established by this attempt. There is no runtime evidence for classifying network reliability.

**ARTITOR 0.3: READY TO TAG** is not established. No tag, remote push, Maven Central publication, GitHub release, production change, or 0.4 work was performed. This report is the only newly created repository file.

---

## Resumed physical-device run — 2026-10-01

**ARTITOR 0.3 ANDROID HARDWARE GATE: REMEDIATION REQUIRED**

The actual minified release Maven consumer fails during public `ArtiTorClient()` construction. It cannot reach `version()`, bootstrap, or callbacks. This mandatory runtime failure independently prevents release acceptance. Confidence: **high**; the same installed APK reproduced the same failure on two launches, and the missing JNA field-order annotation was verified in the installed DEX against the original runtime class.

No production fix was made. The report records acceptance evidence and reproducers at the required production HEAD, rather than silently changing the candidate under test.

### Resumed baseline and device

`git rev-parse HEAD` again returned `628c6ea13c7286c9a83fb4c028e2bbcbb66b58fb`.

Initial status contained the original three user-owned untracked files plus this existing hardware-gate report; no tracked changes. All original untracked files were preserved.

SDK ADB again needed host permission to start its local server; the successful enumeration returned:

```text
List of devices attached
26101JEGR21458         device usb:1-1 product:bluejay model:Pixel_6a device:bluejay transport_id:5
```

| Property | Captured value |
|---|---|
| Serial | `26101JEGR21458` |
| Manufacturer | Google |
| Model | Pixel 6a (`bluejay`) |
| Android release | `17` |
| API level | `37` |
| Primary ABI | `arm64-v8a` |
| Physical / emulator | Physical USB device; `ro.kernel.qemu` empty |
| Minified installed package ABI | `dumpsys package consumer.artitor03.hardwaregate`: `primaryCpuAbi=arm64-v8a` |

Only this device was listed. Device properties were read through `adb -s 26101JEGR21458 shell getprop <property>`.

### Fresh release publication and artifact provenance

Command (run before runtime testing):

```text
rtk ./gradlew :tor:publishToMavenLocal -PreleaseBuild=true --no-configuration-cache --console=plain
```

Result: **BUILD SUCCESSFUL in 11s**, 78 tasks, 22 executed. Android Cargo tasks explicitly reported optimized `release` builds. Maven-local publication only; no remote publication.

Coordinate: **`io.github.yet300:tor:0.3.0`**. Android variant: `io.github.yet300:tor-android:0.3.0`.

| Artifact | Bytes | SHA-256 |
|---|---:|---|
| Published Android AAR | 10,825,882 | `e557591411ecb15a7f0ef76faff75bd17296dbc7a03fc4d022ad824f992cf0d6` |
| Fresh minified signed release APK | 8,931,100 | `035b6bf2b03d328c0bfc5eb2921301a2eb99325a82fce078bf1ff569f593fbec` |
| Installed APK pulled back from device | 8,931,100 | `035b6bf2b03d328c0bfc5eb2921301a2eb99325a82fce078bf1ff569f593fbec` |

Published AAR arm64 `libarti_kmp_ffi.so` SHA-256 is `59e4acebef0d584c6d41e221c1d8d150d8533e6865f244466baedd5f3c776ffb`, exactly matching `tor/build/uniffi/build/rust/aarch64-linux-android/release/libarti_kmp_ffi.so`. The minified APK's stripped arm64 library SHA-256 is `8dab245aede46d9ef52f6e8cd3b1831b74f92e2f6c4295663afe2620addd334a`, exactly matching the release-path instrumentation APK's arm64 entry. The fresh consumer's native merge report attributes the Arti library to `io.github.yet300:tor-android:0.3.0`. No consumer native build or override exists. The minified APK contains only arm64 JNI entries for Arti, JNA dispatch, and the UniFFI runtime.

### Existing instrumentation: first result preserved

```text
rtk ./gradlew :tor:connectedAndroidDeviceTest -PreleaseBuild=true --no-configuration-cache --console=plain
```

| Attempt | Tests | Pass | Fail | Skip | XML aggregate duration | Gradle duration |
|---|---:|---:|---:|---:|---:|---|
| First, unchanged repository tests | 2 | 1 | 1 | 0 | 101.657 s | 1m51s |
| Test-only cleartext manifest correction | 2 | 1 | 1 | 0 | 142.801 s | 2m32s |
| Focused resume/error diagnostic | 1 | 1 | 0 | 0 | 29.955 s | 38s |

The first `bootstrapFetchPauseResume` passed in **66.264 s**. It constructed the real public client, returned `arti-kmp-ffi 0.3.0 (arti-client 0.46, rustls)` from the native version call, bootstrapped, completed explicit SOCKS HTTPS requests whose responses asserted `IsTor:true`, paused with a retained client, resumed in **23 ms**, performed another Tor HTTPS request, shut down, cold restarted, and completed another Tor HTTPS request. The corrected run's same test passed in **77.907 s**, with resume **24 ms**.

The first integrated test bootstrapped in **34,121 ms**, then failed at its first plain-HTTP fixture request:

```text
java.net.UnknownServiceException:
CLEARTEXT communication to api.ipify.org not permitted by network security policy
at com.yet.tor.TorE2ETest.httpSuccess(TorE2ETest.kt:191)
at com.yet.tor.TorE2ETest$integratedIsolationOnionAndTransactionalLifecycle$1.invokeSuspend(TorE2ETest.kt:123)
```

This was an Android test-client policy failure before the SOCKS request, not a Tor transport failure. The test APK targets API 37 and its original manifest had only permissions, with no cleartext opt-in. The only tracked edit made in this gate is a test-only `<application android:usesCleartextTraffic="true" />` plus explanatory comment in `tor/src/androidDeviceTest/AndroidManifest.xml`. The merged test manifest confirmed the setting. No production manifest, library configuration, timeout, retry, or Tor behavior changed.

The corrected run is explicitly **not an unchanged retry**, and does not replace the first result. It completed root HTTP, A/B HTTP, unique root/A/B endpoints, A CLOSED, continued root/B HTTP after A close, all idle capacity samples, 33rd-session rejection, and synchronous pause state/null endpoints. It then failed after successful `resume()`:

```text
LIVE_TIMING,cold_ms=37510
LIVE_TIMING,resume_ms=18
java.lang.IllegalArgumentException: Required value was null.
at com.yet.tor.TorE2ETest$integratedIsolationOnionAndTransactionalLifecycle$1.invokeSuspend(TorE2ETest.kt:153)
```

Line 153 immediately requires B's endpoint after `resume()` returned. Thus the existing integrated acceptance did **not** pass, and its onion/config/restart tail did not execute.

### Resume publication diagnostic and safe asynchronous error callback

Added test-only `AndroidGateResumeProbe.kt` records the immediate session snapshot separately from eventual StateFlow delivery; it does not change production synchronization. Command:

```text
rtk ./gradlew :tor:connectedAndroidDeviceTest -Pandroid.testInstrumentationRunnerArguments.class=com.yet.tor.AndroidGateResumeProbe -PreleaseBuild=true --no-configuration-cache --console=plain
```

The probe bootstrapped the real native runtime once, created a session, and paused/resumed five times without Tor HTTP traffic. All five immediate post-return snapshots had **root RUNNING + session PAUSED/null**; all five later received **session ACTIVE + endpoint** within the diagnostic's 5-second bound:

| Cycle | Resume return / immediate snapshot (µs from call) | ACTIVE observed (µs from call) |
|---:|---:|---:|
| 0 | 14,377 | 17,046 |
| 1 | 15,188 | 17,806 |
| 2 | 12,909 | 15,003 |
| 3 | 12,119 | 14,282 |
| 4 | 4,255 | 4,532 |

These are approximate diagnostic timings, including logging overhead, not a performance guarantee. The probe proved retained-client behavior and eventual session restoration. It did **not** show a permanently lost callback or stranded session.

Focused causal inspection explains the ordering: `run_socks_worker` publishes root RUNNING before sending the root-ready signal; `wait_root_then_rebind_sessions` waits for that signal and then restores sessions. The public facade waits for root readiness. The existing acceptance test's assumption that B is already ACTIVE at the next instruction is therefore incompatible with the observed ordering. The completion/test contract needs resolution; this report does not relabel that failure as an external-network flake or prove permanent lifecycle corruption.

For a safe real native asynchronous error, the probe paused and requested a root port held by a local `ServerSocket`. It observed:

```text
ROOT,state=ERROR,kind=BIND
ASYNC_BIND_PUBLIC,kind=BIND,status_kind=BIND
SESSION,state=INVALIDATED,endpoint=null
ROOT,state=OFF,kind=null
```

Assertions checked public `ArtiException.Bind`, `TorErrorKind.BIND`, typed `status.lastError`, the session PAUSED invariant on error, then INVALIDATED/null and `hasClient=false` after shutdown. This proves native → UniFFI/JNA → Kotlin error/status/session callback transport in the **unminified instrumentation path**. It does not prove the corresponding minified path.

### Minified release Maven consumer: deterministic runtime blocker

A fresh standalone fixture was created under `/tmp/artitor-android-gate/consumer`, with source/config retained in the evidence directory below. It has one ArtiTor dependency:

```text
implementation("io.github.yet300:tor:0.3.0")
```

Maven local is exclusive for `io.github.yet300`; no project dependency, source inclusion, native override, explicit additional coroutines dependency, or extra consumer keep-rule file exists. AGP **9.3.1**, Kotlin **2.4.20**, compile/target SDK **37**, min SDK **26**, arm64-only package. Release has `isMinifyEnabled=true` and the standard optimized Android ProGuard file only; it is signed with the local debug key for installation. Debug signing does not disable release minification; installed package flags do not include DEBUGGABLE.

Fixture preparation initially used AGP's default Kotlin compiler 2.2.0, which rejected the library's Kotlin 2.4 metadata. The compiler was aligned to the repository's 2.4.20 before any consumer runtime run; no metadata-check suppression or library workaround was used. The diagnostic probe also had an initial compile-only import error (`kotlin.test` is not on this Android test source set); it was corrected to the existing JUnit assertions before execution. These compile-only preparation failures are separate from preserved runtime attempts.

Fresh build:

```text
rtk ./gradlew -p /tmp/artitor-android-gate/consumer :app:assembleRelease --refresh-dependencies --no-configuration-cache --console=plain
# Initial compiler mismatch above; after aligning Kotlin:
rtk ./gradlew -p /tmp/artitor-android-gate/consumer :app:assembleRelease --no-configuration-cache --console=plain
```

Final build: **BUILD SUCCESSFUL in 11s**, all 47 tasks executed, including `minifyReleaseWithR8`. JNA dispatch stripping produced the usual nonfatal packaging warning.

Install and first launch:

```text
rtk proxy /Users/yet/Library/Android/sdk/platform-tools/adb -s 26101JEGR21458 install -r /tmp/artitor-android-gate/consumer/app/build/outputs/apk/release/app-release.apk
rtk proxy /Users/yet/Library/Android/sdk/platform-tools/adb -s 26101JEGR21458 shell am start -n consumer.artitor03.hardwaregate/consumer.artitor03.app.MainActivity --es run first
```

**FIRST CONSUMER RUNTIME RESULT: FAIL**, during `ArtiTorClient()` construction, before the native version marker:

```text
java.lang.Error: Structure.getFieldOrder() on class uniffi.runtime.h$a
does not provide enough names [0] ([]) to match declared fields [3]
([capacity, data, len])
  at com.sun.jna.Structure.getFields(...)
  at com.sun.jna.Structure.deriveLayout(...)
  at com.sun.jna.Structure.validateFields(...)
  at uniffi.runtime.x.<init>(...)
  at uniffi.runtime.x$a.<init>(...)
  at com.yet.tor.ffi.ArtiTor.<init>(...)
```

The same APK was force-stopped and launched once with `--es run confirm`; the same construction error reproduced. No source/config/native/keep-rule change or rebuild occurred between these two launches. Both full app-captured stacks are preserved. This is a deterministic construction confirmation, not a Tor network retry.

R8 mapping resolves `uniffi.runtime.h$a` to **`uniffi.runtime.RustBufferStruct$ByValue`**, and `uniffi.runtime.x$a` to **`UniffiRustCallStatusStruct$ByReference`**. Original runtime 1.2.1 classfile inspection shows:

```text
RuntimeVisibleAnnotations:
  com.sun.jna.Structure$FieldOrder(value=["capacity","len","data"])
```

Installed DEX inspection shows the obfuscated RustBufferStruct base class still extends JNA Structure and still has public `capacity`, `data`, `len` fields, but has **no runtime FieldOrder annotation**; its ByValue subclass also lacks that annotation. Merged R8 configuration confirms both the published ArtiTor and upstream runtime rules were applied, including `-keepattributes RuntimeVisibleAnnotations` and JNA member rules. Those static rules did not preserve the required struct annotation in this actual minified app.

No consumer keep-rule workaround was attempted. This is the concrete failure that prior static DEX/class-presence checks did not detect. Minified native status/log/error/session callbacks and the integrated consumer flow are **blocked at construction**, not accepted on the basis of static presence.

### Android idle capacity evidence

From the corrected existing integrated test, after successful root traffic:

| Additional sessions | FD count | RSS (KiB) | Threads | Listener evidence |
|---:|---:|---:|---:|---|
| 0 | 123 | 122,400 | 28 | root usable |
| 1 | 124 | 123,092 (3 samples) | 28 | 2 distinct root/session ports |
| 8 | 131 | 122,088 (3 samples) | 28 | 9 distinct ports |
| 16 | 139 | 121,012–121,272 (3 samples) | 28 | 17 distinct ports |
| 32 | 155 | 121,800 (3 samples) | 28 | 33 distinct ports |
| All extra sessions closed | 123 | 122,584 | 28 | session close completed; sockets were not separately connection-probed |

The **33rd additional session was rejected** (existing `isFailure` assertion passed). Its exact exception/category was not logged; no stronger typed-cap claim is made. FD count returned from 155 to its root-only baseline of 123; thread count stayed 28. RSS varied with ongoing process work and retained pages; this does not prove exact memory reclamation. Private `MAX_SESSIONS=32` was not changed or exposed.

Separate loaded-traffic root+1/root+8 resource samples, PSS measurements, explicit dead-listener connection probes, and process FD recovery after native shutdown were planned in the minified launcher but did not execute because construction failed. Do not substitute the idle samples for those missing measurements.

### Acceptance coverage and limits

| Required check | Captured result |
|---|---|
| Real public KMP → JNA → UniFFI → Rust construction/version/start | PASS in release-native instrumentation; FAIL at construction in minified consumer |
| JNA load exception checks | No load exception in passing instrumentation; minified JNA struct-layout error as above |
| Native status/log callbacks | Observed public STARTING, BOOTSTRAPPING, RUNNING, PAUSED and log events in instrumentation |
| Native error callback/category | PASS: safe occupied-port asynchronous Bind → public BIND + typed lastError |
| Native session callbacks / StateFlow | ACTIVE → PAUSED → ACTIVE → INVALIDATED/null observed in focused real-runtime probe; corrected suite asserts A CLOSED |
| Endpoint iff ACTIVE | Asserted for every observed probe session snapshot; no contrary snapshot observed |
| Root SOCKS HTTP/HTTPS | PASS in existing release-native suite; explicit IsTor assertion on HTTPS |
| Session A/B HTTP | PASS in corrected suite before resume failure |
| Close A leaves root/B usable | PASS; both requests completed after A close |
| Closed endpoint refuses connection / old listener liveness | Not directly connection-probed |
| Pause / retained root client | PASS; root/session null states and hasClient retained |
| Resume without cold bootstrap | Observed retained client and 4–24 ms readiness evidence; probe shows eventual session ACTIVE without cold start |
| Immediate post-resume session readiness | Reproduced mismatch: PAUSED/null immediately after successful return; eventual ACTIVE in all 5 probe cycles |
| Positive public onion / disabled-onion rejection | NOT VERIFIED: existing suite stopped before these; minified consumer stopped at construction |
| Malformed bridge / ON-empty / invalid config preservation | NOT VERIFIED on hardware: not reached |
| Typed Config / TARGET_REJECTED transport | NOT VERIFIED on hardware; typed BIND verified only |
| Runtime synthetic bridge-secret redaction | NOT VERIFIED: synthetic malformed bridge steps not reached |
| Shutdown / root hasClient absent | PASS in old lifecycle test and focused probe |
| Session INVALIDATED/null after shutdown | PASS in focused probe |
| Root cold restart / HTTP | PASS in old lifecycle test |
| Old-session handles across cold restart / fresh session HTTP | NOT VERIFIED: integrated tail not reached |
| Idle cap / FD recovery after session close | Captured table above; 33rd rejected |
| Loaded-traffic capacity / shutdown FD recovery | NOT VERIFIED |
| Minified/R8 JNA and callback runtime | FAIL at construction; callback phase not reached |

Fail-closed review: the existing Tor clients explicitly set a SOCKS proxy; they do not switch to a direct client when SOCKS fails. The older test has a **separate** direct-IP comparison request, which is not fallback and is not used to satisfy Tor assertions. Its captured IP values were redacted from retained app logs. The integrated helper uses a fresh explicit SOCKS client/pool and disables redirects. The minified launcher opens only loopback SOCKS sockets, sends unresolved names to Tor, and has no direct target-DNS or direct-socket branch; it never reached those requests. A runtime negative HTTP fallback test did not complete, so fail-closed runtime acceptance is not fully claimed.

No `RemoteNetworkTimeout`, `RemoteStreamError`, or SOCKS code 5 was observed in the captured ArtiTor app evidence for these runs. No external-network diagnostic retry was used. The captured failures were Android cleartext policy, post-resume null endpoint, and deterministic minified JNA construction. None was labeled an unexplained environmental network flake, and no Tor timeouts/retries/bootstrap/mapping/isolation/lifecycle behavior was modified.

Optional cargo/iOS regression suites were not repeated: this gate made no production change and found a mandatory minified runtime blocker. Android device test compilation passed for the corrected suite and final probe.

### Preserved evidence and checkout changes

Permanent, small evidence bundle: `docs/audit/evidence/0.3/android-hardware-2026-10-01/` contains the first and corrected suite XML, focused probe XML, selected app-only logs, both minified failure stacks, artifact hashes, original/DEX annotation and R8 rule/mapping excerpts, and the exact standalone consumer source/config. Full raw UTP logs and installed APK remain outside the checkout in `/tmp/artitor-android-gate/evidence/`; no huge unrelated device log or APK was copied into repository evidence. Captured logs were checked for synthetic bridge markers and unredacted comparison-IP responses before preservation. No real bridge was used. No evidence was committed.

Checkout changes from this resumed attempt: this report updated; the Android **test-only** manifest correction; new Android **test-only** diagnostic probe; new evidence bundle. Production source, production build configuration, consumer rules, and Git HEAD remain unchanged. No tag, push, remote publication, GitHub release, or 0.4 work.

### Final release disposition

**IMPLEMENTATION BLOCKERS:**

- Deterministic published/minified consumer JNA struct initialization failure: public `ArtiTorClient()` cannot construct; required UniFFI runtime FieldOrder annotation is absent from installed R8 DEX. Reproducer above.

**RELEASE BLOCKERS:**

- Minified release runtime JNA/UniFFI/callback gate failed, before version/bootstrap/traffic.
- Existing integrated Android acceptance did not pass. Resolve the observed root/session resume-completion ordering versus the test contract; its tail and the other explicitly unverified mandatory checks still require completion after remediation.

**NON-BLOCKING FOLLOW-UPS:** No unexplained external Tor reliability failure was established. Idle cap/resource observations are evidence, not a performance or memory guarantee.

**ARTITOR 0.3 ANDROID HARDWARE GATE: REMEDIATION REQUIRED**

**ARTITOR 0.3: READY TO TAG** is not established. Do not tag this candidate on the basis of this run.

## Appended Android R8 remediation run — 2026-10-01

This section supersedes the earlier release disposition without changing its historical results. Focused mechanism, artifact-chain, mutation, callback, resource and regression details are in [ARTITOR_0_3_ANDROID_R8_REMEDIATION.md](ARTITOR_0_3_ANDROID_R8_REMEDIATION.md). Durable evidence: `docs/audit/evidence/0.3/android-r8-remediation-2026-10-01/`.

Baseline was still `628c6ea13c7286c9a83fb4c028e2bbcbb66b58fb` at remediation entry. Physical Google Pixel 6a, serial `26101JEGR21458`, Android 17/API 37, arm64-v8a, emulator flag 0. This run used wireless transport `adb-26101JEGR21458-ZDzyCZ._adb-tls-connect._tcp`.

### Narrow changes and local commits

- `a64ebb4526c64a83ea85806578b445bcc5963ea0`: library R8 class rule for `uniffi.runtime.** extends com.sun.jna.Structure`, allowing shrinking/optimization/obfuscation; permanent generic DEX regression, standalone minified Maven fixture and CI invocation.
- `ff29a474e7e6d157a58361015192b6eea36c64fc`: Android acceptance waits under a 30-second bound for session StateFlow ACTIVE/current endpoint after checking root readiness; retained test-only manifest rationale and diagnostic probe.
- Reports/evidence are recorded in the subsequent documentation commit. Its SHA is supplied in the final response and can be resolved with `git log -1 --format=%H -- docs/audit/ARTITOR_0_3_ANDROID_R8_REMEDIATION.md`.

No Rust behavior, public API, isolation, bridge/onion/timeout policy, Cargo feature, dependency version or Apple production source was changed. No production resume barrier was justified: frozen root readiness and per-session usability are separate, while pause explicitly promises synchronous-before-return transitions. The accepted freeze does not explicitly promise an all-session callback-publication barrier before resume returns. The three unrelated pre-existing review/probe files remain untouched and uncommitted.

### First results preserved, mutation and published-artifact proof

The original exact minified APK again failed construction with missing `RustBufferStruct$ByValue` FieldOrder. Original classfile has `capacity,len,data`; installed original DEX lacks it. The permanent verifier fails against this APK for 15 runtime annotations.

After `:tor:publishToMavenLocal -PreleaseBuild=true`, a fresh source/config-only consumer using **only** `io.github.yet300:tor:0.3.0` was built with release minification, optimized defaults and refreshed dependencies. It has no extra keep rule, direct coroutine workaround, project dependency or native override. Source rule → local Maven Android AAR `proguard.txt` → fresh merged R8 configuration → final installed DEX is demonstrated. All 17 surviving / 18 original annotated structures pass exact runtime FieldOrder/field checks, including RustBufferStruct, UniffiRustCallStatusStruct and callback vtables. The unused eighteenth class may shrink.

Installed APK equals built APK byte for byte, SHA-256 `fd77d95d7116a9d665e94a1cd57f8485fda22576bd523cd011182d16f9c524d6`. Published arm64 native bytes reproduce installed bytes after the standard NDK strip operation; no override was used. Full mapping/configuration/seeds/usage/APK are retained in temporary build output with hashes in durable evidence.

The first fixed runtime log, `minified-postfix-first.txt`, ends **GATE_COMPLETED,PASS**. Package replacement automatically recreated the old foreground activity with its previous `first` run label. That same process continued after returning to the foreground; its background interval during instrumentation makes elapsed onion timing unsuitable for a performance conclusion. No first failure was replaced. Removing only the new rule in a disposable artifact repository makes both generic DEX regression and physical-device construction fail again; restoring the fixed APK yields another full **GATE_COMPLETED,PASS**. Authoritative checkout and real Maven-local publication were never mutated for this proof.

### Actual minified runtime coverage

| Mandatory behavior | Captured fixed result |
|---|---|
| Public KMP → JNA → UniFFI → Rust | Construction/version/start PASS; native version 0.3.0 |
| JNA metadata/loading beyond construction | RustBuffer/CallStatus, callback vtables and real error transport PASS |
| Native status/log/error/session callbacks; StateFlow | PASS, including asynchronous Bind/BIND, ERROR + typed lastError; session PAUSED→INVALIDATED |
| Root and A/B traffic | PASS, explicit unresolved SOCKS names, distinct root/A/B endpoints |
| Close A leaves root/B usable | PASS, CLOSED/null and dead A listener; root/B HTTP still works |
| Pause/resume | PASS, null endpoints, old listeners refuse, retained client, bounded B ACTIVE wait/current-endpoint traffic, no cold bootstrap |
| Positive public onion / policy rejection | PASS; policy false yields expected SOCKS 5 + categorical ForbiddenStreamTarget |
| Malformed onion | Expected SOCKS 5 + InvalidStreamTarget |
| Malformed bridge/config preservation | PASS, Config in AUTO/ON/OFF and ON-empty; valid running root/B preserved and usable |
| Shutdown, INVALIDATED old handles, restart | PASS; no client/endpoints, old listeners refuse; fresh root/session traffic succeeds |
| Cap/resources/listener recovery | PASS observations below; 33rd rejected Runtime/RUNTIME |
| Bridge secret redaction | Public exceptions and collected public logs PASS; filtered Logcat returned no entries, so full first-process Logcat coverage is unverified |

TARGET_REJECTED evidence is specifically a categorical native log delivered to Kotlin alongside expected SOCKS rejection. The frozen public facade has no per-target connect call; a thrown public TARGET_REJECTED exception from the minified SOCKS client was not observed or claimed. Typed Config, Runtime/RUNTIME and asynchronous Bind/BIND are actual public exception/error transport. All requests use explicit SOCKS; the standalone fixture has no direct DNS/socket fallback. Expected negative-test SOCKS 5 is not a flaky external fetch.

### Loaded device resource observations

Original idle 0/1/8/16/32-session FD measurements remain intact above. New first fixed minified flow:

| Additional sessions / phase | FD | Threads | RSS KiB | PSS KiB |
|---|---:|---:|---:|---:|
| 0, loaded root | 153 | 53 | 138244 | 58362 |
| 1, used B | 152 | 52 | 139716 | 58536 |
| 8, two used sessions | 160 | 52 | 139800 | 58458 |
| 1, capacity samples (B used) | 152 | 53 | 138028–138280 | 56829–56893 |
| 8, B used | 159 | 53 | 139060 | 57521–57585 |
| 16, B used | 167 | 53 | 139580 | 57805–57869 |
| 32, B used | 183 | 53 | 140360 | 58621–58693 |
| Close 31, B retained | 152 | 53 | 141588 | 59717 |
| Before shutdown | 151 | 51 | 133560 | 61049 |
| After shutdown | 139 | 43 | 131680 | 54331 |
| Final shutdown | 139 | 46 | 136012 | 56302 |

Three samples per capacity row; 33rd creation fails with public Runtime/RUNTIME. Removed listeners individually refuse connections. Shutdown removes all endpoints/client and reduces FD 151→139. Initial whole-process FD 121 is not regained; no exact memory/thread baseline recovery or leak-free guarantee is inferred. Timing: cold 20806 ms, pause interval 1024 ms, resume 11 ms, restart 8025 ms.

### Regression results and latest release disposition

Cargo fmt PASS. Regular and single-threaded host-permitted Rust suites both PASS: 97 tests each, 0 failures/ignored. Initial sandbox regular suite was preserved as 52 pass/45 fail due to bind PermissionDenied; host-permitted execution removed that execution restriction. Release bindings/iOS-arm64 compile/Android device-test assembly PASS.

First iOS simulator suite: 103 tests, 102 pass/1 fail (`liveTwoSessionsLifecycle`, `recv failed (eof)`). The one unchanged failing-test diagnostic passes. This remains a reliability follow-up, not an erased first failure.

First post-remediation connected Android suite: **3 tests, 1 pass/2 fail**, XML time 115.462 s. Probe passes; old bootstrap test fails post-resume HTTPS with SSLHandshakeException `connection closed` (line 84); integrated test fails A's first HTTP SOCKS request with `SOCKS: Connection refused` (line 141).

One unchanged diagnostic connected suite: **3 tests, 2 pass/1 fail**, XML time 186.242 s. Probe and the complete integrated test pass, including the corrected bounded resume wait. Old `bootstrapFetchPauseResume` fails the **initial** `assertTorExit` at line 65: SocketTimeoutException `Connect timed out` while reading the SOCKS reply for `check.torproject.org:443`. No second diagnostic retry was made; no Tor behavior was changed. Cause is unknown; external network attribution is not proven.

**IMPLEMENTATION BLOCKERS:** No remaining deterministic R8/JNA/UniFFI/callback/isolation/lifecycle failure reproduced after remediation. Repeated HTTPS-request failure cause is unknown.

**RELEASE BLOCKERS:** Complete mandatory Android connected suite still fails after the allowed unchanged retry. The condition for PASS WITH RELIABILITY FOLLOW-UP is therefore not met. First-process nonempty filtered Logcat coverage also remains unverified.

**NON-BLOCKING FOLLOW-UPS:** Preserved iOS EOF passed unchanged retry; upstream Ubique FieldOrder consumer-rule follow-up is suitable but none was sent; process resource figures are observations. Investigate remaining Android request reliability and close Logcat coverage before a release decision.

**ARTITOR 0.3 ANDROID HARDWARE GATE: REMEDIATION REQUIRED**

READY TO TAG is not established. No tag, push or remote publication occurred.
