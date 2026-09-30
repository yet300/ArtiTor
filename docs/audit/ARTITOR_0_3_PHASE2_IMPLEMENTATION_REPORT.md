# ArtiTor 0.3 Phase 2 — Bridge Configuration Hardening

Status: **IMPLEMENTED — READY FOR INDEPENDENT AUDIT**. Phase 2 is not CLOSED.
Confidence: **high** for the frozen zero-network bridge contract and verified regressions.
Verification date: **2026-09-30**, Asia/Tbilisi.

## Immutable baseline and implementation

- Baseline HEAD: `dd664744142285631408485c5b7104b683d2bc58`.
- Implementation SHA: `8bb0d95a4a865b1d322086e2b2ec37c26fb22c57` (`feat(config): harden bridge configuration`).
- `git rev-parse HEAD`, `git status --short`, and `git log --oneline -12`
  were run before editing. HEAD matched the requested baseline; the tree was clean.
- The preceding commits were `54eeeca` (Rust modularization), `ad83254`
  (capability audit), and `5c506b9` (Phase-1 closure).
- Primary authority: `docs/design/ARTITOR_0_3_API_FREEZE.md` §§3, 9–11.
  The API freeze supersedes conflicting capability-audit prose.
- The implementation remains modular: public data in `lib.rs`, parsing/configuration
  in `config.rs`, lifecycle consumption in existing engine modules. No session
  publication or ownership architecture was changed.

Baseline SHA-256 values, captured before implementation:

| File | SHA-256 |
|---|---|
| `rust/arti-kmp-ffi/Cargo.toml` | `af10230fff1048d59e65c4b6c2d66652394b17c4e71a2ac55ac92d4a971f5942` |
| `rust/arti-kmp-ffi/Cargo.lock` | `6670be49a0e87563287249f94b113acb523168bc4d7c2d40e35837da3ca5eab5` |
| `rust/arti-kmp-ffi/src/config.rs` | `c224271bf9928dc1a10ff7ce8f4444122a1196c06ea19664b293c6d04278cc14` |
| `rust/arti-kmp-ffi/src/ffi.rs` | `b288def174b3ab8c248acf2004cddb9a850e996901bf1b675be5a215d0bccf1b` |
| `rust/arti-kmp-ffi/src/engine/lifecycle.rs` | `02761f3eae255d7b34c9e9c4322bdd0a0b00ad2512de029bfa5bfe45d388cd5d` |
| `tor/src/commonMain/kotlin/com/yet/tor/ArtiTorClient.kt` | `394c401a1c539f86424f928f9ad8e214d4a5987f774d4de6b1ffe821d73f92a9` |
| `tor/build.gradle.kts` | `b9741b8aaf45e8adea758f9e34d189059305d2b2552cd087b093ebcc03d90afc` |
| `rust/hide-sqlite3-symbols.sh` | `eeb578bf6c323a2aaa310fa8fb694bc1e9a7a3ca21092045a3588d690e1e0604` |

## Pinned upstream API inspection

The repository's local Cargo registry sources, under
`/Users/yet/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/`, were inspected
before production edits. The actual pinned Arti crates are **0.46.0**.

| Source | Verified API/behavior |
|---|---|
| `arti-client-0.46.0/src/config.rs:22–31` | Reexports `BoolOrAuto`, `BridgeConfigBuilder`, `BridgeParseError`, and `ConfigBuildError`. |
| `arti-client-0.46.0/src/config.rs:350–384` | `BridgesConfig` and generated `BridgesConfigBuilder`; enabled policy plus bridge list. |
| `arti-client-0.46.0/src/config.rs:442–483` | Upstream pre-build validation rejects `Explicit(true)` with no bridges; AUTO uses the list's emptiness. |
| `arti-client-0.46.0/src/config.rs:501–527` | List-builder accessors: `builder.bridges().bridges()` accepts `BridgeConfigBuilder` values. |
| `arti-client-0.46.0/src/config.rs:715–723` | Final `TorClientConfig` implements `AsRef<BridgesConfig>` and upstream guard enablement semantics. |
| `tor-guardmgr-0.46.0/src/bridge/config.rs:49–75` | Direct syntax: optional `Bridge`, numeric IPv4/bracketed IPv6 endpoint, mandatory RSA identity, optional ed25519 identity. |
| `tor-guardmgr-0.46.0/src/bridge/config.rs:220–372` | `BridgeConfigBuilder::build() -> Result<BridgeConfig, ConfigBuildError>`. |
| `tor-guardmgr-0.46.0/src/bridge/config.rs:377–419` | `FromStr<BridgeConfigBuilder>` returns `BridgeParseError`; it parses through upstream `Inner`. |
| `tor-guardmgr-0.46.0/src/bridge/config.rs:446–475` | Upstream trims/splits whitespace; blank entries are `BridgeParseError::Empty`; without PT support, PT names produce `PluggableTransportsNotSupported`. |
| `tor-guardmgr-0.46.0/src/bridge/config/err.rs:117–128` | Unsupported-PT parser category and secret-bearing offending-word payload. |
| `tor-guardmgr-0.46.0/src/bridge/config.rs:742–762` | Upstream direct IPv4/IPv6 test syntax used as fixture authority. |
| `tor-config-0.46.0/src/misc.rs:19–24` | Exact variants: `BoolOrAuto::Auto` and `BoolOrAuto::Explicit(bool)`. |

