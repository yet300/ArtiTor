# ArtiTor 0.3 Android R8 remediation

Date: 2026-10-01. **ANDROID R8 REMEDIATION COMPLETE — HARDWARE GATE RESULT: REMEDIATION REQUIRED.**

Confidence: **high** for the reproduced R8 mechanism, mutation sensitivity and captured minified runtime results; **unknown** for the cause of the remaining external-request failures. The narrow implementation blocker is fixed. The complete connected instrumentation suite did not pass its one unchanged diagnostic retry, so READY TO TAG is not established.

## Baseline and change scope

- Baseline: `628c6ea13c7286c9a83fb4c028e2bbcbb66b58fb` (verified before changes).
- R8 remediation: `a64ebb4526c64a83ea85806578b445bcc5963ea0`, `fix(android): preserve UniFFI JNA struct metadata under R8`.
- Test contract: `ff29a474e7e6d157a58361015192b6eea36c64fc`, `test(android): await session activation after resume`.
- This report/evidence commit is the subsequent documentation commit; resolve its exact SHA with `git log -1 --format=%H -- docs/audit/ARTITOR_0_3_ANDROID_R8_REMEDIATION.md`. A commit cannot contain its own hash. Final response records that SHA.

The only production change is an Android consumer rule. Cargo features, Rust semantics, Kotlin public API, UniFFI API, dependency/toolchain versions and Apple production sources are unchanged. Permanent regression infrastructure consists of a standalone Maven consumer, generic DEX verifier and CI invocation. Test-only changes await session activation and preserve the hardware diagnostic probe. The three unrelated pre-existing untracked review/probe files remain untouched and uncommitted. Earlier hardware evidence is retained.

Physical device: Google Pixel 6a, bluejay, serial `26101JEGR21458`, Android 17/API 37, primary ABI `arm64-v8a`, `ro.kernel.qemu=0`. This run used wireless ADB transport `adb-26101JEGR21458-ZDzyCZ._adb-tls-connect._tcp`; this transport name is distinct from the hardware serial.

## Reproduction and R8 mechanism

The exact original standalone minified APK was reinstalled and failed during public `ArtiTorClient()` construction, before version or network use:

```text
Structure.getFieldOrder() on class uniffi.runtime.h$a
does not provide enough names [0] ([]) to match declared fields [3]
([capacity, data, len])
```

Original mapping identifies `h$a` as `uniffi.runtime.RustBufferStruct$ByValue`; `x$a` is `UniffiRustCallStatusStruct$ByReference`. Ubique runtime 1.2.1's original RustBufferStruct classfile contains runtime `com.sun.jna.Structure$FieldOrder(value=["capacity","len","data"])`. The original installed DEX retains the public fields and JNA superclass but loses that annotation. The previous evidence bundle includes original classfile, installed DEX and mapping excerpts; the new `minified-prefix.txt` independently reproduces construction failure on this device.

Both Ubique's consumer rules and the published ArtiTor rules were merged. `-keepattributes RuntimeVisibleAnnotations` and member retention do not, on their own, make these runtime classes eligible for class annotation retention in this processed R8 full-mode graph. ArtiTor's existing generated-binding class keep rule covers `com.yet.tor.ffi`, while the affected runtime structs reside in `uniffi.runtime`.

The added rule is exactly:

```proguard
-keep,allowshrinking,allowoptimization,allowobfuscation class uniffi.runtime.** extends com.sun.jna.Structure
```

Existing annotation retention and JNA member rules already preserve annotation type, reflected field names and constructors. No new blanket runtime keep, optimization disable or app keep rule was introduced. Existing broad JNA rules were left unchanged. Runtime class names remain obfuscated; an unused annotated structure can still shrink.

## Published rule and installed DEX chain

Release publication ran locally:

```sh
rtk ./gradlew :tor:publishToMavenLocal -PreleaseBuild=true --no-configuration-cache
```

`tests/android-minified-consumer` was copied as source/config only to a fresh `/tmp/artitor-android-remediation/consumer`, without old build directories. Its release build used `--refresh-dependencies`. It depends only on `implementation("io.github.yet300:tor:0.3.0")`, with transitive coroutine dependencies, standard optimized Android ProGuard configuration, no project dependency, no native override and no application keep-rule file. Release minification is enabled; local debug-key signing does not make the release APK debuggable.

Every requested arrow is evidenced:

| Boundary | Evidence |
|---|---|
| Source consumer rule → Android AAR | `tor/consumer-rules.pro` and extracted published AAR `proguard.txt` in `published-consumer-rules.pro` contain the exact new rule |
| Local Maven coordinate → Android AAR | Root module's Android variant resolves the locally published `tor-android:0.3.0`; fresh consumer resolved AAR inventory feeds the verifier |
| Published rule → merged R8 configuration | `merged-rules-excerpt.txt`; full `configuration.txt` retained in temporary build with SHA in `artifacts.json` |
| R8 processing → final/installed DEX | `metadata-postfix.json`, `metadata-installed.json`: PASS, 17 surviving / 18 original annotated structs |
| Built APK → installed APK | Both SHA-256 `fd77d95d7116a9d665e94a1cd57f8485fda22576bd523cd011182d16f9c524d6` |
| Published native library → installed native library | AAR and APK raw hashes differ because Android strips the native binary; NDK `llvm-strip --strip-unneeded` on the published arm64 library reproduces installed bytes exactly (`native-strip-provenance.json`) |

The installed RustBufferStruct retains JNA superclass, runtime FieldOrder `capacity,len,data` and named fields `capacity,data,len`. UniffiRustCallStatusStruct retains JNA superclass, runtime FieldOrder `code,errorBuf` and those named fields. Both callback vtable structures and all other surviving annotated runtime structures also pass. The original and mutation APKs fail the same generic check for 15 runtime structs, including these two construction structs.

The verifier reads annotations in original resolved AAR classfiles to discover the inventory, parses all APK DEX files with SDK dexdump and maps original names through R8 mapping. It checks actual runtime annotation visibility/order and reflected fields, rather than merely looking for keep-rule text. `:app:verifyJnaMetadata` passes locally against the fresh fixed consumer. CI now invokes that same task after local release publication; remote CI was not run.

## Rule-removal mutation

A disposable local Maven repository copied the artifact and removed **only** the added class keep rule from its AAR `proguard.txt`. Authoritative checkout and real Maven-local artifact were not mutated. The same consumer source was built fresh against that repository:

1. Metadata task failed: 15 runtime FieldOrder annotations missing, including RustBufferStruct and UniffiRustCallStatusStruct.
2. Installed mutation APK failed public construction with the same JNA field-order error (`minified-mutation.txt`).
3. Fixed APK restored; installed DEX verified again; restored runtime completed (`minified-restored-rule.txt`, `GATE_COMPLETED,PASS`).

This is a deterministic rule mutation, not an external Tor diagnostic retry. No native implementation bytes were changed in the mutation repository.

## Resume contract and cleartext fixture

The frozen contract does **not** explicitly promise that all successful additional-session callback publications finish before the public `resume()` suspend call returns. API freeze §§5–6 makes root readiness own engine RUNNING and `isReady`, separates session usability, permits PAUSED/null after an individual rebind failure, and requires apps to observe dynamic session endpoints. It explicitly states synchronous-before-return behavior for **pause**. The accepted amendments and Phase-1 acceptance do not add an all-session resume barrier. The existing iOS live acceptance already waits for session StateFlow activation.

Therefore production lifecycle behavior was not changed. Android acceptance now verifies root RUNNING/ready and awaits `b.status.first { ACTIVE && endpoint != null }` under a 30-second bound before using the current endpoint. No arbitrary sleep or endpoint-equality assumption was introduced for readiness. The diagnostic probe passes and retains the original five-cycle ordering evidence.

The Android instrumentation test-only cleartext setting remains because its HTTP fixtures include public onion HTTP. The manifest explains this rationale. No production manifest/network policy or consumer cleartext requirement was added. The standalone minified fixture implements HTTP over a raw loopback SOCKS socket and requires no Android cleartext-policy opt-in.

## Minified physical-device runtime

The **first post-fix runtime result** is preserved as `minified-postfix-first.txt`, ending `GATE_COMPLETED,PASS`. Android recreated the foreground activity after APK replacement using its prior `run=first` intent; a subsequent launch delivered an intent to that existing activity. The first process later continued after it was brought to the foreground and completed. It spent an interval in the background while instrumentation ran; no reliable onion latency conclusion is drawn from that interval, and no Tor failure was recorded for it. The separately restored-rule full flow also passes. Neither replaces the first result.

