# ArtiTor 0.3 iOS 15 support-policy remediation

Date: 2026-10-01  
Baseline: `3f5365a5a2a1b74710c6e10342ff034697f31669`  
Implementation commit: `42c00fab569fb9703b59b222e518809863b58854`

## Decision

ArtiTor 0.3 minimum supported iOS version is **15.0**. Final consumer-facing
Apple artifacts are built with iOS 15 deployment metadata; 0.3 aligns the
declared support policy with the produced artifacts. iOS 13 and iOS 14 are not
supported.

This changes the supported Apple OS baseline and build/documentation/release
verification metadata only. There is no Kotlin API change, UniFFI API change,
Cargo feature change, or runtime behavior change. Lifecycle, sessions, bridge
logic, error mapping, onion behavior, and timeout semantics were not modified.

## Deployment target declarations

| Location | Before | After / status |
|---|---|---|
| `rust/arti-kmp-ffi/.cargo/config.toml` | `IPHONEOS_DEPLOYMENT_TARGET=13.0` | `15.0`, with Cargo `force=true` so task environment cannot override it |
| `rust/hide-sqlite3-symbols.sh` | device and simulator prelink already used 15.0 | unchanged; device is `ios 15.0 15.0`, simulator is `ios-simulator 15.0 15.0` |
| `tor/build.gradle.kts` | Apple targets and post-Cargo SQLite hook; no separate minimum declaration | unchanged; its existing hook still invokes the localization script |
| Kotlin Multiplatform target declarations | `iosArm64()` and `iosSimulatorArm64()` | production targets unchanged; consumer fixture now emits device and simulator frameworks for link verification |
| KMP publication metadata | target-specific KLIB variants; no separate deployment-target field | unchanged; native archives inside the published KLIBs carry `LC_BUILD_VERSION` metadata |
| `README.md` | no numerical iOS minimum; Android alignment statement covered 32-bit libraries | explicitly says minimum iOS 15.0; limits 16 KB alignment wording to bundled 64-bit Android JNI libraries |
| `docs/design/ARTITOR_0_3_API_FREEZE.md` | prior freeze assumed iOS 13 | appended a release-policy amendment; historical review evidence was not rewritten |
| `docs/audit/evidence/0.3/consumer/` | simulator framework only | separate device and simulator consumer frameworks, both linked from published coordinates |

Cargo's `.cargo/config.toml` is the authoritative Apple Cargo minimum. The
SQLite localization script repeats 15.0 because `ld -platform_version` needs a
platform-specific linker argument; its active values match Cargo and are
verified below. Repository-wide search found no active iOS 13 or iOS 14 build
or support declaration. Remaining references are historical audit evidence,
the explicit statement that iOS 13/14 are unsupported, and unrelated version
strings such as dependency `1.13.0` / lockfile `0.13.0`.

## Artifact and consumer evidence

All Mach-O platform IDs matched their targets. The pristine Cargo device archive
had 436 members with `minos=15.0`. The pristine simulator archive had 436
members at 15.0 and 393 Rust toolchain/runtime members at 14.0; none required
more than 15.0. The normal SQLite prelink consolidates each archive into one
`merged.o`, explicitly linked at 15.0. Both localized archives, both published
KLIB-included archives, and both fresh consumer frameworks report exactly
15.0. Thus the shipped minimum is 15.0 on device and simulator; the lower
minimum on some simulator intermediate members does not raise the consumer
minimum.

Fresh consumer project: `docs/audit/evidence/0.3/consumer`, with only
`io.github.yet300:tor:0.3.0` and `mavenLocal()` for this group; no project-source
dependency or consumer workaround.

