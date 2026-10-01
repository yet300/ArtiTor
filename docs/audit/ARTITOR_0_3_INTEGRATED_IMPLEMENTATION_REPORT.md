# ArtiTor 0.3 integrated implementation report

Implementation run started 2026-09-30 and finalized 2026-10-01 (Asia/Tbilisi).

**Implementation is complete and ready for final independent review. Release is not ready to tag: Android hardware acceptance remains pending.** This report records implementation evidence; the separate final independent review has not been performed.

Initial HEAD: `695990a1a9ed55278c79275d77588cdc48bf028c`.
Initial dirty state: two untracked user-owned files, preserved without modification: `docs/audit/ARTITOR_0_3_PHASE2_INDEPENDENT_AUDIT.md` and `tor/src/iosTest/kotlin/com/yet/tor/IndependentReplacementAuditProbe.kt`.
Local checkout is authoritative. Phase 1 accepted at `86da3c8dc71025278df66d588126cc0bef475b49`, closure `5c506b93a5381bb570c39373118d63e7733204db`; permanent regressions were rerun. Phase 2 initial implementation `8bb0d95a4a865b1d322086e2b2ec37c26fb22c57`.

The full current freeze, module structure, capability audit, Phase-1 independent audit, Phase-2 implementation report and modularization report were inspected before implementation. Historical withdrawn proposals do not authorize features. No dormant, PT, hosting, RPC, direct-stream or live-reconfigure work is in scope.

Baseline SHA-256 production inputs:

| File | SHA-256 |
|---|---|
| `rust/arti-kmp-ffi/src/error.rs` | `b0277678b0e5a986aaadffb88bfa2ca70f780fd62cb862da9bb8aff417a3b6aa` |
| `rust/arti-kmp-ffi/src/config.rs` | `e18a436b87f4e8bc50648508d43d9bf4abbe9ca64df0fe4009e99b3c0e1c7dc5` |
| `rust/arti-kmp-ffi/src/lib.rs` | `6870bb8d8a02e72ae49fdb62d5ffa909436650490e53bb391d9915f8b7d3945f` |
| `rust/arti-kmp-ffi/src/logging.rs` | `4af58c1cef0103088c4f7c2d2f4a2a73a875ddb6655066af20f2cd19c7bbdf6e` |
| `rust/arti-kmp-ffi/src/ffi.rs` | `b288def174b3ab8c248acf2004cddb9a850e996901bf1b675be5a215d0bccf1b` |
| `rust/arti-kmp-ffi/src/socks.rs` | `735324801f82a7e53f76a50f71256ef1e08e77d4198890882e442ad4336b521a` |
| `rust/arti-kmp-ffi/src/engine/dispatch.rs` | `eb44344283f919dba0d7d9785482d7f53ebea23d3d59caf67a6dfb8e57bc7d31` |
| `rust/arti-kmp-ffi/src/engine/bootstrap.rs` | `583731c91f8fd66c217e0513c31baff6f97c396c8e9969e97f175d9e8249e89e` |
| `rust/arti-kmp-ffi/src/engine/mod.rs` | `245ed49bcb017c220a0d439cebc597529795ca6099c1f522c414196bdeb3c3b3` |
| `rust/arti-kmp-ffi/src/engine/lifecycle.rs` | `3c30a074a430ea8102b79809db7ba736b0515ddd9c5b52490be26da7fcc1e2e9` |
| `rust/arti-kmp-ffi/src/engine/root_socks.rs` | `9c7c75d7839b8af1e2769ffee13463877b880e3e0d4da3ff46b412c457b54529` |
| `rust/arti-kmp-ffi/src/engine/test_hooks.rs` | `27ad07c255f0e9ca1115d767722ab577c707319d11fd8173bcf31971f8b38ec2` |
| `rust/arti-kmp-ffi/src/engine/session/mod.rs` | `6bf7122ced6a962302eaadfdfe77a491642d58046c134936e21ff682e07d08b1` |
| `rust/arti-kmp-ffi/src/engine/session/listener.rs` | `fef46a66b48317df5e634ae75ba73f5b8ca7e11bb9ec538e143a55fd4c89ac35` |
| `rust/arti-kmp-ffi/src/engine/session/admission.rs` | `7eabb78c308cb7f2cb51bdccce43592452bb7b815e31c503c2b05a4c6824c8d8` |
| `rust/arti-kmp-ffi/Cargo.toml` | `af10230fff1048d59e65c4b6c2d66652394b17c4e71a2ac55ac92d4a971f5942` |
| `rust/arti-kmp-ffi/Cargo.lock` | `6670be49a0e87563287249f94b113acb523168bc4d7c2d40e35837da3ca5eab5` |
| `rust/hide-sqlite3-symbols.sh` | `eeb578bf6c323a2aaa310fa8fb694bc1e9a7a3ca21092045a3588d690e1e0604` |
| `tor/build.gradle.kts` | `b9741b8aaf45e8adea758f9e34d189059305d2b2552cd087b093ebcc03d90afc` |
| `tor/src/commonMain/kotlin/com/yet/tor/ArtiTorClient.kt` | `6fa923becc15f2acbdc11e1e488631fb6a0fadb9fadb19782736e4b3b959b2cf` |