| Check | First fixed minified result |
|---|---|
| Public KMP → JNA → UniFFI → Rust construction/version | PASS; `arti-kmp-ffi 0.3.0 (arti-client 0.46, rustls)` |
| JNA load / RustBuffer / RustCallStatus / callback vtables | PASS through real native construction, lifecycle, errors and callbacks; no FieldOrder or native-load exception |
| Cold bootstrap/root SOCKS HTTP | PASS, bootstrap 100; initial HTTP 200 |
| A/B traffic and isolation | PASS; distinct root/A/B ports; HTTP 200 through A and B |
| Close A | CLOSED/null; old port refuses connection; B and root traffic still succeed |
| Pause/resume | PAUSED/null endpoints, old listeners refuse; retained client; root ready and bounded B StateFlow ACTIVE; B traffic succeeds, no cold bootstrap on resume |
| Ordinary onion | Positive public Tor onion HTTP succeeds |
| Policy/malformed target | Expected SOCKS code 5 with native categorical `ForbiddenStreamTarget` / `InvalidStreamTarget`; clearnet remains usable |
| Malformed bridge/config | Config rejection in AUTO/ON/OFF plus ON-empty; running root/B snapshot and traffic preserved |
| Typed error transport | Public Config; 33rd session public Runtime/RUNTIME; real asynchronous Bind/BIND and typed `status.lastError` |
| Native callbacks and public flows | Status STARTING/BOOTSTRAPPING/RUNNING/PAUSED/ERROR/OFF; log callback; session ACTIVE/PAUSED/CLOSED/INVALIDATED; native error callback upgrades typed Bind; public StateFlow assertions pass |
| Shutdown/cold restart | No client/live endpoints; old handles remain INVALIDATED/CLOSED; old ports refuse; fresh root and session traffic succeed |
| Safe occupied-port error | Real root bind collision produces Bind/BIND, ERROR with typed lastError; session PAUSED/null then INVALIDATED on shutdown |
| Resources/cap | Samples below; 33rd rejected, extra listeners close, shutdown endpoint probes pass |

Per-target requests enter through SOCKS. The public facade exposes no per-target connect call that could throw a public `ArtiException` to this consumer. Therefore the `*_TargetRejected` fixture labels prove SOCKS rejection plus native categorical log transport; they do **not** prove a thrown public `TARGET_REJECTED` exception. Existing category-mapping tests pass. No API was added to manufacture that result.

Synthetic malformed-bridge IP, fingerprint and transport markers are absent from captured public exceptions and all collected public logs; in-app assertions pass in both fixed flows. App-tag filtered Logcat reads after the run returned no entries, including an all-buffer read. Their empty result has no marker, but cannot substantiate a nonempty retained Logcat stream for the first process. Public-log evidence is preserved; this Logcat coverage limitation is explicit. No real bridge was used.

The `/proc/self/maps` filename filter returned no `libarti_kmp_ffi` entry; it is not used as loaded-library-path proof. Installed arm64 packaging, native-byte provenance, version and successful Rust operations establish the actual runtime path.

Requests explicitly connect only to loopback SOCKS, send unresolved target domain names, and have no direct traffic fallback. Deliberate malformed/policy SOCKS code 5 results are expected rejection assertions, not intermittent external fetch failures.

## Device resources

KiB measurements are process observations, not memory/performance guarantees. Historical idle samples (FD 123/124/131/139/155 for 0/1/8/16/32, back to 123 after close) remain in the original hardware report.

| Additional sessions / condition | Listeners | FD | Threads | RSS KiB | PSS KiB |
|---|---:|---:|---:|---:|---:|
| 0, loaded root | 1 | 153 | 53 | 138244 | 58362 |
| 1, used B (after A close) | 2 | 152 | 52 | 139716 | 58536 |
| 8, two used sessions | 9 | 160 | 52 | 139800 | 58458 |
| 1, capacity samples, B used | 2 | 152 | 53 | 138028–138280 | 56829–56893 |
| 8, B used | 9 | 159 | 53 | 139060 | 57521–57585 |
| 16, B used | 17 | 167 | 53 | 139580 | 57805–57869 |
| 32, B used | 33 | 183 | 53 | 140360 | 58621–58693 |
| Close 31, B retained | 2 | 152 | 53 | 141588 | 59717 |
| Before shutdown | 2 | 151 | 51 | 133560 | 61049 |
| After shutdown | 0 | 139 | 43 | 131680 | 54331 |
| Final shutdown after restart/error | 0 | 139 | 46 | 136012 | 56302 |

