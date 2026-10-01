# ArtiTor

Kotlin Multiplatform wrapper over [Arti](https://gitlab.torproject.org/tpo/core/arti) (Tor,
implemented in Rust). One dependency gives you an embedded Tor client with a local SOCKS proxy and
**first-class bootstrap status** — no native build, no hand-written JNI, no log scraping.

- Bindings generated with
  [UbiqueInnovation/uniffi-kotlin-multiplatform-bindings](https://github.com/UbiqueInnovation/uniffi-kotlin-multiplatform-bindings)
  (UniFFI for Kotlin Multiplatform): one Rust surface → Kotlin for Android
  (JNA-backed generated bindings) and Kotlin/Native cinterop (iOS).
- rustls only (no OpenSSL). The bundled 64-bit Android JNI libraries satisfy
  Android's 16 KB page-size alignment requirement.
- The async tokio runtime lives inside the native layer; bootstrap runs asynchronously. Lifecycle calls perform synchronous native state transitions.
- **Lifecycle**: `start` / `pause` (keep client) / `resume` / `shutdown` — suited for chat apps
  that toggle Tor without a full re-bootstrap.

## Status

The repository implements the frozen 0.3 API in
[the API contract](docs/design/ARTITOR_0_3_API_FREEZE.md). The 0.3.0 release candidate adds isolation sessions,
bridge policy, error categories, and onion/timeout controls.

See [the integrated implementation report](docs/audit/ARTITOR_0_3_INTEGRATED_IMPLEMENTATION_REPORT.md)
for exact test targets and remaining release evidence. Simulator results do not establish
physical iOS device performance or background execution support.

## Install

The APIs below require the **0.3.0 release candidate**. Local Maven consumer verification
checks the candidate with `mavenLocal()`; it does not establish Maven Central availability.
For local candidate validation, run `./gradlew :tor:publishToMavenLocal -PreleaseBuild=true`
and use `mavenLocal()` in a separate consumer. The release flag builds all three Android ABIs.
For a published release, use `mavenCentral()` after verifying that 0.3.0 is available.

```kotlin
// settings.gradle.kts -> dependencyResolutionManagement { repositories { mavenCentral() } }
commonMain.dependencies {
    implementation("io.github.yet300:tor:0.3.0")
}
```

The Android `.so` for all three supported ABIs are bundled inside the AAR (`jniLibs`); AGP merges them into your
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

### One isolation session per application identity

The root/default SOCKS endpoint is one circuit isolation domain. Use an additional
`TorIsolationSession` for each application identity that needs its own domain:

```kotlin
val alice = tor.createIsolationSession().getOrThrow()
val bob = tor.createIsolationSession().getOrThrow()
// Build separate HTTP clients and connection pools from each handle's current
// status.value.socksEndpoint. Never share pools across identities or sessions.
alice.status.collect { status ->
    // Close Alice's previous HTTP pool on each status/endpoint change.
    // ACTIVE: create a fresh SOCKS-only pool for status.socksEndpoint.
    // PAUSED/CLOSED/INVALIDATED: keep Alice unavailable; never route directly.
}
```

[The compilable application TorController example](docs/examples/TorController.kt) maps identity
strings to session handles and uses an application-supplied `HttpPoolFactory`. It discards pools
when session status changes, builds them from the current endpoint on demand, rechecks status after
factory suspension and before use, and clears failed initialization. The factory must create a
fresh pool per identity/session/endpoint, resolve target names through SOCKS, and abort outstanding
requests when its pool closes. Platform HTTP configuration belongs in that factory; a global or
shared default connection pool violates this separation.

`pause()` retains circuit identity while synchronously removing root and session endpoints.
`resume()` rebinds listeners; ports may change. `close()` closes one handle; `shutdown()`, `restart()`,
and client-defining configuration replacement invalidate old handles permanently. Closed and
invalidated handles remain terminal observers. Close an application's identity entry explicitly
before creating a new session for it; handles never silently attach to a new client generation.

This partitions circuits across identities. It does not guarantee different exit relays or
persistent identity unlinkability. Application cookies, login state, timing, and other identifiers
still belong to the application's isolation model.

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
- **Pluggable transports (obfs4/snowflake)**: unsupported in stable 0.3; PT-shaped bridge
  lines fail with `ArtiException.Config`. No PT binaries or transport features are enabled.

## Bridge configuration

`ArtiConfig.bridges` accepts one direct bridge line per entry. Each line is parsed and built
with Arti's bridge parser in Rust before any bootstrap worker or TorClient is created.
Direct lines require a numeric IP endpoint and an RSA identity fingerprint; an optional
`Bridge` prefix and an optional ed25519 identity follow upstream syntax.

`bridgesEnabled` defaults to `BridgesEnabled.AUTO`, preserving existing callers:

| Policy | Behavior |
|---|---|
| `AUTO` | Use bridges iff the list is non-empty. |
| `ON` | Require a non-empty valid list; an empty list fails with `ArtiException.Config`. |
| `OFF` | Retain and validate supplied lines, but use normal guards. |

Malformed lines, including empty/whitespace entries and unsupported PT-shaped lines, fail
with the existing typed `ArtiException.Config` in **every** mode, including `OFF`.
Diagnostics do not include bridge input. Treat bridge configuration as secret: do not log
`ArtiConfig` or store it in unprotected preferences.

Changing `bridges` or `bridgesEnabled` rebuilds TorClient and invalidates existing isolation
sessions. Lists are compared exactly, including order and whitespace, even in `OFF` mode.
Stable 0.3 does not use live `TorClient::reconfigure()`.

## Onion addresses and network timeouts

`.onion` names use the same SOCKS5 CONNECT endpoint as other targets. Pass the hostname to SOCKS;
never resolve it with the platform DNS resolver. Stable 0.3 supports public onion clients, with no
onion-service hosting or authenticated onion-client API.

```kotlin
import kotlin.time.Duration.Companion.seconds

val config = ArtiConfig(
    dataDir = dir,
    allowOnionAddrs = true,       // default
    connectTimeout = 10.seconds, // default Arti connect timeout
    resolveTimeout = 10.seconds, // default Arti resolve timeout
)
```

`connectTimeout` bounds Tor's BEGIN exchange after circuit acquisition; `resolveTimeout`
bounds the resolution operation after circuit acquisition. Neither is a whole HTTP request,
circuit acquisition, or bootstrap deadline. The `start`/`resume` readiness deadline is separate.
Zero is permitted. Values must be finite, nonnegative, and representable exactly as signed
64-bit nanoseconds.
Configuration failures return `ArtiException.Config`; input is never clamped or wrapped.
Changing any of these three values rebuilds TorClient and invalidates isolation sessions.
Invalid replacement configuration is validated before retained RUNNING/PAUSED resources are
discarded, including `restart(config)`. A valid rebuild can still fail during bootstrap or binding.

## Typed failures

The original seven `ArtiException` subclasses remain exhaustive. Each now has `kind: TorErrorKind`
for stable classification independent of diagnostic text. Arti bootstrap/runtime failures retain
categories such as `NETWORK`, `STORAGE`, `BOOTSTRAP_REQUIRED`, and `TARGET_REJECTED` when their
underlying error path supplies them. Unsupported future categories map to `UNKNOWN`.

SOCKS connection failures arrive as SOCKS replies; this API does not turn every per-stream network
error into an engine exception or promise a `TorErrorKind` callback for every rejected target.
Use kinds where provided; never classify by parsing messages. Native lifecycle and SOCKS
diagnostics redact bridge material, target names, and keys. Upstream tracing forwards only
level/module metadata; event payloads and panic payloads are withheld. Avoid logging
configurations, requests, or application identity secrets.

## Targets

Required: Android `arm64-v8a` / `armeabi-v7a` / `x86_64`; minimum iOS version: **15.0** (`iosArm64` and `iosSimulatorArm64`).
Scaffolded (easy to enable): macOS, Linux, Windows desktop. **wasm is unsupported** — browsers have
no raw TCP, so Tor cannot work there.

> Compatibility note: 32-bit Android `x86` (`i686-linux-android`) was dropped in the
> Gobley → Ubique binding migration (0.2.0): the Ubique plugin 1.2.1 does not expose an
> x86 Android release target, and only ships `arm64-v8a`, `armeabi-v7a`, `x86_64`.
> `armeabi-v7a` (32-bit ARM) is still fully supported.

## License & attribution

Apache-2.0. Bundles Arti (© The Tor Project, Apache-2.0/MIT). See [NOTICE](NOTICE).