## Execution checklist

- [x] Phase-2 transactional validation and old positional source repair
- [x] Phase-3 stable classification and pinned mapping
- [x] Phase-4 onion policy and exact duration controls
- [x] Phase-5 regression, live, packaging, consumer, resource and mutation evidence
- [x] Final API/scope/privacy self-review and release readiness disposition

Temporary raw logs, baseline feature graph, artifact-size snapshot and intermediate evidence are under `/tmp/artitor-integrated`. Durable conclusions and unsuccessful attempts are recorded below. The execution checklist denotes completed implementation/local verification work; it does not override the pending hardware gate.

## Phase-2 remediation and compatibility evidence

The supplied independent audit identified shutdown-before-validation for RUNNING replacement and mutable bridge-list identity aliasing. Two original offline actual-parser probes were executed unchanged before repair: both failed (2 tests / 2 failures, Gradle exit 1). PAUSED already validated before native mutation; the permanent corpus covers it as well.

`validate_config(ArtiConfig)` is a free internal UniFFI export using the same Rust `build_tor_config` helper as construction. Kotlin preflights before RUNNING pause/shutdown and before restart; PAUSED construction already validates first in Rust. Bridge lists are copied before comparison, validation and retention as the effective successful config. No bridge parser or public validation API was added. Invalid Kotlin port values outside 0..65535 fail existing Config before u16 conversion.

The current 0.2 constructor was verified from historical source. Its five parameters remain first, and `bridgesEnabled` is appended. Compile-only fixture covers positional calls of arity 1–5, named calls, lifecycle/status/log access, and the seven-class exhaustive `when` without `else`. This corrects the freeze sketch's ordering conflict with its explicit source-compatibility gate.

Verification attempts:

- Baseline audit probes: expected RED, 2 failures. Their original XML was overwritten by later Gradle runs; observed tool output remains evidence, not a claim of retained red XML.
- First postrepair selection: 43 tests / 1 failure because an intended-valid fake fixture used unsupported obfs4 and invalid fingerprint. Replaced only intended-valid fixtures with synthetic direct documentation-IP lines; no production parser weakened.
- Final offline selection: 88/88 tests, zero failures/errors/skips. Counts: lifecycle25, session30, identity10, invariants6, error6, bridge2, transactional6, supplied audit probes3. Normal source sets; live setup probe excluded.
- Expanded old-source fixture: compileTestKotlinIosSimulatorArm64 PASS after adding arities1–4.
- Native sandbox attempt: exit101,38 pass/43 failures; local socket permission denied. Approved rerun:81/81,0 doc tests, exit0.
- cargo fmt --check and git diff --check: PASS.

Final offline XML snapshot: `/tmp/artitor-integrated/transaction-simulator-xml`. Native unsuccessful/successful logs are separate `transaction-native-{sandbox,approved}.log` files. These tests distinguish scripted public lifecycle resources with actual Rust validation from real native retained-client probes and live Tor networking.

## Phase 2 checkpoint and stronger native retention proof

Checkpoint `50a62a4dc15089cbe0a52de02a7cd2138ca640a3`. An additional native retained-root probe binds real local SOCKS sockets and checks RUNNING/PAUSED rejection against root/session Arc identities, endpoints, client epoch/generation, worker identity/revision, bootstrap state, listeners and last config. Its sandbox attempt failed on socket permissions; approved targeted rerun passed1/1 (81 filtered). The original independent audit and its untracked probe were preserved.

## Phase 3 stable classification

The public enum has exactly15 categories, and all seven sealed exception subclasses remain. Legacy constructors retain their kinds. Runtime accepts an optional richer kind; Bootstrap keeps its public one-string constructor and uses an internal richer constructor. Synchronous FFI errors and asynchronous callback details carry an independent stable category while retaining the original outer error class. No text parsing is used.

Classification exclusively reads `HasKind::kind()`. The following table exhaustively lists all59 variants in pinned `tor-error0.46.0`; the non-exhaustive wildcard returns UNKNOWN. Construction/bootstrap messages are generic safe operation descriptions; startup logging no longer emits the private data directory. SOCKS errors retain only the upstream categorical enum and safe operation context. Session snapshot/close operations remain infallible; terminal category transport is tested without inventing public throw triggers.