The PT-string parser's actual error is **`PluggableTransportsNotSupported`**,
rather than `ConfigBuildError::NoCompileTimeSupport`. The builder also has a
`NoCompileTimeSupport` branch for unsupported channel methods. This is the frozen
unsupported-at-compile-time semantic category; both remain internal and the public
result is existing typed `Config`. No API discrepancy required a new design or feature.

## Public API and construction flow

The only intended public KMP API additions are:

```kotlin
enum class BridgesEnabled { AUTO, ON, OFF }
// ArtiConfig, in the frozen constructor position after bridges:
val bridgesEnabled: BridgesEnabled = BridgesEnabled.AUTO
```

Packages remain `com.yet.tor` and generated `com.yet.tor.ffi`. The Rust mirror is
`BridgesEnabled::{Auto, On, Off}` plus `ArtiConfig.bridges_enabled`. No upstream
bridge/parser types cross UniFFI. The generated record intentionally requires the
new field; the public facade supplies it using the default and an exhaustive mapping.
All other exported objects, functions, records, enums and error subclasses retain
their API. Adding a data-class field is the planned Phase-2 API evolution; this is
not a claim of byte-identical ABI compatibility.

`config::build_tor_config()` is the sole bridge configuration helper:

1. Parse every supplied entry with `BridgeConfigBuilder::from_str` and explicitly
   call `build()`. Stop with typed Config on any invalid entry. No caller-side
   filtering, custom syntax, trimming or normalization is introduced.
2. Resolve existing directory defaults and construct `TorClientConfigBuilder`.
3. Set the exact policy; copy validated builders into the applied bridge list
   only when policy is not OFF.
4. Build the whole upstream config; propagate its ON+empty check as typed Config.

`ffi_start()` calls this helper **before** taking engine locks, installing the
listener, initializing tracing, creating a runtime, invalidating sessions, spawning
workers, or constructing TorClient. The existing worker receives the already-built
config. Invalid input therefore returns synchronously with no new worker/runtime/client.
Filesystem creation remains in the existing cold worker after validation.

| Public policy | Exact upstream mapping | Applied result |
|---|---|---|
| AUTO | `BoolOrAuto::Auto` | Empty list: normal guards; non-empty valid list: bridges. |
| ON | `BoolOrAuto::Explicit(true)` | Valid non-empty list: bridges; empty: Config before worker creation. |
| OFF | `BoolOrAuto::Explicit(false)` | Every line validated, then no bridge entries applied; normal guards. |

Native and Kotlin identity comparisons include both the exact original bridge list
and policy. Order, whitespace, different textual representations, clearing a list,
and OFF+A→OFF+B remain changes. `socksPort` is still listener-only. Replacement uses
the existing Phase-1 invalidation/client-discard/cold-start path and advances client
epoch; no `TorClient::reconfigure()` call was added.

## Bridge secrecy and typed errors

The existing bootstrap code formatted the upstream parser error as
`bad bridge line: {e}` and forwarded full config-build error text. Upstream parser
variants embed the offending word, so this was a bridge-secrecy blocker.
Those error paths were removed from bootstrap.

- Parse/build errors now return `ArtiError::Config` with
  `invalid bridge configuration`.
- Final client-config construction errors, including ON+empty, return
  `ArtiError::Config` with `invalid client configuration`.
- No upstream bridge Display/Debug output is included in those diagnostics.
- Existing synchronous Kotlin error conversion and native callback detail
  conversion retain the public `ArtiException.Config` classification.
- All local production Rust logging paths were searched for config/bridge dumps.
  No existing generic config Debug dump was found. The remaining cold-start log
  contains directory and local SOCKS-port information, not bridge material.
