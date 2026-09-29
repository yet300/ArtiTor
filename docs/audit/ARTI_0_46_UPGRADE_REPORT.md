# Arti 0.46 Upgrade Report (ArtiTor 0.2 lifecycle baseline)

Date: 2026-09-29
Base branch: `main` (0.2.0 lifecycle already merged)
Scope: strict dependency upgrade only — `arti-client 0.43.x → 0.46.0` and the
corresponding Arti crates required by ArtiTor. No 0.3.0 features, no public API
redesign, no new capabilities.
Normative inputs: `docs/api-lifecycle-bitchat.md`,
`docs/audit/ARTITOR_0_2_LIFECYCLE_CONFORMANCE_REPORT.md` (both unchanged by this task).

## Summary

`cargo update` + `cargo check` on the bumped manifest produced **zero compile
failures**. The entire ArtiTor-used API subset compiles identically on 0.46, all
14 Rust tests and all 45 iOS simulator tests pass unmodified, and the full live
Tor E2E sequence re-verified on the iOS simulator against `arti-client 0.46`.
The diff is intentionally minimal: 2 version lines in `Cargo.toml`, the
regenerated `Cargo.lock`, a 1-line `version()` string, and a CI Rust pin
(`stable` → `1.91`). No implementation logic was touched.

## Version matrix

| Crate / toolchain                                                                                        | Before (0.43 baseline) | After (0.46)                                            |
|----------------------------------------------------------------------------------------------------------|------------------------|---------------------------------------------------------|
| `arti-client`                                                                                            | 0.43.0                 | 0.46.0                                                  |
| `tor-rtcompat`                                                                                           | 0.43.0                 | 0.46.0                                                  |
| Rust minimum (`cargo info arti-client`)                                                                  | 1.89                   | **1.91**                                                |
| Local `rustc --version`                                                                                  | 1.96.0                 | 1.96.0 (≥ 1.91 ✓)                                       |
| CI toolchain (`.github/workflows/ci.yml`, `publish.yml`)                                                 | generic `stable`       | **pinned `1.91`** (`rustup default 1.91`)               |
| ArtiTor own crate edition                                                                                | 2021                   | 2021 (unchanged)                                        |
| All other `tor-*` Arti crates (`tor-proto`, `tor-dirmgr`, `tor-circmgr`, `tor-keymgr`, `tor-persist`, …) | 0.43.0                 | 0.46.0                                                  |
| `tokio`                                                                                                  | 1.52.3                 | 1.53.1                                                  |
| `rustls`                                                                                                 | 0.23.41                | 0.23.45 (still 0.23.x; provider-install path unchanged) |
| `uniffi` (Mozilla Rust)                                                                                  | 0.32.2                 | 0.32.2 (unchanged — no UniFFI migration needed)         |
| Ubique plugin (`ch.ubique.uniffi.plugin`)                                                                | 1.2.1                  | 1.2.1 (unchanged)                                       |
| `libsqlite3-sys`                                                                                         | 0.37.0                 | 0.37.0 (unchanged)                                      |
| `rusqlite`                                                                                               | 0.39.0                 | 0.39.0 (unchanged)                                      |
| ArtiTor library version (`tor/build.gradle.kts`, `Cargo.toml` package)                                   | 0.2.0                  | 0.2.0 (NOT bumped to 0.3.0 per scope)                   |

Important changed transitive Arti crates: every `tor-*` dependency moved in
lockstep 0.43.0 → 0.46.0 via `cargo update` (no hand-edited checksums). No
non-Arti dependency was upgraded except the routine `tokio`/`rustls` patch
bumps pulled in by Cargo resolution.

## Upstream API changes (per API ArtiTor uses)

Researched via docs.rs 0.46 API pages + `cargo info` + empirical
`cargo update && cargo check` (recorded below). Classification per task
(1 mechanical, 2 semantic, 3 config/storage, 4 runtime, 5 feature-flag,
6 unexpected regression):