| Check | Result |
|---|---|
| Cargo device archive platform / member `minos` | iOS / all inspected members 15.0 |
| Cargo simulator archive platform / member `minos` | iOS Simulator / 14.0 and 15.0 members; none above 15.0 |
| Localized/prelinked device archive | iOS / `merged.o` minos 15.0 |
| Localized/prelinked simulator archive | iOS Simulator / `merged.o` minos 15.0 |
| Published device KLIB archive | iOS / minos 15.0 |
| Published simulator KLIB archive | iOS Simulator / minos 15.0 |
| Fresh linked device consumer framework | iOS / minos 15.0 |
| Fresh linked simulator consumer framework | iOS Simulator / minos 15.0 |
| Strict published device archive link at iOS 15.0 | PASS |
| Strict published simulator archive link at iOS 15.0 | PASS |
| Strict published device archive link at iOS 14.0 | Expected failure: `-fatal_warnings` on library minos 15.0 |
| Strict published simulator archive link at iOS 14.0 | Expected failure: `-fatal_warnings` on library minos 15.0 |

## SQLite isolation

Counts are defined global symbols matching `_sqlite3*`:

| Artifact | Count |
|---|---:|
| Pristine Cargo device archive | 283 |
| Pristine Cargo simulator archive | 283 |
| Post-hook device archive | 0 |
| Post-hook simulator archive | 0 |
| Published device KLIB archive | 0 |
| Published simulator KLIB archive | 0 |
| Fresh linked device consumer framework | 0 |
| Fresh linked simulator consumer framework | 0 |

The existing `rust/hide-sqlite3-symbols.sh` and Gradle hook remain in place.

## Regression and packaging results

| Check | Result |
|---|---|
| `cargo fmt --check` | PASS |
| `cargo test` | PASS, 97 tests |
| `cargo test -- --test-threads=1` | PASS, 97 tests |
| Gradle release build, device compile, Android device-test assembly, simulator suite | PASS on a clean rerun, 103/103 simulator tests |
| Final simulator suite after forced Cargo environment | 102/103 passed; `bootstrapFetchPauseResume` failed once with live SOCKS `RemoteStreamError`; focused retry passed 1/1. The other live two-session/onion test passed. |
| `:tor:publishToMavenLocal -PreleaseBuild=true --no-configuration-cache` | PASS after final Cargo configuration |
| Fresh consumer device/simulator compile and framework link | PASS |
| Android AAR ABI set | Unchanged: `arm64-v8a`, `armeabi-v7a`, `x86_64` |
| Android 64-bit Arti LOAD alignment | Unchanged: all four LOAD segments in both `arm64-v8a` and `x86_64` are 16,384-byte aligned and offset/address congruent |
| Cargo features | Unchanged; `Cargo.toml` was not modified |
| Android hardware (`adb devices -l`) | No attached devices; hardware gate NOT VERIFIED |

The simulator's intermittent live fetch error is not attributed to this
deployment-target change and is not claimed fixed. A passing focused retry and
the earlier clean 103/103 run are retained alongside the final 102/103 result.

## Before / after evidence

| Check | Before | After |
|---|---|---|
| Declared minimum iOS | 13 | 15.0 |
| Device artifact minos | 15.0 | 15.0 |
| Simulator artifact minos | 15.0 | 15.0 |
| iOS 15 strict device link | PASS control | PASS |
| iOS 15 simulator link | PASS | PASS |
| iOS 14 support | Ambiguous / indirectly implied | NOT SUPPORTED; strict device and simulator links fail as expected |
| Post-hook SQLite globals | 0 | 0 |
| Published SQLite globals | 0 | 0 |

R01 is **RESOLVED** by aligning the declared baseline with the final artifacts.
R02 is **RESOLVED** by narrowing the README alignment statement to 64-bit
Android libraries.

## Release status

Implementation blockers: **NONE**.  
Release/environment blockers: Android hardware acceptance and minified runtime
smoke are **NOT VERIFIED** because no Android device was attached.  
Non-blocking follow-up: investigate the intermittent live SOCKS fetch
`RemoteStreamError` without assuming a cause.

No remote publication or tag was created. No 0.4 work was started.