- The pinned parser/config builder performs no tracing of input. The inspected
  upstream bridge descriptor cache error path uses `safelog::sensitive(bridge)`
  (`tor-dirmgr-0.46.0/src/bridgedesc.rs:1018–1022`). This work does not claim a
  complete independent audit of every upstream runtime logging path.
- README/KDoc explicitly say bridge configuration is secret and must not be logged
  or persisted in unprotected app preferences.

Adversarial tests inspect native Config Display/Debug, safe internal error
Display/Debug, callback-detail messages, native listener logs, and actual public
facade errors/logs. Synthetic address, port, RSA, fingerprint-marker and PT-option
markers must be absent. Invalid entries never reach worker logging.

## Changed files

Implementation commit:

- `README.md`
- `rust/arti-kmp-ffi/examples/host_poc.rs`
- `rust/arti-kmp-ffi/src/config.rs`
- `rust/arti-kmp-ffi/src/engine/bootstrap.rs`
- `rust/arti-kmp-ffi/src/engine/lifecycle.rs`
- `rust/arti-kmp-ffi/src/engine/mod.rs`
- `rust/arti-kmp-ffi/src/lib.rs`
- `rust/arti-kmp-ffi/src/tests/config.rs`
- `rust/arti-kmp-ffi/src/tests/support.rs`
- `tor/src/commonMain/kotlin/com/yet/tor/ArtiTorClient.kt`
- `tor/src/commonTest/kotlin/com/yet/tor/ArtiTorClientLifecycleTest.kt`
- `tor/src/commonTest/kotlin/com/yet/tor/ConfigIdentityTest.kt`
- `tor/src/iosTest/kotlin/com/yet/tor/BridgeConfigurationTest.kt`

Report-only follow-up:

- `docs/audit/ARTITOR_0_3_PHASE2_IMPLEMENTATION_REPORT.md` (this document).

`ffi.rs`, Cargo manifests/lockfile, Gradle build hooks, SQLite script, session
implementation files and existing live E2E test files remain unchanged.

## Binding and public API comparison

A baseline `:tor:buildBindings` was generated before production edits and copied to
`/tmp/artitor-phase2-evidence/bindings-before/`. Final bindings were regenerated.
The full diff is `/tmp/artitor-phase2-evidence/bindings.diff`, SHA-256
`6e10a1d570ab38f2bde5cddb7c88402b2b65a172dac1dfee08c16f174b9f33f5`.
These are local audit artifacts, not checked-in generated source.

| Generated file (under `tor/build/uniffi/bindings/`) | Diff |
|---|---|
| `commonMain/kotlin/com/yet/tor/ffi/arti_kmp_ffi.common.kt` | +22/-0: record field and three-value enum with documentation/spacing. |
| `androidMain/kotlin/com/yet/tor/ffi/arti_kmp_ffi.android.kt` | +23/-0: field read/size/write plus enum converter. |
| `jvmMain/kotlin/com/yet/tor/ffi/arti_kmp_ffi.jvm.kt` | +23/-0: field read/size/write plus enum converter. |
| `nativeMain/kotlin/com/yet/tor/ffi/arti_kmp_ffi.native.kt` | +23/-0: field read/size/write plus enum converter. |

No unrelated generated signature or function-checksum changes appeared. The binary
record wire layout intentionally changes. No mechanism to dump/check public Kotlin
ABI is configured in the checked build files, so the public source diff and complete
generated diff were reviewed directly; no fictitious Gradle ABI task was used.
Defaults are tested for named `ArtiConfig(dataDir = ...)` callers, both with and
without bridge lists. Constructor field placement follows the freeze.

## Dependency, feature and Apple invariants

`Cargo.toml`, `Cargo.lock`, `tor/build.gradle.kts` and
`rust/hide-sqlite3-symbols.sh` retain exactly their baseline hashes above.
`cargo tree --manifest-path rust/arti-kmp-ffi/Cargo.toml -e features` was captured
before and after and compared byte-for-byte: **identical**. Both snapshots have
SHA-256 `3e5e1e3bdbdbd5c927055671ec5b61ffef30284b4be5d18601eac3d3076ac357`.
No dependencies or Cargo features were added or changed. `bridge-client` remains
on; `pt-client` and all forbidden new features remain unenabled.

The iOS simulator and iOS device builds executed the existing build hook and printed
`hide-sqlite3-symbols: localized bundled sqlite3` for their static archives. Neither
the script nor its invocation was edited.

## Zero-network test contract

Native config module: **15 tests passed** (9 existing, 6 added).
The additional contract includes:

- Seven valid policy/list combinations: AUTO empty/one/two, ON one, OFF empty/one/two.
  The final `AsRef<BridgesConfig>` value is compared to independently constructed
  upstream expected configs, including the exact enabled value and applied list.
  Upstream 0.46 exposes no public getters on this config type; equality covers its
  policy and list fields. OFF must have zero applied entries.
- Thirty-four invalid matrix rows across all policies: ON+empty; garbage;
  `""`, `" "`, `"\t"`, `"\n"`; secret-bearing malformed direct and PT-shaped lines;
  malformed first/middle/last and valid-prefix lists. Every row proves Config,
  no runtime, no worker, no client, and unchanged worker revision.
- Direct fixtures use documentation IPv4/IPv6 addresses and synthetic repeated-digit
  RSA identities. No real bridge is committed and no bridge connection is attempted.
- PT-shaped input must produce the pinned unsupported-PT parser category. Enabling
  PT would invalidate this negative test.
- Exact identity for unchanged policy/list, every cross-policy transition,
  changed list, changed order and changed whitespace, including OFF.
- Real offline engine/session fixtures exercise AUTO→ON, ON→OFF, OFF+A→OFF+B,
  and AUTO+A→AUTO+B. The existing cold-spawn failure seam stops at entry to the
  replacement path, while asserting old session INVALIDATED/null, invalidation
  publication, old client discarded, bootstrap reset and client_epoch advanced.
  Only local loopback listeners are used; no Tor bootstrap network is needed.

Kotlin common additions test the AUTO default, exact identities in every policy,
FFI mapping of all three values, unchanged-config no-op, and existing teardown
selection for bridge/policy changes. Existing error mapping and Phase-1 tests remain
in the corpus. Two real facade/UniFFI iOS tests add 18 malformed-input cases plus
ON+empty, proving public Config/secrecy without bootstrap.

## Verification results and attempts

| Command/run | Result |
|---|---|
| `cargo fmt --check --manifest-path rust/arti-kmp-ffi/Cargo.toml` | PASS. |
| `cargo test --manifest-path rust/arti-kmp-ffi/Cargo.toml` | PASS: 81 unit tests; 0 doc tests. Final verification after mutation work also passed 81/81. |
| `cargo test --manifest-path rust/arti-kmp-ffi/Cargo.toml -- --test-threads=1` | PASS: 81/81, including final verification. |
| `cargo tree --manifest-path rust/arti-kmp-ffi/Cargo.toml -e features` | PASS: no before/after difference. |
| `./gradlew :tor:buildBindings` | PASS at baseline and after implementation. |
| `./gradlew :tor:iosSimulatorArm64Test :tor:compileKotlinIosArm64 :tor:assembleAndroidDeviceTest` | PASS, 2m23s. Simulator: 79/79, 0 failures/skips, including two live regressions. iOS device compilation and Android test APK assembly passed. |
| `./gradlew :tor:iosSimulatorArm64Test --tests com.yet.tor.BridgeConfigurationTest` | PASS, 30s; 2/2 new real-FFI zero-network tests. |
| `git diff --check` | PASS before implementation commit and report completion. |

**Simulator total: 81 distinct tests passed across the full run (79) and subsequent
focused real-FFI run (2).** This is not presented as one full 81-test execution.
The first simulator XML evidence was saved before the filtered run replaced Gradle's
result directory, in `/tmp/artitor-phase2-evidence/simulator-first/`.

Preliminary failures are retained, not hidden:

1. The new pre-worker regression was run twice against the baseline and failed as
   intended: malformed bridge input returned after initiating a worker rather than
   a synchronous Config. `/tmp/artitor-phase2-evidence/red-test.txt` records the
   second run. The process-global panic hook suppressed the assertion text in that
   test's output, but the failed result and source assertion are preserved.
2. Initial matrix test compilation failed with missing enum/field/helper errors
   before implementation. A later test-authoring compile attempt used nonexistent
   `BridgesConfig` getters (four errors); inspection corrected the test to use its
   public `AsRef` and equality API.
3. The first native config-suite execution after implementation passed 14 tests
   and failed the replacement fixture's loopback bind with `Operation not permitted`.
   The approved unsandboxed rerun passed all 15. Native complete suites ran outside
   that loopback restriction; this was not a Tor/network correctness failure.
4. An intermediate Kotlin compile before facade field mapping failed with
   `No value passed for parameter 'bridgesEnabled'`. After mapping/public API were
   implemented, test compilation and all required platform commands passed.