| Stable category | Pinned upstream variants |
|---|---|
| Network | TorAccessFailed, DirectoryExpired, TorProtocolViolation, LocalNetworkError, RelayIdMismatch, CircuitCollapse, TorNetworkTimeout, TorDirectoryError, RelayTooBusy, CircuitRefused, NoPath, TorDirectoryUnusable, ClockSkew, TorDocumentRejected |
| Storage | PersistentStateAccessFailed, LocalResourceAlreadyInUse, FsPermissions, PersistentStateCorrupted, CacheCorrupted, CacheAccessFailed, KeystoreCorrupted, KeystoreAccessFailed |
| ExitFailed | RemoteNetworkTimeout, RemoteStreamClosed, RemoteStreamReset, RemoteStreamError, RemoteConnectionRefused, ExitPolicyRejected, ExitTimeout, RemoteNetworkFailed, RemoteHostNotFound, OnionServiceNotFound, OnionServiceNotRunning, OnionServiceProtocolViolation, OnionServiceConnectionFailed, RemoteHostResolutionFailed, RemoteProtocolViolation, NoExit |
| TargetRejected | OnionServiceMissingClientAuth, OnionServiceWrongClientAuth, OnionServiceAddressInvalid, InvalidStreamTarget, ForbiddenStreamTarget |
| Config | InvalidConfig, InvalidConfigTransition, NoHomeDirectory |
| BootstrapRequired | BootstrapRequired |
| Runtime | ReactorShuttingDown, ArtiShuttingDown, SoftwareDeprecated, NotImplemented, FeatureDisabled, LocalProtocolViolation, LocalResourceExhausted, ExternalToolFailed, TransientFailure, BadApiUsage, Internal |
| Unknown | Other |

Verification attempts: initial Kotlin compilation was red for the missing public enum/kind API (and stale generated constructor usages); earlier sandbox compile failed on Gradle cache permission. Generated constructor migrations use typed categories. Native parallel and serial suites each passed87/87 with no skips. The first full simulator suite passed95/95, including three live networking tests (two existing live tests plus the supplied independent live probe). A further synchronous all-category transport regression was added afterward and passed in the checkpoint matrix reported below.

Sources: locally resolved `tor-error-0.46.0/src/lib.rs:133–751`, `arti-client-0.46.0/src/lib.rs:72`, and `arti-client-0.46.0/src/err.rs:393–454`. Native mapping/table tests use the resolved pinned APIs, not diagnostic representations.

Phase3 checkpoint matrix: buildBindings, compileKotlinIosArm64, assembleAndroidDeviceTest and final ErrorClassificationTest5/5 all pass (Gradle59s). `phase3-platform-matrix.log`, preserved XML and generated bindings snapshot under `/tmp/artitor-integrated`. Public source compatibility and exhaustive seven-branch fixture compiled in the ordinary common test source set. Native class count remains6; public sealed class count7; sole public additions are TorErrorKind/ArtiException.kind and optional Runtime kind. Internal Bootstrap richer constructor is documented in the freeze without changing its public legacy constructor.

Phase3 checkpoint: `4bd683bc8e898fde103dca51d13e20d2e72ddf18`.

## Phase 4 onion policy and stream deadlines

Appended `allowOnionAddrs=true`, `connectTimeout=10.seconds`, `resolveTimeout=10.seconds` after the original five constructor fields and bridge policy. Corresponding FFI defaults preserve older fake constructors. The three fields are TorClient-defining in both production comparators; only socksPort remains listener-only. Kotlin snapshots bridge lists and validates representability/preflights Rust before teardown. Rust owns bridge/negative-timeout semantic validation and builds the whole upstream config before mutation.

Signed64 nanos are converted without saturation or truncation. Kotlin requires finite durations and exact `.inWholeNanoseconds.nanoseconds` roundtrip. Negative exact values reach Rust and return CONFIG through checked unsigned conversion. Kotlin tests include negative1ns, infinity, finite10bseconds, first overflowing milliseconds, zero and the largest representable millisecond duration. Native tests include1ns precision and signed64MAX.

Actual built TorClientConfig equality is checked against independently configured upstream builders, including untouched upstream defaults. Whole-config comparison is necessary because pinned Arti keeps nested address/timeout fields private. Actual unbootstrapped `TorClient.connect` rejects a valid v3 onion with policyfalse as ForbiddenStreamTarget and malformed.onion as InvalidStreamTarget, both TARGET_REJECTED through the production classifier. No host matching or simulated target error is used.