| API ArtiTor uses                                              | 0.43 behavior/API                                                                                | 0.46 behavior/API                                                                                                                                                          | ArtiTor adaptation | Semantic impact                                                                                                                                                                                           |
|---------------------------------------------------------------|--------------------------------------------------------------------------------------------------|----------------------------------------------------------------------------------------------------------------------------------------------------------------------------|--------------------|-----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `TorClient` / `TorClient::builder`                            | Returns builder; constructors return `Arc<TorClient>` (since 0.43 change)                        | Same; `builder()` still requires a running Tokio runtime context                                                                                                           | None               | None                                                                                                                                                                                                      |
| `TorClientConfigBuilder`                                      | `from_directories(state, cache)` with `AsRef<Path>`; `bridges()`, `build()`                      | Same signatures on docs.rs 0.46 (`from_directories<P: AsRef<Path>, Q: AsRef<Path>>`, `bridges()`, `build()`)                                                               | None               | None                                                                                                                                                                                                      |
| Storage/state/cache config                                    | `from_directories` + optional `state_dir`/`cache_dir` overrides                                  | Same; the `StorageConfigBuilder::cache_dir/state_dir → CfgPath` note from the changelog affects direct storage-builder users, not the `from_directories` path ArtiTor uses | None               | Config-identity invariant (`dataDir`/`stateDir`/`cacheDir` exact compare, `socksPort` excluded) verified unchanged                                                                                        |
| Bridges (`builder.bridges().bridges().push(line.parse()?)`)   | `List<String>` lines parsed; empty = no bridges; invalid → `Config`                              | Same; compiles identically, invalid-line `parse()` error still maps to typed `ArtiError::Config`                                                                           | None               | Bridge contract (empty/non-empty/cleared/invalid) unchanged                                                                                                                                               |
| `create_unbootstrapped` (sync)                                | Exists; used inside owned runtime                                                                | Still exists (async `create_unbootstrapped_async` sibling also present, not used)                                                                                          | None               | None                                                                                                                                                                                                      |
| `bootstrap` / `bootstrap_events` / `BootstrapStatus::as_frac` | `bootstrap_events()` stream + `bootstrap().await`; `as_frac()` 0.0–1.0, explicitly non-monotonic | Same on 0.46 docs; `as_frac`, `ready_for_traffic`, `blocked` unchanged; non-monotonicity still documented                                                                  | None               | Bootstrap semantics preserved: progress capped at 99 until `bootstrap()` returns, RUNNING only after bootstrap + SOCKS bind; cancellation still aborts the single worker task (no detached progress loop) |
| `TorClient::connect`                                          | `connect((host, port))` → `DataStream`                                                           | Same signature on 0.46                                                                                                                                                     | None               | SOCKS CONNECT (IPv4/IPv6/domain) + fail-closed stream termination unchanged                                                                                                                               |
| Runtime selection / `PreferredRuntime`                        | `tor-rtcompat` with `tokio`, `rustls`                                                            | Same; `PreferredRuntime` still the client parameter                                                                                                                        | None               | Owned `new_multi_thread` runtime path unchanged                                                                                                                                                           |
| rustls integration                                            | `rustls 0.23` + explicit `ring` provider install                                                 | `rustls` 0.23.45, still 0.23.x; `rustls::crypto::ring::default_provider().install_default()` still compiles                                                                | None               | Rustls-only invariant holds; no `native-tls` needed                                                                                                                                                       |
| Static SQLite support                                         | `static-sqlite` feature → `libsqlite3-sys` bundled                                               | Same feature name; lockfile still `libsqlite3-sys 0.37.0` + `rusqlite 0.39.0`                                                                                              | None               | Apple symbol-isolation hook still required and still effective (see SQLite audit)                                                                                                                         |
| Lifecycle/resource ownership                                  | `TorClient: Send + Sync`, `Arc`-based sharing                                                    | Same                                                                                                                                                                       | None               | Single-live-engine-per-process + pause-keeps-client / shutdown-drops-everything unchanged                                                                                                                 |
| Onion-service client deps                                     | `onion-service-client` feature                                                                   | Same feature name, still additive                                                                                                                                          | None               | No server/RPC/PT pulled in (see feature audit)                                                                                                                                                            |
| Bridge-client deps                                            | `bridge-client` feature                                                                          | Same feature name                                                                                                                                                          | None               | No PT stack pulled in                                                                                                                                                                                     |

Compile-failure record: **zero failures across all six classes**. `cargo check`
finished clean on the first attempt after the version bump (35.9 s), so there
was nothing mechanical to fix and no semantic/config/runtime/feature adaptation
to make. The only code change is the `version()` string (`0.43` → `0.46`).

## Feature audit

`rust/arti-kmp-ffi/Cargo.toml` after migration (only the two version numbers changed):

```toml
arti-client = { version = "0.46", default-features = false, features = [
    "tokio",
    "rustls",
    "compression",
    "bridge-client",
    "onion-service-client",
    "static-sqlite",
] }
tor-rtcompat = { version = "0.46", features = ["tokio", "rustls"] }
```

Enabled arti-client features after migration (via `cargo tree -e features -i arti-client@0.46.0`):
`tokio`, `rustls`, `compression`, `bridge-client`, `onion-service-client`,
`static-sqlite` (+ internal `__is_nonadditive`, `tor-hsclient`, `tor-hscrypto` as
children of `onion-service-client`).

