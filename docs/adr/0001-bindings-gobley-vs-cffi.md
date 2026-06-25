# ADR 0001: Bindings via Gobley (UniFFI for KMP), not hand-written C-FFI

Status: Accepted (2026-06-25)

## Context

ArtiTor needs to expose a small Rust surface over `arti-client` (start/stop/status/version/logs) to
Kotlin on Android (JNI) and iOS (Kotlin/Native). Two options were considered:

1. **Gobley** — a Gradle plugin pair (`dev.gobley.cargo` + `dev.gobley.uniffi`) that runs cargo /
   cargo-ndk, generates Kotlin bindings from a single UniFFI interface, and bundles the `.so`
   (Android) / `.a` (iOS) into the KMP artifact.
2. **Hand-written C-FFI** — `extern "C"` staticlib + cinterop (iOS) + a JNI shim (Android), as the
   consuming project (bitMessage) does today with `ArtiNative`.

## Decision

Use **Gobley 0.3.7**. A PoC proved it end-to-end: bootstrap to 100% (first-class, from
`bootstrap_events()`) plus a SOCKS fetch exiting via Tor, on Android arm64 (on-device) and the iOS
simulator — with no hand-written JNI.

## Consequences

- Pinned to Gobley 0.3.7's toolchain: **Kotlin 2.1.10, AGP 8.7.3, Gradle 8.12**, classic
  `com.android.library` + `kotlin("multiplatform")`. Knock-on pins: kotlinx-coroutines ≤ 1.10.2
  (1.11.0 is klib-ABI-incompatible with Kotlin/Native 2.1.10), vanniktech-maven-publish 0.30.0
  (0.37.0 needs AGP ≥ 8.13). Gradle configuration cache is disabled (Gobley tasks not compatible).
- Single source of truth for the API: the `#[uniffi::export]` surface in `rust/arti-kmp-ffi`.

## Fallback

If Gobley fails on a future target, fall back to hand-written C-FFI + cinterop + JNI shim for that
target. The existing bitMessage `tools/arti-build` proves that path on Android. Not needed so far.