The configured2ms/4ms deadlines expire genuinely pending futures through `PreferredRuntime` and the same `SleepProviderExt::timeout` primitive Arti consumes. This proves conversion and runtime deadline behavior; it does not directly exercise private Arti BEGIN/resolution machinery against a black-holed relay. Pinned source inspection locates connect timeout around BEGIN after circuit acquisition (`client.rs:1566–1595`) and forward resolve around `circ.resolve` after circuit acquisition (`client.rs:1699–1712`). There is no promise that these bound bootstrap, circuit acquisition, DNS in an app, or a whole HTTP request. A stronger private-BEGIN test would need an invasive upstream circuit seam; no such mock is added.

A shared512-case field-change table checks each real comparator independently against the same oracle (all combinations of8 client fields and listener-only socksPort), both directions and identical clones. This establishes agreement over the table without adding a comparator export to the FFI API. Existing every-mode/exact-order/whitespace bridge identity cases remain.

Native attempts retained: first check failed trying nonexistent AsRef accessors for private nested upstream config; changed tests to whole-config equality. First full test invocations found a missing newly required field in the host example; repaired its defaults, then all-targets check and parallel/serial94-test suites passed. The further512-case property test passed. Platform/live attempt1 is recorded separately, with no retry concealed.

Live fixture: the Tor Project lists `arti.torproject.org` at publicv3 `hjirlp6fu47kox4cnede4zlvaeq672bibss3oxgmsnsc5mdxygqshbqd.onion` in its [official onion list](https://onion.torproject.org/), verified2026-09-30. The integrated test routes that domain through normal SOCKS, checks HTTP200 or a valid HTTP redirect, rejects malformed onion, verifies failed bridge replacements in every mode preserve usable root/B, shuts down, proves old handles dead, cold-restarts a fresh working session, then rebuilds with policyfalse and checks SOCKS0x05 plus a still-usable normal root. POSIX socket I/O has60s send/receive deadlines; a coroutine wait alone would not interrupt blocking POSIX calls.

Phase4 platform/live attempt1: Gradle exit0 in4m12s; buildBindings, device compile and Android test APK assembly pass. Simulator100/100,0 failures/errors/skips. Includes three live tests; integrated lifecycle/root/onion/negative/badreplacement flow passed. Native final512-property full suite95/95; prior serial94/94 passed before adding the pure property. Final Kotlin512-case selection passed12/12 because the first full run compiled before that test was added.

First integrated live timing: cold readiness8623ms, retained resume2ms (evidence only). Safe code5 observations: malformed.onion→InvalidStreamTarget and disabled onion policy→ForbiddenStreamTarget, both expected negatives. No unexpected code5 appeared in this run or Phase3's first full suite. This does not disprove previously recorded intermittent network failures. No environment attribution or retry was needed for the successful current live run.

Simulator resources from the same bootstrapped process:0sessions FD18;1 FD19;8 FD26;16 FD34;32 FD50, each active count sampled3 times; closed-all FD18. Lifetime peak RSS174931968bytes at every sample, dominated by earlier process allocations; this cannot establish zero incremental memory cost or memory reclaim. Unique active IDs/ports, listener readiness, and33rd-session refusal were checked. This is a simulator foreground measurement of idle sessions; it does not replace mobile hardware or loaded-circuit measurements. Keep the internal32-session cap conservative and revisit against mobile evidence.

Phase4 checkpoint `c00397a7fceec179e9f7482ea03971998d1ad4ba`. Final Kotlin identity selection12/12 includes the512-case production comparator property; no live rerun required for that pure test addition.

## Phase 5 release hardening, attempts and privacy repair

Version updated to0.3.0 in Gradle and the Rust root package/lock only; no remote publication/tag. The actual application sample is compiled from docs/examples in commonTest, with meaningful tests for separate identity pools, status/endpoint rebuilds, independent close, failed initialization and terminal fail-closed behavior. Its HTTP factory remains application-owned and contractually requires SOCKS remote-name routing and fresh per-identity pools. Android instrumentation now stages the corresponding integrated live flow and0/1/8/16/32 resource sampler for hardware execution; the previous arbitrary30s resume assertion became timing evidence only.

Publication attempt1 failed at Gradle script compilation because commonTest is a provider, not a source-set instance; changed sample source routing to getByName("commonTest"). Publication/platform attempt2 passed in2m29s: local0.3 metadata/artifacts, Android instrumentation assembly, simulator common/sample test compilation. Initial artifact verifier failed because this invocation used the development path and packaged onlyarm64. The existing normal release workflow supplies `-PreleaseBuild=true` to include allthree Android ABIs and release native profiles; final evidence will use that explicit flag. The first incomplete local artifact is not accepted release evidence.

Final native serial suite after root version update95/95 passed. Full simulator100/100 had already exercised actual new configuration and the entire live matrix before pure identity/sample additions.

Privacy review found a concrete inherited forwarding defect: ForwardLayer formatted all upstream INFO/WARN event fields with Debug. Pinned `tor-guardmgr0.46.0/src/guard.rs:777,780` warns with guard Debug; GuardId→RelayIds→RSA identity Debug exposes the complete fingerprint, including bridge guards. `tor-persist/fs.rs:144` can emit an app state path, and raw panic forwarding exposes arbitrary payload/source location. This contradicts the frozen logging rule. Repair keeps direct safe lifecycle/SOCKS category callbacks and LOG_SINK ownership unchanged, but forwards only event level/compiler module metadata and a static panic description. Payload-redaction regression evidence passed before the final artifact rebuild.

Final release publication attempt3 (with releaseBuild flag and privacy repair) passed in1m9s. Fresh Gradle hook localization messages were observed for both Apple release targets. Actual published AAR contains arm64-v8a/armeabi-v7a/x86_64 only. Artifact verifier attempt2 passed: all64-bit Arti LOAD segments16KB aligned with offset/address congruence; each pristine Cargo Apple archive has283 global_sqlite3 symbols, each posthook archive and the actual published cinterop archive has0. The same checks also inspect available debug-profile pairs. Symbol counts are direct nm results; hook hash alone is not used as proof.

Clean consumer attempt1 correctly failed common and Android compilation because StateFlow/SharedFlow were public API types but coroutines was published as implementation. This was a real inherited metadata defect, not a reason to add an extra consumer workaround. Changed the existing commonMain coroutines dependency to api (no new dependency/version), then local release publication attempt4 passed in25s. Consumer attempt2 uses exactly the same isolated published-coordinate fixture and is recorded separately: Android compilation now passed, common metadata still failed. Attempt3 with refreshed dependencies confirmed the common metadata failure; a separate fixture/classpath investigation follows.

Cargo tree -e features is exactly equal after normalizing only arti-kmp-ffi0.3→0.2:966 unique package/feature pairs, zero additions/removals; equal normalized SHA256 `3e5e1e3bdbdbd5c927055671ec5b61ffef30284b4be5d18601eac3d3076ac357`. Manifest/dependencies/lock equality also verified except rootversion. Root arti-client experimental-api is absent. The resolved graph does include tor-cert's internal experimental-api transitively, already identically present in the accepted baseline; it was neither enabled nor exposed by this run. Whole-graph zero-delta evidence distinguishes this existing internal dependency feature from forbidden new public Arti API exposure. Full privacy native suite97/97 parallel and97/97 serial passed,0 doc tests. Adversarial event red test failed against old production formatting and passed after payload suppression; two green tests exercise production formatter via isolated subscriber and panic redaction. Ownership/callback order remain unchanged.

Regression sensitivity evidence is preserved under `docs/audit/evidence/0.3`: four native Phase2/3 mutations, one KotlinCAS mutation with two terminal-history regressions, and four Phase4 mutations (each3field identity omitted independently, onion policy ignored). All baselines passed; all mutants compiled and failed their intended assertions. No disposable source mutation was applied to the production checkout. Exact patches and command/result JSON are included. Sensitivity verification is not the separate final independent review.

Consumer attempt3 still failed common metadata after refresh because Gradle retained an up-to-date transformation from the earlier same-coordinate publication. Dependency/classpath inspection confirmed correct published API metadata and resolution, but the transformation index contained only tor/stdlib. Recomputing the standard transform with `compileCommonMainKotlinMetadata --rerun-tasks --offline` passed and included transitive coroutines commonMain/concurrentMain, without fixture/dependency changes. A fresh directory without build/.gradle/.kotlin data is being checked to establish clean consumer behavior rather than relying on stale transform outputs.

Freeze reconciliation also explicitly resolves a future-operation sketch: stable0.3 status snapshots and close remain infallible; there is no public fallible terminal-handle operation to throw SESSION_CLOSED/SESSION_INVALIDATED. These categories exist in Runtime transport for future fallible resource operations. No new session API or throw trigger is invented; accepted terminal lifecycle behavior is unchanged.

Clean consumer common/Android compilation passed normally in a fresh directory with source/config only and no build/.gradle/.kotlin output, using solely the published tor0.3 dependency. This verifies the API dependency repair independently of the stale transformed original directory.

Final release-profile simulator attempt1:103 tests,102 pass/1 failure,0 errors/skips, Gradle2m16s. All100 non-live tests passed, including sample2tests and512-property; integrated session/root/onion/config flow and supplied independent live replacement probe also passed. Legacy bootstrapFetchPauseResume failed on its second cold-start HTTP fetch at TorIosE2ETest.kt:189; safe category RemoteNetworkTimeout, SOCKS0x05. The same run also recorded the two expected onion negatives InvalidStreamTarget/ForbiddenStreamTarget. Default upstream-config equality and10s transport/default tests rule out an intentional timeout/default change, but do not identify the remote cause. RemoteNetworkTimeout maps EXIT_FAILED; it is not automatically classified as a host environment outage or proven unrelated to the wrapper. One unchanged diagnostic rerun is recorded separately; the failed full-suite result is retained and is not overwritten by a later pass. Historical intermittent code5 is therefore observed again, now with a stable safe category. Its underlying cause remains a follow-up pending reproducible network/circuit evidence.

One unchanged live diagnostic rerun of bootstrapFetchPauseResume passed1/1 in23.266s (Gradle32s), no code5. It does not establish a fix or remote cause; the full103-test failure is retained. The user specifically directs preservation as a reliability follow-up absent an isolated product defect (§50).

Android gate was attempted, not inferred from compilation: connectedAndroidDeviceTest with releaseBuild failed exit1 after1m5s with `DeviceException: No connected devices!`. Earlier adb discovery also showed no devices. Instrumentation APK compiled/assembled successfully, including integrated session/onion/resource cases, but current0.3 hardware behavior and minified runtime callbacks are ENVIRONMENTALLY UNVERIFIED. Freeze§13 explicitly requires Android hardware, so tagging/publishing remains blocked pending that gate and the separate final independent review.

Clean consumer full attempt1 compiled common/Android/iOS sources and reached Android packaging, but lintVitalAnalyzeRelease exhausted default Gradle metaspace (`OutOfMemoryError: Metaspace`,13s). Only the verification fixture JVM capacity was raised to4GiB heap/1GiB metaspace, with2workers; no lint/R8 tasks disabled and no consumer keep-rule/dependency workaround added. Full attempt2 is recorded separately.

## Final release checkpoint and available-gate disposition

Phase 5 checkpoint: `6ea7f0babdb6777ad33d85a90cd07e3cf6512a18`.

Clean consumer full attempt2 passed in 10 seconds with lint and R8 enabled. Common/Android compilation, iOS device/simulator compilation, simulator framework linkage and unsigned minified Android release APK assembly all completed. The fixture uses only the locally published `io.github.yet300:tor:0.3.0`; no project dependency, direct coroutines declaration, app keep-rule workaround or native build is required. Runtime dependency resolution includes JNA AAR 5.19.1 and Ubique runtime-android 1.2.1 transitively. Coroutines is now correctly an API dependency because flows appear in the public wrapper signatures.

Final artifact verifier with consumer checks passed. Minified APK is 24,171,691 bytes, contains exactly arm64-v8a/armeabi-v7a/x86_64 and all three native libraries per ABI (Arti, JNA dispatch, UniFFI runtime). DEX and R8 mapping retain com.sun.jna.Native and generated ArtiTor names through published consumer rules. All six 64-bit JNI libraries pass LOAD alignment and offset/address congruence at 16 KB. Actual linked simulator consumer framework also has zero globally defined sqlite3 symbols. Runtime reflection/callback behavior under R8 still needs hardware execution.

The Apple hide script is byte-identical to baseline and runs from the normal Gradle hook. Both fresh release archives and actual published cinterop archives have been inspected, not merely hashed. A pre-existing deployment-version difference remains a follow-up: symbol-localization prelink uses iOS15 while Cargo config sets iOS13. Current simulator consumer linking succeeds; oldest-device deployment compatibility was not established here.

### Final verification matrix

| Gate | Result / evidence limit |
|---|---|
| Phase 1 invariants | Preserved; native ownership/isolation/dispatch/generation/publication corpus and Kotlin terminal/CAS/lifecycle corpus pass |
| Native final | 97/97 parallel and 97/97 serial; no failures or ignored tests; doc tests0 |
| Full final simulator | 103 run:102 pass,1 remote-timeout live failure; no errors/skips. All100 non-live tests pass. Unchanged failed-test rerun1/1 passes; original failure retained |
| Integrated live root/session/onion/config | Pass in Phase4 and final Phase5 release-profile run; no port-stability or exit-IP-isolation assertion |
| Android instrumentation | APK compiled/assembled; actual connected gate fails solely on no connected devices |
| Maven consumer | Fresh common/Android/iOS compilation, simulator framework linkage, lint and R8 assembly pass |
| Android native package | Exactly3 supported ABIs; 64-bit Arti libraries and all6 consumer JNI libraries16 KB aligned |
| Apple SQLite | Both pristine release archives283 globals → both posthook0; published cinterop0; linked consumer framework0 |
| Cargo features | Exact normalized graph equal;966 package/feature pairs; additions0/removals0 |
| Compatibility | Original1–5 positional/named ArtiConfig calls and exhaustive seven-exception when compile in library tests and independent Maven consumer |
| Public API | Frozen enum/kind/config/session scope only; no generated FFI type in public wrapper signatures |
| Mutation sensitivity | All9 required/expanded mutations killed with green baselines; exact patches/results preserved |
| Resources | Real host idle listeners/handles and live simulator0/1/8/16/32 measured; mobile loaded costs remain unknown |
| Publication | Local only; version0.3.0; one root coordinate with normal KMP platform variants; no tag/push/remote publish |

### Like-profile binary-size measurements

All sizes are bytes. These are observed like-profile artifact deltas, not an isolated compiler-only experiment or a memory measurement. Zero feature delta rules out newly enabled Cargo features.

| Artifact | Baseline | Final | Delta |
|---|---:|---:|---:|
| Published/repository release AAR | 10,646,930 | 10,825,882 | +178,952 (+1.68%) |
| Android arm64-v8a release so | 8,036,768 | 8,116,888 | +80,120 |
| Android armeabi-v7a release so | 5,131,140 | 5,182,788 | +51,648 |
| Android x86_64 release so | 9,110,760 | 9,215,448 | +104,688 |
| Cargo pristine iOS device release a | 110,620,240 | 111,646,264 | +1,026,024 |
| Cargo pristine iOS simulator release a | 110,615,840 | 111,641,128 | +1,025,288 |

Stale i686 outputs are neither rebuilt nor packaged and do not count as supported release artifacts. Full profile/path comparisons are in evidence/0.3/release-artifacts.json.

### Resource interpretation

Host native debug probe (real isolated clients and additional session listeners, offline retained-client fixture):32 sessions add32 FDs/32 listeners,0 threads, observed process RSS +992 KiB. Baseline17,360 KiB/12threads/14FD;32sessions18,352 KiB/12threads/46FD; close returns FD14/listeners0; shutdown threads2/FD7. Root listener/network/bootstrap was excluded in this host fixture. The sampler has fixed observer overhead and allocator retention, so this is not an isolated allocation-size sum.

Live simulator foreground root: baselineFD18 → 1session19 → 8sessions26 → 16sessions34 → 32sessions50 → close18. Peak RSS174,931,968 bytes stayed flat because an earlier process peak dominated. Peak RSS cannot prove zero cost or memory reclamation. The33rd session rejects. Retain the internal cap32 conservatively; hardware and loaded-circuit measurements remain follow-ups.

### API / forbidden-feature review

Compared with accepted Phase1/source0.2 shape, the additions are BridgesEnabled with appended bridgesEnabled; TorErrorKind and ArtiException.kind/optional Runtime kind; appended allowOnionAddrs/connectTimeout/resolveTimeout. Original constructor positions1–5 and seven sealed subclasses remain. Bootstrap's richer constructor is internal; its original public constructor remains. Compared with initial HEAD, bridge policy already existed and the latter error/config controls plus compatibility repair are the deltas. Generated Phase3→final snapshots change exactly the three config fields/defaults, serialization/lowering and associated ABI checksums. The small validation export is internal integration, not a wrapper API or new ArtiTorInterface method.

No allowLocalAddrs, defaultSession, immutable plain session endpoint, creation timeout, dormant API, live reconfiguration, PT, service hosting, authenticated onion-client API, RPC, direct streams, advanced passthrough, OpenSSL/native-tls or additional Maven feature artifacts were introduced. Root/session isolation is deterministic via incompatible isolation contexts and actual concrete client dispatch selection; coincident exit addresses are not an isolation test. Concurrent-process LOG_SINK last-writer ownership is unchanged. Compiler module/severity and safe explicit lifecycle/SOCKS category callbacks carry diagnostics; upstream event/panic payloads never cross the new forwarder.

### Final self-review answers

| Required question | Answer and confidence |
|---|---|
| Did Phase1 behavior regress? | No reproduced regression in permanent corpus; high |
| Did bridge validation/regeneration regress? | No; all modes/ONempty/exact identity/retention and mutation tests pass; high |
| Can invalid replacement destroy a valid running client? | Parser/representation rejection preserves RUNNING/PAUSED resources, including restart; high. Valid accepted rebuild failures have no rollback promise |
| Are Kotlin/native identity semantics identical? | Yes across shared512-case table plus exact bridge cases and actual transitions; high |
| New ArtiException subclasses? | No; seven remain; high |
| Classification parses message text? | No; HasKind.kind only; high |
| UNKNOWN fallback present? | Yes; Other/future fallback and forced-fallback mutation tested; high |
| allowLocalAddrs exposed? | No; high |
| Live reconfigure used? | No; high |
| PT enabled? | No; feature graph unchanged and PT-shaped bridges reject; high |
| Dormant implemented? | No; high |
| Ordinary onion connections work? | Public v3 HTTP/valid redirect passed through real session SOCKS twice; high for observed fixture |
| allowOnionAddrs=false rejects onion? | Actual upstream policy before bootstrap and real live SOCKS reject; high |
| Timeouts applied? | Actual final upstream config equality and same-runtime pending-future expiry; high for config/timer behavior. Private BEGIN black-hole path is not directly tested |
| All new config fields rebuild-triggering? | Yes; comparator property plus retained-session invalidation for each; high |
| Cargo features changed? | No; exact normalized graph; high |
| Apple SQLite isolation effective? | Yes in fresh posthook/published/linked artifacts; high |
| Android64-bit libs16KB aligned? | Yes, actual packaged and consumer JNI LOAD segments; high |
| Old0.2 source calls compile? | Yes in standard fixtures and fresh published-coordinate consumer; high |
| Exhaustive exception when compiles? | Yes, seven original branches; high |
| FFI types absent from public KMP API? | Yes in wrapper signatures; generated bindings remain internal integration namespace; high |
| Maven consumer builds? | Yes common/Android/iOS/framework/R8; high. Android runtime smoke remains unknown |

### Release disposition

**Not technically ready to tag0.3.0 yet. Implementation complete; release gate pending Android hardware and final independent review.** No tag or remote publication was performed.

BLOCKER / ENVIRONMENTALLY UNVERIFIED: freeze§13 Android hardware integrated acceptance and minified runtime callback smoke. No attached device. The separate final independent review is still pending, not claimed passed.

NON-BLOCKING FOLLOW-UPS: historical intermittent SOCKS0x05, now observed as RemoteNetworkTimeout in one legacy second-cold HTTP attempt, cause unknown and not claimed fixed; physical iOS/mobile loaded resource costs and cap review; stronger private-BEGIN/resolution black-hole test if a non-invasive upstream seam becomes available; pre-existing iOS deployment-target/prelink minimum mismatch. Simulator timing (8,623ms initial cold /2ms retained resume; later cached cold909ms) is evidence, not a performance promise.

### Commands / durable evidence

Run from repository root, through RTK:

```text
rtk proxy cargo fmt --check                           # rust/arti-kmp-ffi cwd
rtk proxy cargo check --offline --all-targets          # same cwd
rtk proxy cargo test --offline                        # parallel
rtk proxy cargo test --offline -- --test-threads=1     # serial
rtk ./gradlew :tor:buildBindings :tor:compileKotlinIosArm64 :tor:assembleAndroidDeviceTest
rtk ./gradlew :tor:iosSimulatorArm64Test -PreleaseBuild=true --no-configuration-cache
rtk ./gradlew :tor:publishToMavenLocal -PreleaseBuild=true --no-configuration-cache
rtk ./gradlew :tor:connectedAndroidDeviceTest -PreleaseBuild=true --no-configuration-cache
rtk ./gradlew -p docs/audit/evidence/0.3/consumer compileCommonMainKotlinMetadata compileAndroidMain compileKotlinIosArm64 compileKotlinIosSimulatorArm64 linkDebugFrameworkIosSimulatorArm64 :app:assembleRelease
rtk proxy git diff --check
```

Fixture consumer needs the local Android SDK location set for the machine. Symbol inspection uses toolchain llvm-nm `--extern-only --defined-only`; ELF inspection uses NDK llvm-readelf `-lW`. Exact segments, symbols/counts, feature hashes, mutation commands/diffs and consumer package paths are committed under evidence/0.3. Raw per-attempt Gradle/cargo logs/XML remain at `/tmp/artitor-integrated`.

### Checkpoint ledger

| Checkpoint | SHA |
|---|---|
| Initial authoritative checkout | `695990a1a9ed55278c79275d77588cdc48bf028c` |
| Accepted Phase1 | `86da3c8dc71025278df66d588126cc0bef475b49` |
| Prior Phase2 implementation | `8bb0d95a4a865b1d322086e2b2ec37c26fb22c57` |
| Phase2 transactional remediation | `50a62a4dc15089cbe0a52de02a7cd2138ca640a3` |
| Phase3 | `4bd683bc8e898fde103dca51d13e20d2e72ddf18` |
| Phase4 | `c00397a7fceec179e9f7482ea03971998d1ad4ba` |
| Phase5 | `6ea7f0babdb6777ad33d85a90cd07e3cf6512a18` |

This report's own commit and final HEAD are supplied in the completion response and can be obtained with `git log -1 --format=%H -- docs/audit/ARTITOR_0_3_INTEGRATED_IMPLEMENTATION_REPORT.md`. Its SHA cannot be embedded in its own content without changing that SHA. User-owned independent audit/probe files remain untracked and unmodified by this run.