Proof of no accidental features:

- `cargo tree | grep -iE 'openssl|native-tls|tor-ptmgr|tor-hsservice|tor-rpc|experimental'`
  → **zero hits**.
- `cargo tree -e normal | grep -cE 'native-tls|openssl'` → **0**.
- `git diff rust/arti-kmp-ffi/Cargo.toml` shows only the two `0.43` → `0.46`
  version lines; feature lists are byte-identical.
- Explicitly NOT enabled (unchanged): `native-tls`, `pt-client`,
  `onion-service-service`, `rpc`, `experimental`, `experimental-api`, `full`.
- No compilation event forced enabling any of the above; nothing was stopped or
  silently enabled.

## Test matrix

| Check                                                             | Result                                                                                                                                                                                                                                                                                | Classification                                                 |
|-------------------------------------------------------------------|---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|----------------------------------------------------------------|
| `cargo check` (0.46, host)                                        | PASS, 0 failures                                                                                                                                                                                                                                                                      | COMPILE VERIFIED                                               |
| `cargo check --examples` (`host_poc` with `on_error`)             | PASS                                                                                                                                                                                                                                                                                  | COMPILE VERIFIED                                               |
| `cargo test` (`rust/arti-kmp-ffi`)                                | **14/14 PASS** (config identity ×8, error types, dirs, typed-error ordering, Bind collision, ephemeral SOCKS, conn-track drain)                                                                                                                                                       | DETERMINISTIC TEST VERIFIED                                    |
| `:tor:compileTestKotlinIosSimulatorArm64` (Ubique bindings regen) | PASS, `hide-sqlite3-symbols` ran                                                                                                                                                                                                                                                      | COMPILE VERIFIED                                               |
| `:tor:iosSimulatorArm64Test`                                      | **45/45 PASS** (24 lifecycle + 8 config + 6 error-map + 6 invariant + 1 live E2E), 0 failures/errors/skips                                                                                                                                                                            | DETERMINISTIC TEST VERIFIED (44) + SIMULATOR LIVE VERIFIED (1) |
| Live iOS simulator E2E `bootstrapFetchPauseResume` on 0.46        | PASS in 54.3 s: `arti version = arti-kmp-ffi 0.2.0 (arti-client 0.46, rustls)` → bootstrap 100% → SOCKS `127.0.0.1:49362` → `HTTP/1.1 200 OK` via Tor (`api.ipify.org`, exit `185.220.100.240`) → pause → resume → 2nd HTTP 200 → shutdown → 2nd cold start → 3rd HTTP 200 → shutdown | SIMULATOR LIVE VERIFIED                                        |
| `:tor:compileKotlinIosArm64`                                      | PASS                                                                                                                                                                                                                                                                                  | COMPILE VERIFIED                                               |
| `:tor:assembleAndroidDeviceTest`                                  | PASS                                                                                                                                                                                                                                                                                  | COMPILE VERIFIED (live device run not attempted)               |
| `:tor:bundleAndroidMainAar -PreleaseBuild=true`                   | PASS;   AAR contains exactly:arm64-v8a armeabi-v7a x86_64                                                                                                                                                                                                                             | COMPILE VERIFIED                                               |
| Android 16 KB-page alignment (NDK `llvm-readelf`)                 | `arm64-v8a` LOAD Align `0x4000` ✓, `x86_64` `0x4000` ✓, `armeabi-v7a` `0x1000` (32-bit, exempt)                                                                                                                                                                                       | COMPILE VERIFIED                                               |
| Consumer R8 rules / JNA wiring                                    | `tor/consumer-rules.pro` unchanged; `com.sun.jna.**` + `com.yet.tor.ffi.**` keeps present; AAR ships `proguard.txt` + `classes.jar`                                                                                                                                                   | COMPILE VERIFIED (by inspection)                               |
| Live Android E2E (`:tor:connectedAndroidDeviceTest`)              | Not run — no `adb`/device/emulator in this environment                                                                                                                                                                                                                                | NOT VERIFIED                                                   |
| iOS hardware device live                                          | Not run                                                                                                                                                                                                                                                                               | NOT VERIFIED                                                   |

No tests were removed, weakened, or added. The task's minimum regression
coverage (builder/config creation, bootstrap progress, bootstrap cancellation,
ephemeral SOCKS, fixed-port Bind, pause/resume, shutdown/start-again, bridge
parsing, direct `TorClient::connect` behind SOCKS, config identity) is already
covered by the existing 14 + 45 tests; since 0.43 → 0.46 introduced zero
semantic differences in this surface, generic test inflation was avoided per
scope.

## Binary size comparison