Capacity rows use three samples each. 33rd creation rejects with public Runtime/RUNTIME. All 31 removed listeners refuse connections; B remains usable. FD returns from 183 to 152 after those closes, and decreases 151→139 after native shutdown. Initial process baseline was FD 121/threads 32/RSS 92652/PSS 16268: exact whole-process baseline recovery is **not** claimed because JVM/runtime pages and threads remain. Cold start 20806 ms, pause interval 1024 ms, resume 11 ms, restart 8025 ms; these are single-run observations.

## Regression outcomes and remaining release decision

| Command / suite | First result | Allowed unchanged diagnostic |
|---|---|---|
| `cargo fmt --manifest-path rust/arti-kmp-ffi/Cargo.toml --check` | PASS | None |
| `cargo test --manifest-path rust/arti-kmp-ffi/Cargo.toml` | Sandbox first: 52 pass/45 fail with bind PermissionDenied; host-permitted run: 97 pass/0 fail/0 ignored | Host permission fixes execution restriction; not product/network retry |
| Same cargo test `-- --test-threads=1` | PASS, 97/0/0 | None |
| `:tor:buildBindings :tor:compileKotlinIosArm64 :tor:assembleAndroidDeviceTest` with release flag | PASS | None |
| `:tor:iosSimulatorArm64Test` with release flag | 103 tests, 102 pass/1 fail; liveTwoSessionsLifecycle `recv failed (eof)` | Failing test only, unchanged: PASS, 1 test; first EOF preserved |
| `:tor:connectedAndroidDeviceTest` with release flag | 3 tests, 1 pass/2 fail, 115.462 s XML time | 3 tests, 2 pass/1 fail, 186.242 s XML time; suite remains failed |
| Fresh fixed consumer `:app:verifyJnaMetadata` | PASS, 17/18 | Installed fixed DEX also PASS |
| Mutation consumer same verifier | Expected FAIL, 15 runtime annotations lost | Restored fixed full runtime PASS |

Combined Gradle regression command:

```sh
rtk ./gradlew :tor:buildBindings :tor:compileKotlinIosArm64 :tor:assembleAndroidDeviceTest :tor:iosSimulatorArm64Test -PreleaseBuild=true --no-configuration-cache --console=plain
```

First Android failures: `bootstrapFetchPauseResume` SSLHandshakeException `connection closed` at its post-resume HTTPS request (TorE2ETest.kt:84); integrated test `SOCKS: Connection refused` at session A request (line 141). Unchanged diagnostic: the integrated test and resume/error probe passed, but `bootstrapFetchPauseResume` failed its initial `assertTorExit` (line 65), with `SocketTimeoutException: Connect timed out` reading the SOCKS reply to `check.torproject.org:443`. Full XML and raw temporary UTP outputs preserve both attempts. There was no second diagnostic retry and no Tor behavior change to make requests green. No attribution to external network versus Tor implementation is established.

**IMPLEMENTATION BLOCKERS:** No remaining deterministic R8/JNA/UniFFI/lifecycle/isolation/callback defect reproduced after the narrow fix. Cause of repeated legacy HTTPS-request failures remains unknown.

**RELEASE BLOCKERS:** Mandatory complete Android instrumentation suite is still failing after its single unchanged retry. This does not satisfy the user's PASS WITH RELIABILITY FOLLOW-UP condition. Nonempty first-process filtered Logcat evidence and a thrown public per-target TARGET_REJECTED exception are not claimed; the latter is outside the current public SOCKS API boundary.

**NON-BLOCKING FOLLOW-UPS:** Investigate preserved iOS EOF that passed unchanged retry; consider an upstream Ubique runtime 1.2.1 FieldOrder/full-mode consumer-rule report (none sent, no upstream acceptance dependency); retain process-resource observations without a leak/performance guarantee. Before any release decision, resolve remaining Android request reliability and explicitly close Logcat evidence coverage.

Evidence bundle: `docs/audit/evidence/0.3/android-r8-remediation-2026-10-01/`. Full APK, mapping/configuration/seeds/usage and raw UTP logs remain in `/tmp/artitor-android-remediation/`; hashes identify them. Old hardware evidence remains under `android-hardware-2026-10-01/`. No tag, push, Maven Central publication or remote release occurred.

Final whitespace check passes for production, test infrastructure and report sources. A whole-change `git diff --check` reports five extra blank lines at EOF in preserved raw failure/rule evidence files; their captured contents were retained rather than edited to make historical evidence look cleaner. Only the three original unrelated untracked files remain outside these commits.

**ARTITOR 0.3 ANDROID HARDWARE GATE: REMEDIATION REQUIRED**