5. ADB could not create its localhost server inside the sandbox. The approved
   rerun returned an empty attached-device list.

Existing build warnings remain: Android host tests not configured, Gradle 10
compatibility deprecations, and the existing iOS `CStructVar.Type` deprecation.
No warning was suppressed or unrelated build cleanup introduced.

## Mutation sensitivity

Mutations ran sequentially in a disposable crate copy at
`/tmp/artitor-phase2-evidence/mutation-crate/`; production sources/commits were
never mutated. Cargo dependencies/features remained identical. Each run compiled
and failed its contract test with exit 101. The disposable source was restored;
normal final native suites subsequently passed.

| Mutation | Detecting test | Result |
|---|---|---|
| A: skip parsing/validation under OFF | `bridge_invalid_matrix_is_secret_safe_and_starts_no_worker` | KILLED: invalid OFF returned success. |
| B: remove enablement from native config identity | `bridge_identity_is_exact_in_all_modes` | KILLED: cross-policy comparison failed. |
| C: map ON to AUTO | `bridge_invalid_matrix_is_secret_safe_and_starts_no_worker` | KILLED: ON+empty returned success. |
| D: echo original line in parser Config diagnostic | `bridge_invalid_matrix_is_secret_safe_and_starts_no_worker` | KILLED: `bridge material escaped`. |

Raw output files: `/tmp/artitor-phase2-evidence/mutation-{A,B,C,D}.txt`.
The earlier A/C mutation failure diagnostics dumped an upstream config via test
`unwrap_err()`; the final test uses `.err().expect(...)` to avoid that unnecessary
failure-output dump. Mutation D only exposed synthetic fixture markers during the
negative experiment. No real bridge material was used.

## Phase-1 regressions and live attempts

The entire native accepted corpus plus additions passed parallel and serial. The
simulator common tests passed: ConfigIdentity 10, LifecycleInvariant 6, ErrorMapping
6, ArtiTorSessionConcurrency 30, ArtiTorClientLifecycle 25 (77 common tests).
This preserves F01 typed facade errors, publication revisions/order, transition
and client-epoch fences, ownership, listener failures, dispatch/framing and terminal
session behavior. No Phase-1 production session machinery changed.

Normal live Tor regression attempts on the simulator, each **once** in the full run:

| Test | Outcome | Test duration |
|---|---|---|
| `liveTwoSessionsLifecycle` | PASS: two sessions, independent close, root survival, pause/resume, restart/stale handles. | 49.197s |
| `bootstrapFetchPauseResume` | PASS: bootstrap, root fetch, pause/resume and cold restart/fetch. | 39.642s |

No live test failed, and no retry-until-green was performed. The later targeted
bridge test run did not repeat live tests. The historical intermittent
`SOCKS CONNECT failed (code=5)` did not appear here; it remains an unresolved
reliability follow-up, not established as bridge-related.

Android: ADB reports **no attached device**. **Android hardware runtime: NOT VERIFIED**.
Android assembly is build evidence, not runtime verification.

## Security and scope review

| Question | Answer |
|---|---|
| Can malformed bridge input reach a new bootstrap worker? | NO. |
| Can ON+empty reach bootstrap? | NO. |
| Can a supplied bridge line appear in the tested Config diagnostic paths? | NO. |
| Can invalid bridge input appear in native listener logs? | NO: validation returns before worker/log setup; native and real-FFI adversarial tests pass. |
| Can OFF bypass validation? | NO; mutation A detects that regression. |
| Can a PT-shaped line silently become supported with the unchanged feature set? | NO; pinned-parser negative tests pass. |
| Can changing bridges avoid the normal TorClient replacement decision? | NO. |
| Can changing enablement avoid replacement? | NO; exact comparison and mutation B verified. |

New Cargo features: **NONE**. Live reconfigure used: **NO**. PT support implemented:
**NO**. TorErrorKind, onion config, timeout config and dormant mode implemented:
**NO**. Session/publication architecture changed: **NO**. **Phase 3+ was not started.**

A focused read-only code review found no actionable production defect, with high
confidence for reviewed bridge code. The reviewer did not independently rerun the
verification corpus or audit all upstream runtime logging. This review is preparatory
and does not replace the requested independent audit.

Remaining follow-ups: independent Phase-2 adversarial audit; Android hardware runtime;
historical intermittent SOCKS code-5 reliability; existing Phase-1/Phase-5 platform
and resource follow-ups. Successful live bridge networking is deliberately not a
Phase-2 acceptance requirement and was not attempted. No Phase-2 correctness blocker
was found in the tested frozen scope.