Release builds, like-for-like (`-PreleaseBuild=true` / `release` profile,
`opt-level = "z"`, `lto = true`). Baselines rebuilt from the pre-migration
`Cargo.toml`/`Cargo.lock` (arti-client 0.43.0) on the same host/toolchain.

Android `.so` (Ubique `tor/build/uniffi/build/rust/*/release`, unstripped):

| ABI           | 0.43 size (bytes) | 0.46 size (bytes) | Delta bytes | Delta % |
|---------------|-------------------|-------------------|-------------|---------|
| `arm64-v8a`   | 7,921,520         | 8,036,768         | +115,248    | +1.45%  |
| `armeabi-v7a` | 5,077,828         | 5,131,140         | +53,312     | +1.05%  |
| `x86_64`      | 8,979,048         | 9,110,760         | +131,712    | +1.47%  |

Packaged AAR `tor-release.aar` (0.46, stripped): `arm64-v8a` 7,894,968,
`armeabi-v7a` 5,056,784, `x86_64` 8,939,816 (baseline AAR was not repackaged
during the baseline rebuild, so the unstripped table above is the controlled
comparison).

Apple static archives (Ubique-processed, post `hide-sqlite3-symbols.sh`):

| Archive             | 0.43 size (bytes) | 0.46 size (bytes) | Delta bytes | Delta % |
|---------------------|-------------------|-------------------|-------------|---------|
| `iosArm64`          | 26,484,344        | 27,329,776        | +845,432    | +3.19%  |
| `iosSimulatorArm64` | 26,468,192        | 27,309,720        | +841,528    | +3.18%  |

Pristine Cargo output (pre-hook intermediate, for context): 0.43 `~103 MB`
(`ls`) vs 0.46 `110,620,240` / `110,615,840` bytes (`~105.5 MB`).

Interpretation: a uniform **+1–3%** growth from three minor Arti releases is
material but expected (directory/circuit/crypto churn, not a new subsystem).
No evidence of an accidentally linked stack (no OpenSSL/PT/RPC in the graph —
see feature audit). Not a failure.

## SQLite audit

- Dependency graph unchanged: `libsqlite3-sys 0.37.0` + `rusqlite 0.39.0` via
  `tor-persist`, still pulled by `static-sqlite`. Arti 0.46 bundles/exports
  SQLite exactly as before.
- Pristine 0.46 Cargo output (pre-hook) still demonstrates necessity:
  `llvm-nm …/target/aarch64-apple-ios/release/libarti_kmp_ffi.a | grep -c "[TDSC] _sqlite3"`
  → **283 globals** (e.g. `T _sqlite3_aggregate_context`); sim archive identical.
- Processed 0.46 Ubique archives (post `rust/hide-sqlite3-symbols.sh`, which ran
  automatically during both Apple builds — log: `hide-sqlite3-symbols:
  localized bundled sqlite3 in …`):
  `llvm-nm …/tor/build/uniffi/build/rust/aarch64-apple-ios/release/libarti_kmp_ffi.a`
  → **0 globals**; `aarch64-apple-ios-sim` → **0 globals**.
- Hook (`rust/hide-sqlite3-symbols.sh`) and its Gradle wiring
  (`tasks.withType<CargoBuildTask>()` Apple-triple gate) are **preserved
  unmodified**. Nothing was removed on the grounds that compilation succeeds.

## Remaining risks

1. **Android live E2E not run** — same standing follow-up as the 0.2.0
   conformance report. Execute `:tor:connectedAndroidDeviceTest` on hardware
   before any release claim covering Android.
2. **Live fixed-port collision not exercised on-device** — Bind path proven by
   Rust (`fixed_port_collision_is_typed_bind`) + facade tests only.
3. **Multi-instance** remains single-instance-by-documentation (`LOG_SINK`
   last-writer-wins for logs; status/error per-instance). Unchanged.
4. **Upstream churn risk**: three Arti minor versions moved the full `tor-*`
   graph; the +1–3% size growth is accounted for, but future Arti minor bumps
   should repeat this report's feature/tree + symbol + size gates.

## Final recommendation

**PASS WITH FOLLOW-UPS**

ArtiTor's 0.2 lifecycle contract is behaviorally preserved on Arti 0.46 (zero
API adaptations, 14 + 45 tests green, full live Tor sequence re-verified on the
iOS simulator with the 0.46 version string in the log); no new public feature
was introduced; all deterministic/compile gates are green; no
security-sensitive packaging regression was found (rustls-only, no
native-tls/PT/server/RPC, SQLite isolation effective, 16 KB pages correct for
64-bit ABIs). Ship this as the new Arti baseline after follow-up (1): the
Android live E2E on hardware. Do not begin 0.3.0 feature expansion here.
