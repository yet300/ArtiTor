# ADR 0002: Bindings via Ubique (UniFFI for KMP), replacing Gobley

Status: Accepted (2026-09-29). Supersedes [ADR 0001](0001-bindings-gobley-vs-cffi.md).

## Context

ArtiTor 0.2.0 generated its Kotlin bindings with Gobley 0.3.7
(`dev.gobley.cargo` + `dev.gobley.uniffi`), which pinned the whole toolchain:
Kotlin 2.1.10, AGP 8.7.3, Gradle 8.12, kotlinx-coroutines ≤ 1.10.2,
vanniktech-maven-publish 0.30.0, UniFFI 0.29.x. Gobley maintenance and toolchain
stagnation blocked upgrades (notably the modern Android-KMP plugin and current
Kotlin), while
[UbiqueInnovation/uniffi-kotlin-multiplatform-bindings](https://github.com/UbiqueInnovation/uniffi-kotlin-multiplatform-bindings)
— the maintained successor of the same Trixnity lineage — tracks current
Gradle/Kotlin/AGP and UniFFI releases.

## Decision

Replace the three-plugin Gobley integration with the unified
`ch.ubique.uniffi.plugin` 1.2.1, keeping the same architecture: one UniFFI
export surface in `rust/arti-kmp-ffi` generates the bindings; the handwritten
`com.yet.tor` KMP facade (`ArtiTorClient`, `ArtiConfig`, `TorStatus`, …)
remains the public API and hides all generated FFI types.

Target stack: UniFFI 0.32.0, Kotlin 2.4.20, AGP 9.3.1 (via
`com.android.kotlin.multiplatform.library`), Gradle 9.6.1,
kotlinx-coroutines 1.11.0, atomicfu 0.33.0, vanniktech-maven-publish 0.33.0,
NDK r28c, Java 21, JVM target 17. `arti-client` stays at 0.43 in this
migration; the Arti upgrade and any public API expansion are separate phases.

## Consequences

- Android is built with the modern KMP Android DSL (`kotlin { android { … } }`,
  device tests in `androidDeviceTest`); consumer R8 rules are published via
  `optimization { consumerKeepRules … }`.
- Release-versus-debug Rust artifacts are selected by `-PreleaseBuild=true`,
  which CI, packaging, and publication builds must pass (replacing Gobley's
  `nativeVariant = Variant.Release`).
- Legacy 32-bit Android `x86` (`i686-linux-android`) is dropped: Ubique 1.2.1
  exposes no x86 Android release target. The supported ABI matrix is now
  `arm64-v8a`, `armeabi-v7a`, `x86_64`. 32-bit ARM (`armeabi-v7a`) is retained.
- `compileSdk` moves 35 → 37 because Ubique's `runtime-android` 1.2.1 requires
  API 37.
- The Apple SQLite symbol isolation (`rust/hide-sqlite3-symbols.sh`) is kept,
  re-hooked onto Ubique's `CargoBuildTask` API (triple-based Apple detection).
- No handwritten C/JNI: Android stays on JNA-backed generated bindings, Apple
  on Kotlin/Native cinterop. The FFI implementation remains internal behind the
  stable KMP facade (source-compatible: no `ArtiTorClient.kt` changes needed).

## Fallback

If Ubique fails on a future target, fall back to hand-written C-FFI + cinterop
+ JNI shim for that target (as ADR 0001 already documented). Not needed so far.
