# ArtiTor

Kotlin Multiplatform wrapper over [Arti](https://gitlab.torproject.org/tpo/core/arti) (Tor,
implemented in Rust). One dependency gives you an embedded Tor client with a local SOCKS proxy and
**first-class bootstrap status** — no native build, no hand-written JNI, no log scraping.

- Bindings generated with [Gobley](https://gobley.dev) (UniFFI for Kotlin Multiplatform): one Rust
  surface → Kotlin for Android (JNI) and Kotlin/Native (iOS).
- rustls only (no OpenSSL). Android `.so` are 16 KB-page aligned (Google Play, Nov 2025).
- The async tokio runtime lives inside the native layer; calls never block the caller's thread.
- **Lifecycle**: `start` / `pause` (keep client) / `resume` / `shutdown` — suited for chat apps
  that toggle Tor without a full re-bootstrap.

## Status

PoC proven end-to-end on Android (arm64, on-device) and the iOS simulator: bootstrap to 100% over the
real Tor network, then an HTTP request through the SOCKS proxy exits via a Tor relay.

See [docs/api-lifecycle-bitchat.md](docs/api-lifecycle-bitchat.md) for the 0.2 API design aimed at a
BitChat KMP port.

## Install

```kotlin
// settings.gradle.kts -> dependencyResolutionManagement { repositories { mavenCentral() } }
commonMain.dependencies {
    implementation("io.github.yet300:tor:0.2.0")
}
```

The Android `.so` for all four ABIs are bundled inside the AAR (`jniLibs`); AGP merges them into your
APK automatically. The iOS static library ships transitively via the KMP artifact. **No native or
Gradle configuration is required in the consumer app.**

## Usage

```kotlin
val tor = ArtiTorClient()

scope.launch {
    tor.status.collect { s ->
        println("Tor ${s.state} ${s.bootstrapPercent}% socks=${s.socksPort} ready=${s.isReady}")
    }
}

// dataDir is provided by you (Android: filesDir; iOS: Application Support).
// socksPort = 0 binds an ephemeral port (recommended).
tor.start(ArtiConfig(dataDir = dir, socksPort = 0)).getOrThrow()
// Ready == status.value.isReady (RUNNING + 100% + socksPort != null).

// Route traffic through 127.0.0.1:<socksPort> (e.g. java.net.Proxy(SOCKS) / Ktor).

// Soft stop: drop SOCKS, keep bootstrapped client (fast re-enable).
tor.pause()

// Bring SOCKS back without a full directory bootstrap.
tor.resume().getOrThrow()

// Full teardown (process exit / panic wipe).
tor.shutdown()
```

### SOCKS limits

The local proxy is **SOCKS5 CONNECT only** (no UDP/DNS associate). That is enough for HTTP(S) and
WebSocket clients (OkHttp, Ktor, URLSession) used by apps like BitChat. There is no DNS proxy port.

## What stays in the consuming app (out of scope for this library)

- **Permissions / foreground service**: `INTERNET` and any foreground-service declaration belong in
  the app manifest.
- **HTTP client wiring**: attach `127.0.0.1:socksPort` as a SOCKS proxy in OkHttp/Ktor/URLSession.
- **Tor ON/OFF prefs, fail-closed policy, Nostr reconnect**: app layer (see lifecycle doc).
- **On-demand `.so` delivery**: Play Feature Delivery / dynamic feature module — app concern.
- **iOS background execution**: library targets foreground use; app decides pause vs keep-alive.
- **Pluggable transports (obfs4/snowflake binaries)**: not bundled; plain bridge lines supported
  via `ArtiConfig.bridges`.

## Targets

Required: Android `arm64-v8a` / `armeabi-v7a` / `x86` / `x86_64`, `iosArm64`, `iosSimulatorArm64`.
Scaffolded (easy to enable): macOS, Linux, Windows desktop. **wasm is unsupported** — browsers have
no raw TCP, so Tor cannot work there.

## License & attribution

Apache-2.0. Bundles Arti (© The Tor Project, Apache-2.0/MIT). See [NOTICE](NOTICE).
