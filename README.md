# ArtiTor

Kotlin Multiplatform wrapper over [Arti](https://gitlab.torproject.org/tpo/core/arti) (Tor,
implemented in Rust). One dependency gives you an embedded Tor client with a local SOCKS proxy and
**first-class bootstrap status** — no native build, no hand-written JNI, no log scraping.

- Bindings generated with [Gobley](https://gobley.dev) (UniFFI for Kotlin Multiplatform): one Rust
  surface → Kotlin for Android (JNI) and Kotlin/Native (iOS).
- rustls only (no OpenSSL). Android `.so` are 16 KB-page aligned (Google Play, Nov 2025).
- The async tokio runtime lives inside the native layer; calls never block the caller's thread.

## Status

PoC proven end-to-end on Android (arm64, on-device) and the iOS simulator: bootstrap to 100% over the
real Tor network, then an HTTP request through the SOCKS proxy exits via a Tor relay.

## Install

```kotlin
// settings.gradle.kts -> dependencyResolutionManagement { repositories { mavenCentral() } }
commonMain.dependencies {
    implementation("com.yet.tor:tor:0.1.0")
}
```

The Android `.so` for all four ABIs are bundled inside the AAR (`jniLibs`); AGP merges them into your
APK automatically. The iOS static library ships transitively via the KMP artifact. **No native or
Gradle configuration is required in the consumer app.**

## Usage

```kotlin
val tor = ArtiTorClient()

scope.launch {
    tor.status.collect { s -> println("Tor ${s.state} ${s.bootstrapPercent}% socks=${s.socksPort}") }
}

// dataDir is provided by you (Android: filesDir-relative; iOS: Application Support / Caches).
tor.start(ArtiConfig(dataDir = dir, socksPort = 9050)).getOrThrow()
// "Proxy ready" == state == RUNNING && bootstrapPercent == 100 && socksPort != null.

// Route traffic through 127.0.0.1:<socksPort> (e.g. java.net.Proxy(SOCKS) on Android).
tor.stop()
```

## What stays in the consuming app (out of scope for this library)

- **Permissions / foreground service**: `INTERNET` and any foreground-service declaration belong in
  the app manifest.
- **On-demand `.so` delivery**: if you don't want the `.so` in the base APK, use Play Feature
  Delivery / a dynamic feature module — declared by the app, not this library.
- **iOS background execution**: a Tor session in the background follows iOS background-execution
  rules; this library targets foreground use.

## Targets

Required: Android `arm64-v8a` / `armeabi-v7a` / `x86` / `x86_64`, `iosArm64`, `iosSimulatorArm64`.
Scaffolded (easy to enable): macOS, Linux, Windows desktop. **wasm is unsupported** — browsers have
no raw TCP, so Tor cannot work there.

## License & attribution

Apache-2.0. Bundles Arti (© The Tor Project, Apache-2.0/MIT). See [NOTICE](NOTICE).
