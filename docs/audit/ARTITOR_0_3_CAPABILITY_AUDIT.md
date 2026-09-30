# ArtiTor 0.3.0 Capability, Architecture, Mobile-Feasibility & Public-API Audit

Date: 2026-09-29
Local HEAD: `0ac8867` (`deps(arti): upgrade to arti-client 0.46; pin Rust 1.91 in CI`)
Upstream baseline: **`arti-client 0.46.0`** (Tor Project Arti 2.6 generation)
Normative inputs (unchanged by this audit):
- `docs/api-lifecycle-bitchat.md` (0.2 lifecycle contract — stable)
- `docs/audit/ARTITOR_0_2_LIFECYCLE_CONFORMANCE_REPORT.md`
- `docs/audit/ARTI_0_46_UPGRADE_REPORT.md`
- `rust/arti-kmp-ffi/src/lib.rs`, `tor/src/commonMain/.../ArtiTorClient.kt`
- `rust/arti-kmp-ffi/Cargo.toml`, Ubique UniFFI 1.2.1 / uniffi 0.32.2

Scope rules honored: no production code modified, no Cargo features enabled,
no lifecycle redesign, no version bumps, no publishing. Audit/design docs only.

Method: primary upstream sources — local crate sources under
`~/.cargo/registry/src/index.crates.io-*/{arti-client,tor-circmgr,tor-proto,tor-config,tor-error,tor-guardmgr,tor-dirmgr,tor-chanmgr}-0.46.0`,
`arti-client-0.46.0/Cargo.toml[.orig]` feature graph, `docs.rs` for
`tor-ptmgr`/`tor-hsservice` (not in the local lockfile), plus read-only
`cargo tree` probes in `/tmp` (committed baseline untouched).
Stability is never inferred from "it compiles". Every claim below cites the
exact API name + feature gate + file reference. Inferences (notably mobile-OS
behaviour, UniFFI throughput) are marked `[inference]`.

Architectural principle (retained): **thin Tor engine, application-owned policy.**
ArtiTor owns embedded `TorClient`, lifecycle, bootstrap status, SOCKS endpoint,
Tor-specific sessions/capabilities, typed Tor errors/events. Applications own
prefs, HTTP/Ktor/OkHttp/URLSession config, UI, foreground/background policy,
account/session policy, Nostr/WebSocket reconnect, PT binary delivery (unless
this audit proves otherwise — it does not, see §PT).

---

## Table of contents

1. [Executive summary](#1-executive-summary)
2. [Upstream stability matrix](#2-upstream-stability-matrix)
3. [Capability deep dives](#3-capability-deep-dives)
4. [Feature / dependency matrix](#4-feature--dependency-matrix)
5. [Binary impact](#5-binary-impact)
6. [Public API proposal](#6-public-api-proposal)
7. [Rust / UniFFI architecture proposal](#7-rust--uniffi-architecture-proposal)
8. [Lifecycle interactions](#8-lifecycle-interactions)
9. [Security / privacy considerations](#9-security--privacy-considerations)
10. [Android limitations](#10-android-limitations)
11. [iOS limitations](#11-ios-limitations)
12. [Test strategy](#12-test-strategy)
13. [Versioning / source compatibility](#13-versioning--source-compatibility)
14. [Implementation phases](#14-implementation-phases)
15. [Deferred capabilities](#15-deferred-capabilities)
16. [Open questions](#16-open-questions)
17. [Final recommendation](#17-final-recommendation--proposed-030-scope)

---

## 1. Executive summary

**What 0.3 should actually contain (minimum coherent, defensible scope):**

`ADOPT_0_3` (stable public API in 0.3.0):

1. **Isolation sessions, SOCKS-level** — N named sessions sharing one `TorClient`
   (one `isolated_client()` handle each, or one `StreamPrefs::new_isolation_group()`
   each), each with its **own SOCKS endpoint** (per-session ports). No raw
   `IsolationToken` over UniFFI; Kotlin gets opaque `TorIsolationSession`
   handles. `isolate_every_stream` is never exposed.
2. **Typed bridge configuration (validation only, transport stays `List<String>`)**
   — keep `ArtiConfig.bridges: List<String>` wire shape, but parse/validate every
   line in Rust via `BridgeConfigBuilder::parse/build` so malformed lines become
   typed `ArtiException.Config` before bootstrap. Add an `enabled: Auto/On/Off`
   tri-state (maps to `BridgesConfig.enabled: BoolOrAuto`) instead of today's
   implicit "empty = off".
3. **Ordinary `.onion` client as documented no-op** — no new API; prove + document
   that SOCKS `.onion` already works via the compiled-in stable
   `onion-service-client` feature with default `allow_onion_addrs=true`.
4. **Richer stable error taxonomy (additive, `ErrorKind`-backed)** — keep all 7
   current `ArtiException` variants; add ~5 stable buckets mapped **only** from
   `tor_error::ErrorKind::kind()` (`Network`, `ExitFailed`, `TargetRejected`,
   `Storage`, `BootstrapRequired`-style), with an `Unknown/Runtime` fallback
   (because `ErrorKind` is `#[non_exhaustive]`). Never expose `ErrorDetail`
   (`error_detail` feature = `__is_experimental`, voids semver).
5. **Address-filter + stream-timeout config knobs** (`allow_onion_addrs`,
   `allow_local_addrs`, `connect/resolve_timeouts`) — the only two
   `TorClientConfig` sections both live-reconfigurable *and* safe for normal
   KMP apps. Everything else stays internal or rejected (see §15).

`ADOPT_INTERNAL` (use inside Rust/Kotlin in 0.3, not public):

- `TorClient::connect_with_prefs` inside the SOCKS accept loop (per-session
  prefs/client selection); `TorClient::reconfigure` for the two whitelisted
  sections only (dry-run `CheckAllOrNothing` first); dormant-mode wiring
  (`set_dormant`) behind an internal flag for dogfooding.

`EXPERIMENTAL` (explicitly *not* in stable 0.3 API):

- Dormant-mode public API (`goDormant/wake`); unmanaged-PT plumbing
  (`proxy_addr` loopback transport entries); authenticated onion-client keys
  (all `experimental-api` + `keymgr` gated).

`DEFER` (valid, later): onion-service hosting, vanguards knob, full
`TorClient::reconfigure` surface, per-stream `DataStream` API, SOCKS
username/password auth multiplexing (alternative to per-session ports),
managed-PT distribution.

`REJECT` (does not belong): RPC (`tor-rpcbase`), managed PT on iOS,
`isolate_every_stream` exposure, raw `IsolationToken`/`StreamPrefs`/
`DataStream`/`TorClientConfig` over UniFFI, full advanced-config passthrough,
geographic exit pinning (`geoip` is `__is_experimental`).

**Why this shape:** isolation-over-SOCKS is the only design that (a) reuses the
stable `isolated_client` primitive (shares runtime/dirmgr/circmgr, isolates
only circuit assignment — exactly the Tor-correct semantic), (b) needs no new
byte path over UniFFI (Rust keeps doing `tokio::io::copy`, apps keep using
OkHttp/Ktor/URLSession), (c) is testable deterministically (circuit-sharing
assertions via `circmgr` internals in Rust tests, not exit-IP comparison), and
(d) survives the precompiled-Maven constraint (zero new Cargo features for the
`ADOPT_0_3` core: isolation + onion-client + errors + address/timeout knobs all
work on the *current* feature set; only `enabled: BoolOrAuto` needs no new
feature either).

**What 0.3 must NOT do:** enable `pt-client` / `onion-service-service` /
`vanguards` / `rpc` / `experimental-api` in the shipped artifact (each forces
all Maven consumers to pay binary/deps cost they cannot opt out of); expose
direct `DataStream`s; fold dormant into `pause()`; change config-identity
rebuild semantics; rename `ArtiTorClient` for aesthetics.

---

## 2. Upstream stability matrix

Legend: Stable = public API, no `__is_experimental`, covered by semver.
`Stable-F` = stable feature flag but operationally difficult on mobile.
Experimental = gated by `__is_experimental` (`experimental`, `experimental-api`,
`error_detail`, `geoip`, `restricted-discovery`, …). UniFFI risk: Low (record/enum/
plain async) / Medium (object lifetime, threading) / High (streaming bytes,
cancellation, key material).

| Capability                                                                                            | Arti API (0.46.0)                                                                                                                                                                    | Feature flag                                             | Stable?                                       | Mobile feasible?                                       | UniFFI risk                               | Recommendation                                                   |
|-------------------------------------------------------------------------------------------------------|--------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|----------------------------------------------------------|-----------------------------------------------|--------------------------------------------------------|-------------------------------------------|------------------------------------------------------------------|
| Isolated client handle                                                                                | `TorClient::isolated_client() -> Arc<TorClient>` (`client.rs:1466`)                                                                                                                  | *(none)*                                                 | **Stable**                                    | Yes (both)                                             | Low (opaque handle)                       | **ADOPT_0_3** (opaque session, per-session SOCKS)                |
| Prefs handle                                                                                          | `TorClient::with_prefs(StreamPrefs)` (`client.rs:1672`)                                                                                                                              | *(none)*                                                 | **Stable**                                    | Yes                                                    | Low                                       | **ADOPT_INTERNAL** (impl detail)                                 |
| Per-stream isolation group                                                                            | `StreamPrefs::new_isolation_group()` (`client.rs:839`)                                                                                                                               | *(none)*                                                 | **Stable**                                    | Yes                                                    | Low (server-side only)                    | **ADOPT_0_3** (as session impl, not public prefs)                |
| Explicit token                                                                                        | `StreamPrefs::set_isolation(IsolationToken)` (`client.rs:854`); `IsolationToken::new()` (`tor-circmgr/isolation.rs:206`)                                                             | *(none)*                                                 | **Stable**                                    | Yes                                                    | Medium (token identity over FFI)          | **ADOPT_INTERNAL** (never expose raw token)                      |
| Isolate every stream                                                                                  | `StreamPrefs::isolate_every_stream()` (`client.rs:875`)                                                                                                                              | *(none)*                                                 | Stable (with **"Use with care"** doc warning) | No (network abuse)                                     | —                                         | **REJECT** (public)                                              |
| Direct stream connect                                                                                 | `TorClient::connect/connect_with_prefs -> DataStream` (`client.rs:1539/1550`)                                                                                                        | *(none)*                                                 | **Stable**                                    | Yes (Rust-side)                                        | **High** (bytes, flush, cancel, lifetime) | **DEFER** (public); **ADOPT_INTERNAL** (inside `handle_socks`)   |
| `DataStream`/`DataReader`/`DataWriter`/`split`                                                        | `tor-proto/.../data.rs:137/665`; re-exported `lib.rs:75`                                                                                                                             | *(none)*                                                 | **Stable** (semver note in source)            | Yes (Rust)                                             | High                                      | **DEFER**                                                        |
| Ordinary `.onion` client                                                                              | `connect` HS branch (`client.rs:1606`); `allow_onion_addrs` default true (`config.rs`)                                                                                               | `onion-service-client` (stable, already on)              | **Stable**                                    | Yes                                                    | Low (none — SOCKS)                        | **ADOPT_0_3** (document/test, no new API)                        |
| Onion client auth keys                                                                                | `generate/rotate/insert/remove/get_service_discovery_key` (`client.rs:2052`)                                                                                                         | `onion-service-client` + `experimental-api` (+ `keymgr`) | **Experimental**                              | No (keystore needed)                                   | High (key material)                       | **EXPERIMENTAL**                                                 |
| Onion hosting launch/create                                                                           | `launch_onion_service` (`client.rs:1918`), `create_onion_service` (`client.rs:2222`), `OnionServiceConfig`                                                                           | `onion-service-service` (stable name, heavy deps)        | Stable-F                                      | No (always-on impossible)                              | High (RendRequest streams)                | **DEFER**                                                        |
| Hosting with fixed hsid                                                                               | `launch_onion_service_with_hsid` (`client.rs:1995`)                                                                                                                                  | `onion-service-service` + `experimental-api`             | **Experimental**                              | No                                                     | High                                      | **EXPERIMENTAL** at best; **DEFER** in practice                  |
| Vanguards                                                                                             | `VanguardConfig.mode` (`tor-guardmgr`); honored only `all(vanguards, any(onion-…))` (`config.rs:665`)                                                                                | `vanguards` (stable name)                                | Stable-F                                      | Partial (CPU/battery)                                  | Low                                       | **DEFER** (no knob; defaults suffice)                            |
| Dormant mode                                                                                          | `TorClient::set_dormant(DormantMode::{Normal,Soft})` (`client.rs:2254`); `DormantMode` is `#[non_exhaustive]`                                                                        | *(none)*                                                 | **Stable**                                    | Yes                                                    | Low                                       | **EXPERIMENTAL** (separate API, not in `pause()`)                |
| Runtime reconfigure                                                                                   | `TorClient::reconfigure(&TorClientConfig, Reconfigure)` (`client.rs:1397`); `Reconfigure::{AllOrNothing,WarnOnFailures,CheckAllOrNothing}` (`tor-config/lib.rs:161`, non-exhaustive) | *(none)*                                                 | **Stable** (with documented gaps, arti#1721)  | Yes                                                    | Low                                       | **ADOPT_INTERNAL** (2 sections); full surface **DEFER**          |
| Bridge lines                                                                                          | `BridgesConfigBuilder.bridges().push(line.parse()?)` (`config.rs:303`); `BridgeConfigBuilder::{parse,build}` (`tor-guardmgr/bridge/config.rs`)                                       | `bridge-client` (stable, already on)                     | **Stable**                                    | Yes                                                    | Low                                       | **ADOPT_0_3** (validate + enabled tri-state)                     |
| Unmanaged PT                                                                                          | `TransportConfig{protocols, proxy_addr}` (`tor-ptmgr/config.rs`)                                                                                                                     | `pt-client`                                              | Stable-F (no spawn)                           | Android/Desktop yes; iOS only with sidecar [inference] | Low                                       | **EXPERIMENTAL**                                                 |
| Managed PT                                                                                            | `TransportConfig{path, arguments, run_on_startup}` + `PtMgr::spawn` (`tor-ptmgr/managed.rs,ipc.rs`)                                                                                  | `pt-client` (+ `managed-pts` default)                    | Stable-F (process mgmt)                       | Android/Desktop only; **iOS infeasible** [inference]   | Medium                                    | **ADOPT_INTERNAL** (dogfood, Android/desktop); **REJECT** on iOS |
| Error kinds                                                                                           | `Error::kind() -> ErrorKind` (`tor-error`, ~60 variants, non-exhaustive); re-export `lib.rs:72`                                                                                      | *(none)*                                                 | **Stable**                                    | Yes                                                    | Low                                       | **ADOPT_0_3** (buckets + fallback)                               |
| Error details                                                                                         | `Error::detail() -> &ErrorDetail` (`err.rs:349`)                                                                                                                                     | `error_detail` (`__is_experimental`)                     | **Experimental** (voids semver)               | —                                                      | —                                         | **REJECT**                                                       |
| Bootstrap status/events                                                                               | `bootstrap_status/events`, `BootstrapStatus{as_frac,ready_for_traffic,blocked}` (`status.rs`, `client.rs:2232/2243`)                                                                 | *(none)*                                                 | **Stable**                                    | Yes                                                    | Low                                       | **ADOPT_0_3** (already used; extend summary)                     |
| Circuit/stream/PT/onion events                                                                        | `DirEvent`, `ConnStatus`, `ClockSkew`, `BridgeDescEvent`, `hs_circ_pool()`                                                                                                           | `experimental-api` or `pub(crate)`                       | Experimental/internal                         | —                                                      | —                                         | **DEFER**                                                        |
| RPC dispatch table                                                                                    | `TorClient::rpc_methods()` (`rpc.rs`); needs external daemon for transport                                                                                                           | `rpc` (+ `tor-rpcbase`)                                  | Stable-F (wrong layer)                        | No use case                                            | Medium                                    | **REJECT**                                                       |
| Address filter                                                                                        | `ClientAddrConfig{allow_local_addrs, allow_onion_addrs}` (`config.rs`) — live-replaceable                                                                                            | *(none / onion-service-client for onion bit)*            | **Stable**                                    | Yes                                                    | Low                                       | **ADOPT_0_3**                                                    |
| Stream timeouts                                                                                       | `StreamTimeoutConfig{connect,resolve,resolve_ptr}` 10s defaults (`config.rs:137`) — live-replaceable                                                                                 | *(none)*                                                 | **Stable**                                    | Yes                                                    | Low                                       | **ADOPT_0_3**                                                    |
| Bridges/channel/path/circuit-timing/preemptive/dir-schedule/storage/system/network/vanguards sections | `TorClientConfig{…}` (`config.rs:549`)                                                                                                                                               | mixed (see §15)                                          | Stable-F … Reject-live                        | —                                                      | —                                         | advanced/internal/reject per §15                                 |
| `geoip` exit-country pinning                                                                          | `StreamPrefs::exit_country`                                                                                                                                                          | `geoip` (`__is_experimental`)                            | **Experimental**                              | —                                                      | —                                         | **REJECT**                                                       |

Stability rule applied throughout: `#[non_exhaustive]` enums (`DormantMode`,
`ErrorKind`, `Reconfigure`, `SystemConfig`, `StreamTimeoutConfig`,
`BridgesConfig`) and `Display`-excluded-from-semver (`err.rs:28-33`) mean Kotlin
`when` must always have an `else`/fallback, and FFI enums must tolerate unknown
variants.

---

## 3. Capability deep dives

### 3.1 Isolation (TorClient isolation mechanisms)

**APIs inspected** (`arti-client-0.46.0/src/client.rs`, `tor-circmgr-0.46.0/src/isolation.rs`):

- `TorClient::isolated_client(&self) -> Arc<TorClient<R>>` (`client.rs:1466`):
  fresh `client_isolation: IsolationToken::new()`, cloned `connect_prefs`,
  `Arc::clone(&self.client)` — "share internal state and configuration, but
  their streams will never share circuits".
- `TorClient::with_prefs(&self, StreamPrefs) -> Arc<Self>` (`client.rs:1672`):
  **same** `client_isolation`, new defaults — does *not* isolate.
- `StreamPrefs::{new, set_isolation, new_isolation_group, isolate_every_stream,
  optimistic, connect_to_onion_services, ipv6_*}` (`client.rs:652-811`).
  `new_isolation_group()` = `set_isolation(IsolationToken::new())`.
  `isolate_every_stream()` mints a fresh token per `prefs_isolation()` call and
  carries an explicit "Use with care … greater load on the Tor network … not
  more privacy" warning (`client.rs:862-871`).
- Final decision is a **conjunction**: private `TorClient::isolation(prefs)`
  (`client.rs:1889`) builds `StreamIsolation{owner_token: client_isolation,
  stream_isolation: prefs token}`; two streams share a circuit iff **both**
  match (`isolation.rs:405-428`; cross-type `Isolation`s never compatible).
  Both `prefs_isolation()` and `TorClient::isolation()` are **private** — FFI
  can only influence via the four public entry points + `connect_with_prefs`.

**Semantic answers:**

1. *Isolated client/session*: fresh owner token; shares **everything else** —
   `ClientShared{R}` (`client.rs:124-197`): tokio runtime, `RunningInner`
   (`chanmgr`, **`circmgr`**, `dirmgr`, `guardmgr`, `hsclient`, `hs_circ_pool`),
   memquota, keymgr, statemgr, addr/timeout config, dormant sender. Circuit
   assignment is the *only* isolated dimension.
2. *Shared isolation token* (`set_isolation(same)` on two prefs): streams may
   share circuits **within the same `TorClient`** (owner equal + stream equal).
   Across two `isolated_client()`s the same stream token still does *not*
   share (owners differ).
3. *Per-stream isolation* (`new_isolation_group()` per identity): one fresh
   token per identity; correct granularity for accounts/tabs.
4. *Isolate-every-stream*: fresh token per `connect` — one circuit per stream.
   Upstream warning applies; never expose publicly.

- *Does an isolated client share directory state/runtime/circuit manager?*
  **Yes** — all shared via `Arc<ClientShared>` (see above). This is the point:
  cheaper than a second `TorClient`, Tor-correct (directory/guards shared,
  circuits partitioned).
- *What exactly is isolated?* Which streams may cohabit which circuits.
- *What isn't?* Directory/consensus, runtime, circuit/channel/guard managers,
  guard selection, keymgr/state files, config (`reconfigure` "applies to **all**
  TorClient instances derived from the same `create_*`" — `client.rs:1380`).
- *How long does isolation identity live?* `IsolationToken(u64)` from a
  process-global `AtomicU64` counter (`isolation.rs:207-214`, never recycled;
  `no_isolation()==0`). Client identity lives as long as the
  `Arc<TorClient>` handle (root minted once in `create_unbootstrapped`,
  `client.rs:1027`). Tokens already joined into live circuits persist there
  until circuits expire. `EveryStream` tokens die immediately after use.
- *Pause/resume?* `TorClient` has no close; ArtiTor `pause()` retains the
  `Arc<TorClient>` when bootstrapped → **same owner token survives
  pause/resume** (correlation-relevant, see §9). Pre-bootstrap `pause()`
  discards the partial client (token dies).
- *Shutdown?* `shutdown()` drops the client → all tokens held only by that
  handle die; circuits/channels tear down as refs drop. Cold restart mints a
  brand-new root token, but guard/state persistence may still link at the
  network layer (do not promise "new identity").
- *UniFFI representation?* **Opaque session handles, never raw tokens.**
  `IsolationToken` is `Copy+Eq(u64)` — trivially passable, but exposing it
  invites cross-session replay, persistence-then-reuse across restarts (tokens
  are process-epoch values, not stable identities), and confusion with the
  orthogonal owner×stream conjunction. Kotlin receives `TorIsolationSession`
  objects; Rust holds the `Arc<TorClient>` + `StreamPrefs` map.
- *SOCKS multi-context?* The current `handle_socks` (`lib.rs:815-902`) is
  CONNECT-only, hardcoded `[0x05,0x00]` no-auth, and calls bare
  `client.connect((host,port))` with default prefs → **one isolation context
  for the whole listener**. Two designs both terminate in the same
  `connect_with_prefs` call (no direct streams required):
  - **A (recommended): per-session SOCKS ports** — each session closes over
    its own `Arc<TorClient>` (`isolated_client()`) or
    `with_prefs(new_isolation_group())`; reuses existing `spawn_socks`/`socks_port`
    machinery; cost is port bookkeeping.
  - **B (deferred): SOCKS username/password auth** — implement RFC 1929 auth in
    `handle_socks` + credential→prefs/client map (the C-Tor `IsolateSOCKSAuth`
    analogue). Upstream anticipated this (`rpc.rs:192-200`: "isolation … from
    SOCKS username/password") but the server-side mapping is **not** in
    `arti-client` — `tor-socksproto` parses `SocksAuth::{NoAuth,Socks4,Username}`
    but no policy mapping exists in the audited crates. B saves ports but needs
    auth negotiation + credential lifecycle + app HTTP-stack support for
    per-request SOCKS credentials (OkHttp: per-client proxy auth; Ktor:
    engine-dependent; URLSession: `kCFStreamPropertySOCKSProxy*` — uneven).
- *Direct streams required?* **No** — both A and B bind each accepted socket to
  the right isolation context inside `handle_socks` via `connect_with_prefs`.
- *Privacy mistakes:* default single-context shares exit circuits across UI
  identities (linkable at exit); `isolate_every_stream` abuse (network load,
  slower, no extra privacy); assuming `isolated_client()` = separate guard
  identity (guards shared by design); assuming restart = unlinkability
  (state/guards persist); `reconfigure` blast radius (one identity's change
  affects all).

**Evaluated sketches:** the task's `TorIsolationSession{id; openStream()}`
implies direct streams — **wrong layer for 0.3**. The simpler
`session = client.createIsolationSession(); session.socksEndpoint` model is
**correct**: each logical session = SOCKS-level isolation over a shared Arti
client, consumable by HTTP/WebSocket/Ktor/OkHttp/URLSession without new byte
paths. `openStream()`-style direct access is deferred to a future experimental
module *if* ever needed.

**Recommendation: ADOPT_0_3** (opaque sessions + per-session SOCKS);
`ADOPT_INTERNAL` (raw `IsolationToken`/`StreamPrefs` stay Rust-side);
**REJECT** (`isolate_every_stream`, raw token over UniFFI).

### 3.2 Direct Tor streams

**APIs inspected:** `TorClient::connect` (`client.rs:1539`) /
`connect_with_prefs` (`client.rs:1550`) → `DataStream`
(`tor-proto-0.46.0/src/client/stream/data.rs:137`, re-exported `lib.rs:75`);
`DataStream: Send+Sync`, both `futures::io` and `tokio::io` AsyncRead/Write
(`data.rs:711-761`); `split() -> (DataReader, DataWriter)` (`data.rs:665`);
stream lives until both halves dropped or closed (`data.rs:119-126`);
**flush required** (cell-buffered writer; `poll_write` queues, `poll_flush`
sends — `data.rs:83-111,969-1000`); **no half-close** ("close means I am done",
`data.rs:238-243`); EOF = `Ok(0)`, other END reasons latch `Closed` with a
documented `NotConnected`-after-error quirk (`data.rs:1081-1103`); backpressure
= `Pending` during cell flush + rate-limited writer; cancellation = drop the
future (no handle); BEGIN wrapped in `connect_timeout` (10s default,
`config.rs:153-166`); `IntoTorAddr` deliberately rejects `SocketAddr`/`IpAddr`
(local-DNS-leak guard, `client.rs:1493-1537`); `.onion` routes to the HS branch
with `optimistic` forcibly cleared (`client.rs:1606-1661`); isolation via prefs.

**FFI implications (UniFFI 0.32 / Ubique 1.2.1):** `DataStream` needs
`Pin<&mut>` borrowing — not directly a UniFFI object; natural mapping is
`split()` into reader/writer objects or a mutex-guarded single object. Either
way every `read(len)->Vec<u8>` pays **≥1 extra copy + allocation per chunk**
(Rust `Vec` → JVM `ByteArray` / `NSData`) versus today's in-Rust
`tokio::io::copy` (`lib.rs:893-900`). Cancellation needs explicit `close()`
plumbing (UniFFI async has no first-class per-read cancel); threading surfaces
completions on tokio workers (Kotlin/Native freeze sensitivity — today's
fire-and-forget listener is far simpler); lifetime needs a stream registry +
abort-on-pause (today's `abort_connections()` covers SOCKS tasks;
leaked Kotlin handles would pin `StreamTarget→ClientCirc` refs past
`shutdown()`); `flush()` discipline must be explicit or writes stall past
`connect_timeout`.

**SOCKS-first vs direct:** SOCKS-first keeps TLS/HTTP in Kotlin stacks, zero new
FFI machinery, TCP backpressure for free; its only deficit (single isolation
context) is fixed by §3.1 without direct streams. Direct gives per-request
isolation precision and no local-TCP hop, at the cost of reimplemented
HTTP/TLS bridging + the full FFI burden above.

**Recommendation: DEFER** public `TorStream`; **ADOPT_INTERNAL**
(`connect_with_prefs` inside `handle_socks` only).

### 3.3 Bridges

**APIs inspected** (`arti-client-0.46.0/src/config.rs`, `tor-guardmgr-0.46.0/src/bridge/`):
`BridgesConfig{enabled: BoolOrAuto (Auto = use iff non-empty), bridges:
BridgeList, transports (pt-client only)}`; builder idiom
`builder.bridges().bridges().push(line.parse()?)`; `BridgeConfigBuilder::
{transport, direct, push_setting, build, FromStr}`; line syntax
`[Bridge] [transport] Host:ORPort fingerprint… [k=v…]` (RSA required, direct
needs numeric IP:port, PT allows hostname/`-`); `enabled=true+[]` rejected;
`BridgeParseError` (~13 variants) + builder `Inconsistent/Invalid/
NoCompileTimeSupport` — ArtiTor already maps both to `Config{bad bridge line}`.
`bridge-client` is **stable** (no `__is_experimental`).

**Semantics:** malformed → `Config` before bootstrap (current behaviour, keep);
reconfiguration is **live-capable** upstream (`TorClient::reconfigure` →
`circmgr` → `guardmgr.replace_bridge_config`, flips `GuardSetSelector`,
`RetireCircuits::All`) — but ArtiTor 0.2 deliberately rebuilds on bridge change
for determinism (keep; see §3.8); guards persisted under `guards` key +
bridge descriptors cached in sqlite + `pt_state` dir; bridge lines are config
(not auto-persisted); bootstrap uses `BridgeSet` universe + `BridgeDescMgr`.
Privacy: bridge addresses in logs/config backups are sensitive (see §9).

**`List<String>` verdict:** keep the UniFFI `bridges: Vec<String>` shape for 0.3
(back-compat), but (a) validate every line in Rust pre-bootstrap (typed
`Config`, fail fast — fixes silent pass-through), and (b) add
`bridgesEnabled: Auto/On/Off` tri-state mapping to `BoolOrAuto` (today's
implicit Auto is fine, but explicit On catches "I set bridges yet nothing
changed" misconfigurations, and Off allows shipping lines while disabled).
A fully typed `Bridge{transport, addr, fingerprint, settings}` record waits
until PT fields force it (see §15).

**Recommendation: ADOPT_0_3** (validation + tri-state; wire shape unchanged).

### 3.4 Pluggable transports

**APIs inspected:** `pt-client = [bridge-client, tor-chanmgr/pt-client,
tor-guardmgr/pt-client, tor-ptmgr]` (`arti-client/Cargo.toml:189-194`);
`TransportConfig{protocols, path (managed), arguments (managed), proxy_addr
(unmanaged), run_on_startup (managed)}` (`tor-ptmgr/config.rs`); validation
(`path`⊕`proxy_addr` exclusive, `arguments`/`run_on_startup` unmanaged-rejected,
bridge↔transport protocol match enforced); `PtMgr::new(transports,
state_dir/pt_state, …)` + `reconfigure` (with `TODO(#2634)`: no prune of
running binaries); managed spawn = `Command::new(path).args().envs(TOR_PT_*).
stdout(piped).stdin(piped).spawn()` + pt-spec v1 stdout parsing
(`VERSION/CMETHOD…/CMETHODS DONE`), `PT_START_TIMEOUT=30s`,
`GRACEFUL_EXIT_TIME=5s`; crash = warn + drop handle, **no auto-respawn**
(next lookup re-spawns on demand); unmanaged = loopback SOCKS5 to caller-provided
endpoint (non-loopback warns); bridge `k=v` encoded into SOCKS handshake.

**Mobile:** no upstream Android/iOS PT claims in audited sources
(`tor-ptmgr` has no `cfg(target_os)` mobile branches). Android managed is
feasible [inference] (`Command::spawn` works; work = per-ABI binaries + exec
permission + `CfgPath` absolutization + `pt_state` storage). **iOS managed is
infeasible** [inference] (sandbox denies fork/exec; `spawn` → `ChildSpawnFailed`;
no App Store path for helper executables). iOS unmanaged needs an in-process or
NetworkExtension sidecar PT — no such crate in scope.

**Binary/deps (measured, `/tmp` probes, committed baseline untouched):**
baseline 565 unique `cargo tree` entries; `+pt-client` → 574 (**+1 crate:
`tor-ptmgr`**; rest already transitive); the dominant cost is the **PT binaries
themselves** (per-ABI MBs + 30s startup budget + `pt_state` I/O), not Rust bloat.
`managed-pts` off would give unmanaged-only `tor-ptmgr` but is not expressible
via `arti-client/pt-client` alone.

**Recommendation:** unmanaged **EXPERIMENTAL** (desktop/Android first, validates
`bridges.transports` plumbing with zero binaries); managed Android/desktop
**ADOPT_INTERNAL** (dogfood, not default — binary bundling + crash UX first);
managed iOS **REJECT**; provider abstraction (`sealed interface
PluggableTransport{Unmanaged{protocol,host,port}}`) **DEFER** until unmanaged
proves out (the task's sketch is directionally right but premature to freeze).

### 3.5 Dormant mode

**APIs inspected** (`arti-client-0.46.0/src/client.rs:638-648,2247-2261,2516-2549`):
`#[non_exhaustive] enum DormantMode{Normal (default), Soft}`; `set_dormant(mode)`
is a fire-and-forget watch send consumed by `tasks_monitor_dormant`, which fans
out to `chanmgr.set_dormancy` (padding off), `bridge_desc_mgr.set_dormancy`
(stale-allowed), and `TaskHandle::{cancel,fire}` over circmgr/dirmgr/chanmgr/hs
periodic tasks (cancel ≠ drop; `fire()` resumes). **Not torn down:** client,
circuits, channels, streams, guards. **Auto-wake:** every use path
(`wait_for_bootstrap_running`) flips `Soft→Normal` first. Wake ≠ bootstrap
(`Manual` + never-bootstrapped still `BootstrapRequired`; expired dir
re-bootstraps normally). Orthogonal `BootstrapBehavior::{OnDemand,Manual}`.

**vs `pause()`:** FFI `pause()` stops SOCKS + aborts handlers, keeps client iff
bootstrapped — but never calls `set_dormant`, so **PAUSED today keeps Arti
background work running** (directory downloads, preemptive circuits, padding).
`pause()` = SOCKS/stream control (fail-closed, fast resume); dormant = power
policy (logically connected, timers stopped, auto-wakes). Folding them together
creates races: a `pause()+dormant(Soft)` concurrent with in-flight `connect` or
`resume()` rebind immediately unwakes; pre-`RunningInner` dormancy only seeds
construction. Kotlin must tolerate a future third `DormantMode` variant.

**Recommendation: EXPERIMENTAL** — separate `goDormant()/wake()` API with race
docs, dogfood internally; **do not change `pause()`** in 0.3.

### 3.6 Onion-service client

**Already works — no new API.** `onion-service-client = [tor-hsclient,
tor-hscrypto]` is stable and compiled in; `TorClient::connect` HS branch
(`client.rs:1606-1661`) + `allow_onion_addrs` default true + `StreamPrefs::
connect_to_onion_services(Auto)` + current CONNECT-only SOCKS handler (passes
`.onion` hostnames verbatim to `connect`, `lib.rs:841-880`) = ordinary public
`.onion` over SOCKS today. Bootstrap implication is only latency (`wait_for_
bootstrap_running` + timely dir). Error kinds are stable-mapped
(`OnionAddressNotSupported→FeatureDisabled`,
`OnionAddressDisabled→ForbiddenStreamTarget`,
`BadOnionAddress→InvalidStreamTarget`, `ObtainHsCircuit→cause.kind()`).
Authenticated/restricted discovery (key insert/rotate/remove/get) is
`experimental-api` + `keymgr` gated (re-exports in `lib.rs:87-91` same gate;
ArtiTor sets no keystore → auto-lookup inert) — keep out of stable 0.3.

**Recommendation: ADOPT_0_3** (document + e2e-test SOCKS `.onion`; zero code);
auth client **EXPERIMENTAL**.

### 3.7 Onion-service hosting

**APIs inspected:** `launch_onion_service(config) ->
Option<(Arc<RunningOnionService>, Stream<RendRequest>)>` (`client.rs:1918`,
stable-gated `onion-service-service` only); `create_onion_service` (offline,
needs `KeyMgr` + `state_dir`); `OnionServiceConfig{nickname, enabled,
num_intro_points(3/20), rate_limit, max_concurrent_streams, enable_pow, …}`;
`RunningOnionService::{reconfigure, status, status_events}` (drop = stop
publish/accept); `RendRequest::{accept→Stream<StreamRequest>, reject}`;
persistence via `state_dir` + `KeyMgr` (`HsIdKeypair` by nickname,
auto-generated). Experimental-gated: `launch_onion_service_with_hsid`,
`hs_circ_pool()`, `wait_for_stop()`, `keymgr()` (`onion-service-cli-extra`),
`restricted-discovery`, `hs-pow-full/equix`, `negotiate-extensions`.
Feature `onion-service-service = [tor-hsservice, tor-hscrypto,
tor-persist/state-dir, keymgr]` — stable name, **heavy**: `+tor-hsservice`
pulls `tor-keymgr, equix, metrics…` (+5 unique crates measured) + writable
`state_dir/keystore` with `fs-mistrust` + new `KeystoreRequired/
LaunchOnionService` failures. **Not** in today's lockfile (`tor-hsservice`
absent — hosting code unlinked).

**Mobile verdict:** "Arti can host" ≠ "an iOS app can reliably provide an
always-available onion service". Hosting needs a live client + service
continuously holding intro circuits + re-publishing descriptors; Android
Doze/background limits and iOS suspension kill it (descriptors go stale);
network transitions force re-publication storms; `DormantMode::Soft` is
incompatible with always-publish; FFI needs a `RendRequest→StreamRequest`
streaming object (order of magnitude beyond CONNECT-only SOCKS); keys need a
mobile keystore story.

**Recommendation: DEFER** (revisit as experimental module with its own
distribution story, not in 0.3).

### 3.8 Vanguards

`vanguards = [tor-guardmgr/vanguards, tor-circmgr/vanguards]` (stable name);
modes `Lite=1 (default) / Full=2 / Disabled=0`; exposed as
`TorClientConfig::vanguards` but honored **only**
`all(vanguards, any(onion-service-client, onion-service-service))`, else forced
`DISABLED_VANGUARDS` (`config.rs:665-695`). Applies to HS client+service
circuits via `HsCircPool`; automatic once on (`Full` opt-in). Measured: **+0 new
crates** (code paths in already-present managers). ArtiTor today: effectively
`Disabled`, no knob wired.

**Recommendation: DEFER** — no public knob in 0.3 (defaults suffice for
client-only use); revisit with hosting or a hardened-client threat model.

### 3.9 Runtime reconfiguration

`TorClient::reconfigure(&TorClientConfig, Reconfigure)` (`client.rs:1397`);
`Reconfigure::{AllOrNothing, WarnOnFailures, CheckAllOrNothing}`
(`tor-config/lib.rs:161`, non-exhaustive); `AllOrNothing` = dry-run then apply
(second-phase failure warns "inconsistent state"); serialized by
`reconfigure_lock`; applies to **all** derived isolated handles; errors →
`InvalidConfigTransition`. **Can change live:** address_filter, stream_timeouts
(new streams only), path_rules/circuit_timing/preemptive (may
`RetireCircuits::All`), bridges/guards/vanguards (may retire all),
channel (Bug-only fallible), dirmgr schedule/tolerance/net-params, PT
transports. **Cannot:** `storage.{cache_dir,state_dir,permissions}`,
`network.authorities`, `path_resolver`, `system` — **plus an explicit upstream
warning that the non-reconfigurable list is incompletely documented
(arti#1721, `client.rs:1384-1392`)**. Pre-bootstrap `reconfigure` just clones
config (only `cache_dir` checked).

**ArtiTor posture (keep):** 0.2 deliberately rebuilds on
`dataDir/stateDir/cacheDir/bridges` change and rebinds on `socksPort` — do not
relax merely because upstream *can* reconfigure. Deterministic lifecycle +
back-compat beat knob coverage. Expose live-reconfigure **only** for the two
safe sections (address filter, stream timeouts) with `CheckAllOrNothing`
dry-run first; everything else continues to force rebuild.

**Recommendation: ADOPT_INTERNAL** (2 whitelisted sections); full surface
**DEFER**.

### 3.10 Errors and diagnostics

Current 7-variant `ArtiException` is coarser than upstream but in the right
place. Upstream stability contract = **`HasKind::kind() -> ErrorKind`**
(`tor-error`, ~60 variants, `#[non_exhaustive]`), re-exported `lib.rs:72`;
`ErrorDetail` + `detail()` are `error_detail`-gated (`__is_experimental`,
voids semver); `Display/Debug/source` explicitly excluded from semver
(`err.rs:28-33`). Notable pinned mappings: `ExitTimeout→RemoteNetworkTimeout`,
`BootstrapRequired` (distinct), `OnionAddress*→FeatureDisabled/
ForbiddenStreamTarget/InvalidStreamTarget`, `KeystoreRequired→InvalidConfig`,
`FsMistrust→FsPermissions`, `Configuration→InvalidConfig`,
`Reconfigure→InvalidConfigTransition`. Diagnostics: `bootstrap_status/events`
+ `BootstrapStatus{as_frac,ready_for_traffic,blocked}` + `BlockageKind`
(non-exhaustive) are **stable** (already the right foundation);
circuit/stream/PT/onion internals are `experimental-api` or `pub(crate)`
(`DirEvent`, `ConnStatus`, `ClockSkew`, `hs_circ_pool()`, …).

**Recommendation: ADOPT_0_3** (additive buckets `Network / ExitFailed /
TargetRejected / Storage / BootstrapRequired-ish` mapped from `kind()` only,
`Unknown` fallback; never `ErrorDetail`); extra event streams **DEFER**.

### 3.11 RPC

`#[cfg(feature="rpc")] pub mod rpc` (`lib.rs:58`); `rpc = [dyn-clone,
dep:tor-rpcbase]`; provides only an **in-process dispatch table**
(`arti:get_client_status`, `watch_client_status`, `new_isolated_client`, plus
SOCKS-coupled `ConnectWithPrefs/Resolve*` that return unserializable
`DataStream`s — "not invoked by an RPC session directly"). No
listener/socket/auth in this crate — that lives in the external `arti` daemon.
Measured: `+rpc` pulls `tor-rpcbase, typetag, erased-serde…` (+8 unique).
Embedded KMP has no out-of-process controller; direct
`bootstrap_status/events` + local SOCKS already cover the use cases.

**Recommendation: REJECT.**

### 3.12 Advanced configuration

`TorClientConfig` sections (`config.rs:549-637`) classified for KMP:

| Section                                               | Live-replaceable?                   | Verdict                                                                  |
|-------------------------------------------------------|-------------------------------------|--------------------------------------------------------------------------|
| `address_filter` (onion/local)                        | Yes (new streams)                   | **Core public** (in 0.3)                                                 |
| `stream_timeouts` (10s defaults)                      | Yes (new streams)                   | **Core public** (in 0.3)                                                 |
| `bridges` (+`transports` when pt-client)              | Yes (may retire all)                | **Advanced public** (lines + enabled tri-state; transports experimental) |
| `download_schedule`, `directory_tolerance`            | Yes                                 | **Advanced** (defer knobs; safe defaults)                                |
| `path_rules`, `preemptive_circuits`, `circuit_timing` | Yes (may retire all)                | **Advanced/internal** (defer; wrong defaults nuke circuits)              |
| `channel`                                             | Yes (Bug-only)                      | **Internal**                                                             |
| `override_net_params`, `path_resolver`                | — / not reconfigurable              | **Internal/reject** (debug/ancillary)                                    |
| `tor_network` (authorities/fallbacks)                 | No (authorities rejected live)      | **Internal**                                                             |
| `storage` (cache/state/permissions)                   | **No**                              | **Reject-live** (restart only; already in `ArtiConfig`)                  |
| `system.memory`                                       | **No**                              | **Internal**                                                             |
| `vanguards`                                           | Yes (may retire all; feature-gated) | **Reject** (no knob in 0.3)                                              |

Rule: opinionated safe defaults; never leak `TorClientConfig`/`TorClientConfigBuilder`
(or `StreamPrefs`, `DataStream`, `IsolationToken`) into Kotlin. `geoip`
(`exit_country`) is `__is_experimental` — **reject**.

---

## 4. Feature / dependency matrix

Baseline (`rust/arti-kmp-ffi/Cargo.toml:16-27`):

```toml
arti-client = { version = "0.46", default-features = false, features = [
    "tokio", "rustls", "compression", "bridge-client",
    "onion-service-client", "static-sqlite",
] }
tor-rtcompat = { version = "0.46", features = ["tokio", "rustls"] }
# rustls 0.23 + ring provider installed explicitly; NO native-tls/openssl.
```

What each proposed capability requires (✓ = already in baseline):

| Proposed capability                                                  | Cargo features needed                                                                             | New crates (measured)                                                                                          |
|----------------------------------------------------------------------|---------------------------------------------------------------------------------------------------|----------------------------------------------------------------------------------------------------------------|
| Isolation sessions (opaque, per-session SOCKS)                       | ✓ *(none)* — `isolated_client`, `with_prefs`, `connect_with_prefs` ungated                        | +0                                                                                                             |
| Ordinary `.onion` client                                             | ✓ `onion-service-client`                                                                          | +0                                                                                                             |
| Bridge validation + enabled tri-state                                | ✓ `bridge-client`                                                                                 | +0                                                                                                             |
| Address filter + stream timeouts                                     | ✓ *(none / onion-client bit)*                                                                     | +0                                                                                                             |
| Richer errors (`ErrorKind`)                                          | ✓ *(none)*                                                                                        | +0                                                                                                             |
| Bootstrap status/events                                              | ✓ *(none)*                                                                                        | +0                                                                                                             |
| Dormant public API                                                   | ✓ *(none)*                                                                                        | +0                                                                                                             |
| Unmanaged PT plumbing                                                | `pt-client`                                                                                       | **+1** (`tor-ptmgr`)                                                                                           |
| Managed PT (Android/desktop)                                         | `pt-client` (+ default `managed-pts`)                                                             | +1 + external PT binaries                                                                                      |
| Onion hosting                                                        | `onion-service-service` (= `tor-hsservice` + `tor-hscrypto` + `tor-persist/state-dir` + `keymgr`) | **+5** (`tor-hsservice`, `growable-bloom-filter`, `k12`, `serde_bytes`, `xxhash-rust`; `sha3` already present) |
| Vanguards knob                                                       | `vanguards`                                                                                       | **+0** (code in present managers)                                                                              |
| Full 0.3 candidate (`pt-client`+`onion-service-service`+`vanguards`) | all three                                                                                         | **+6** (union of above)                                                                                        |
| RPC                                                                  | `rpc` (+ `tor-rpcbase`)                                                                           | **+8** (`tor-rpcbase`, `typetag[-impl]`, `erased-serde`, `typeid`, `futures-await-test[-macro]`, …)            |
| Auth onion client                                                    | `experimental-api` + `keymgr` (+ `onion-service-client`)                                          | keymgr subtree + semver void                                                                                   |
| `error_detail` diagnostics                                           | `error_detail` (`__is_experimental`)                                                              | semver void                                                                                                    |

Dependency proof (read-only; committed baseline untouched):
`cargo tree | grep -iE 'openssl|native-tls|tor-ptmgr|tor-hsservice|tor-rpc|experimental'`
→ **zero hits** on baseline; unique-entry counts
baseline 565 / +pt 574 / +hss 582 / +vg 570(+0 real) / full 578 / +rpc 587.
No `native-tls`/`openssl` in any probe (rustls-only holds).

---

## 5. Binary impact

Controlled comparison from the 0.46 upgrade report (same host/toolchain,
release `opt-level="z"`, `lto=true`): 0.43→0.46 grew Android `.so` **+1.0–1.5%**
(~+115 KB arm64-v8a: 7,921,520→8,036,768) and Apple static archives **+3.2%**
(~+845 KB iosArm64: 26,484,344→27,329,776) — directory/circuit/crypto churn,
no new subsystem (feature lists byte-identical, no OpenSSL/PT/RPC in graph).

Candidate-feature impact (this audit; dependency-level evidence, no committed
changes — exact `.so`/`.a` builds in temp worktrees were judged
disproportionate for an audit when `cargo tree` deltas are this small and the
dominant costs are external):

- `+pt-client`: +1 Rust crate (`tor-ptmgr` + pt-spec IPC parser; deps already
  transitive). **Rust delta: small.** Dominant cost: PT binaries per ABI (MBs
  each) + 30s startup budget + `pt_state` I/O. Verdict: do not ship in the
  default artifact for 0.3.
- `+onion-service-service`: +5 crates + mandatory `keymgr` + `state-dir` +
  `fs-mistrust` failure modes. **Rust delta: moderate; integration cost: large**
  (persistent identity, RendRequest FFI, always-on story). Verdict: defer.
- `+vanguards`: +0 crates. **Rust delta: negligible** — but enabling it changes
  HS-circuit behaviour for all consumers with no 0.3 use case. Verdict: defer.
- Full candidate union: +6 crates. Still small in Rust terms — the argument
  against it is **distribution** (precompiled Maven, §8 in task numbering / §18
  in brief), not bytes: every consumer pays even if unused.
- `ADOPT_0_3` core (isolation + bridges validation + onion-doc + errors +
  address/timeouts): **+0 crates, +0 features** — no measurable binary change
  expected beyond normal code growth (new UniFFI objects + SOCKS fan-out).

SQLite/native-TLS guardrails (unchanged): `libsqlite3-sys 0.37.0` + `rusqlite
0.39.0` via `static-sqlite`; Apple `hide-sqlite3-symbols.sh` still required +
effective (283 globals → 0 post-hook on 0.46); 16 KB page alignment holds for
64-bit ABIs; consumer R8/JNA rules unchanged.

---

## 6. Public API proposal

Design rules: preserve `ArtiTorClient`/`ArtiConfig`/`TorStatus`/`ArtiException`/
`TorState` names and source compatibility; additive changes only; generated
UniFFI types stay internal (`com.yet.tor.ffi.*` never in signatures); no
platform types in `commonMain`; `commonMain` depends only on
`kotlinx-coroutines-core`.

```kotlin
package com.yet.tor

// --- Config (additive) -------------------------------------------------------
enum class BridgesEnabled { AUTO, ON, OFF }  // maps to BoolOrAuto; default AUTO

data class ArtiConfig(
    val dataDir: String,
    val socksPort: Int = 0,
    val bridges: List<String> = emptyList(),       // UNCHANGED wire shape
    val bridgesEnabled: BridgesEnabled = BridgesEnabled.AUTO, // NEW (default = old behaviour)
    val stateDir: String? = null,
    val cacheDir: String? = null,
    val allowOnionAddrs: Boolean = true,           // NEW (default = Arti default)
    val allowLocalAddrs: Boolean = false,          // NEW (default = Arti default)
    val connectTimeout: Duration = 10.seconds,     // NEW (Arti default)
    val resolveTimeout: Duration = 10.seconds,     // NEW
)

data class TorSocksEndpoint(val host: String, val port: Int) // 127.0.0.1:port

// --- Sessions (new; the 0.3 centrepiece) ------------------------------------
/**
 * Opaque isolation session. Shares one Arti TorClient (runtime, directory,
 * guards) with all sibling sessions; circuits are never shared across sessions.
 * Each session has its own SOCKS endpoint. Thread-safe; usable from any coroutine.
 * Lifetime: explicit close(); sessions do NOT survive shutdown(); pause/resume
 * preserves session identity (same circuits partition) — see §8.
 */
interface TorIsolationSession : AutoCloseable {
    val id: String            // random UUID string (NOT the IsolationToken value)
    val socksEndpoint: TorSocksEndpoint
    val isClosed: Boolean
    override fun close()      // stops its SOCKS listener; idempotent
}

val ArtiTorClient.defaultSession: TorIsolationSession  // the pre-0.3 endpoint, now named
// (TorStatus.socksPort remains as the default session's port for back-compat.)

// --- Client additions (additive) ---------------------------------------------
class ArtiTorClient {
    // existing: version, status, logs, isReady, hasClient,
    //           start/pause/resume/shutdown/stop/restart — UNCHANGED signatures
    //           (start gains no new params; new config fields flow through ArtiConfig)

    /** Create a new isolation session (up to a documented small-N limit, e.g. 8). */
    suspend fun createIsolationSession(timeout: Duration = 30.seconds): Result<TorIsolationSession>

    /** Snapshot of live sessions (default + created). */
    val sessions: List<TorIsolationSession>

    /** Close one session (== session.close()). */
    fun closeSession(session: TorIsolationSession)
    /** Dormant-mode — EXPERIMENTAL, not stable 0.3 (listed for completeness). */
    // fun goDormant() / fun wake()  — DO NOT SHIP in stable 0.3
}

// --- Errors (additive; existing 7 variants UNCHANGED) -------------------------
sealed class ArtiException {
    // ... AlreadyRunning, NotRunning, Config, Bind, Bootstrap, Timeout, Runtime ...
    class Network(msg: String) :
        ArtiException(msg)        // NEW: TorAccess/directory/circuit collapse
    class ExitFailed(msg: String) : ArtiException(msg)     // NEW: exit/refused/not-found/timeout
    class TargetRejected(msg: String) : ArtiException(msg) // NEW: Invalid/ForbiddenStreamTarget
    class Storage(msg: String) :
        ArtiException(msg)        // NEW: state/cache/keystore/FsPermissions
    class BootstrapRequired(msg: String) :
        ArtiException(msg) // NEW: use-before-bootstrap (distinct from failure)
}
```

Per-type contract:

| Type                       | Responsibility                                                    | Lifecycle / ownership                                                   | Threading                              | UniFFI repr.                                             | Errors                                        | Stability                       |
|----------------------------|-------------------------------------------------------------------|-------------------------------------------------------------------------|----------------------------------------|----------------------------------------------------------|-----------------------------------------------|---------------------------------|
| `ArtiTorClient` (extended) | engine + session registry                                         | single per process (log-sink note kept)                                 | mutex-serialised lifecycle (unchanged) | internal `ArtiTor` + N `SocksHandle`s                    | existing + 5 new buckets                      | **stable 0.3**                  |
| `TorIsolationSession`      | opaque circuit partition + SOCKS endpoint                         | `close()` stops listener; dead on `shutdown()`; survives `pause/resume` | safe from any coroutine                | internal object holding `Arc<TorClient>` + port          | `Bind` on create; `NotRunning` if client gone | **stable 0.3**                  |
| `BridgesEnabled`           | explicit bridge enablement                                        | value in `ArtiConfig`                                                   | —                                      | internal `BoolOrAuto` map                                | `Config` (ON+empty)                           | **stable 0.3**                  |
| `TorSocksEndpoint`         | typed endpoint (replaces raw `socksPort: Int?` at new call sites) | value                                                                   | —                                      | two scalars                                              | —                                             | **stable 0.3**                  |
| New `ArtiException` ×5     | stable `ErrorKind` buckets                                        | values                                                                  | —                                      | `ErrorKind`-string + bucket enum (with Unknown fallback) | —                                             | **stable 0.3**                  |
| `TorStream` (direct)       | raw Tor TCP                                                       | registry + explicit close                                               | IO handoff                             | reader/writer objects                                    | stream-mapped                                 | **future (deferred)**           |
| `OnionService` (hosting)   | accept RendRequests                                               | always-on (undeclared on mobile)                                        | —                                      | stream objects                                           | launch/keystore                               | **future (deferred)**           |
| `PluggableTransport`       | PT endpoint/config                                                | app-supplied paths                                                      | —                                      | record                                                   | PT errors                                     | **future (experimental first)** |

What is deliberately *not* proposed: renaming `ArtiTorClient→ArtiClient`
(aesthetics only — rejected); `openStream(host,port): TorStream` on the session
(deferred with direct streams); username/password SOCKS auth in 0.3 (deferred
alternative to per-session ports); `TorEvent` bus beyond `status`+`logs`
(deferred — bootstrap events suffice); `Bridge{…}` typed record (deferred until
PT forces it); `OnionService`, `TorEndpoint` beyond `TorSocksEndpoint` (future).

---

## 7. Rust / UniFFI architecture proposal

Native facade deltas (all inside `rust/arti-kmp-ffi`, same crate, same
`uniffi.toml` package `com.yet.tor.ffi`):

```rust
// New UniFFI surface (sketch — proposal only, not implemented):
#[derive(uniffi::Object)]
pub struct SocksSession { /* Arc<TorClient> handle + bound port + shutdown tx */ }

#[uniffi::export]
impl ArtiTor {
    // existing: new/version/has_client/is_ready/socks_port/start/resume/pause/shutdown/stop
    pub fn create_session(&self, listener: Box<dyn StatusListener>) -> Result<Arc<SocksSession>, ArtiError>;
    pub fn close_session(&self, session: &SocksSession);
    pub fn session_socks_port(&self, session: &SocksSession) -> Option<u16>;
    // config: ArtiConfig gains bridges_enabled: BridgesEnabledFfi,
    //   allow_onion_addrs: bool, allow_local_addrs: bool,
    //   connect_timeout_secs: u64, resolve_timeout_secs: u64
    // errors: ArtiError gains Network{msg}, ExitFailed{msg}, TargetRejected{msg},
    //   Storage{msg}, BootstrapRequired{msg} (+ ErrorKind mirror in ArtiErrorDetail)
}
#[uniffi::export]
impl SocksSession { pub fn socks_port(&self) -> u16; pub fn close(&self); pub fn id(&self) -> String; }
```

Internals:

- `Shared` gains a session table: `sessions: Mutex<HashMap<SessionId,
  SessionState>>` where `SessionState{client: Arc<TorClient<PreferredRuntime>>,
  prefs: StreamPrefs (fresh isolation group), socks_task: JoinHandle,
  shutdown_tx, bound_port}`. Default session = today's path (root client +
  default prefs) so back-compat is structural, not special-cased.
- `cold_start` builds `TorClientConfig` from the extended `ArtiConfig`
  (`bridges.enabled`, `address_filter`, `stream_timeouts`) and validates every
  bridge line via `BridgeConfigBuilder` pre-bootstrap (fail fast with typed
  `Config`).
- `handle_socks(stream, client, prefs)` — today's `handle_socks` gains a prefs
  parameter and calls `client.connect_with_prefs((host,port), prefs)`; each
  session's accept loop closes over its own `(client, prefs)`. No new byte
  path; `tokio::io::copy` stays in Rust.
- `notify_error` maps `arti_client::Error::kind()` (stable `ErrorKind`) to the
  extended `ArtiError` buckets; the `ErrorKind` mirror discriminant travels in
  `ArtiErrorDetail` (never `ErrorDetail`, never message parsing).
- Pause/shutdown semantics: `pause()` signals **all** session listeners +
  aborts all tracked connections (fail-closed, unchanged blast radius, now
  across N listeners); retains clients. `shutdown()` drops all sessions +
  clients + runtime (all session handles become `NotRunning`). Per-session
  `close()` stops one listener + aborts its tracked connections only.
- Dormant wiring (internal in 0.3): a `set_dormant_soft(bool)` FFI entry used
  only by dogfood builds; public `goDormant/wake` deferred with race docs.
- `reconfigure` (internal in 0.3): Rust helper applying only
  `address_filter`/`stream_timeouts` via `TorClient::reconfigure(...,
  CheckAllOrNothing)` dry-run → `AllOrNothing`; exposed to Kotlin only through
  `start()` with changed-whitelisted-fields (rebuild path unchanged otherwise).
- Single-engine-per-process retained: one `ArtiTor` → one root `TorClient` →
  N `isolated_client()` handles. `LOG_SINK` last-writer-wins note unchanged.

---

## 8. Lifecycle interactions

Current states (`OFF STARTING BOOTSTRAPPING RUNNING PAUSED STOPPING ERROR`)
are **unchanged**. 0.3 adds a session dimension orthogonal to the engine state:

```text
Engine (existing, unchanged transitions):
OFF --start--> STARTING --bootstrap_events--> BOOTSTRAPPING --(100% + default SOCKS bound)--> RUNNING
RUNNING --pause--> PAUSED (client kept, % stays 100, all session SOCKS down)
PAUSED --resume/start--> RUNNING (no re-bootstrap)
any --shutdown--> STOPPING --> OFF (all sessions dead, runtime released)
any --error--> ERROR (typed lastError; sessions frozen, SOCKS down)
pause during STARTING/BOOTSTRAPPING --> OFF (unchanged)

Sessions (new, per-session, only meaningful when engine has a client):
  createIsolationSession() while RUNNING/PAUSED --> session RUNNING (own port) within one engine state
  session.close() --> session CLOSED (its port freed; engine state unchanged unless it was the last/only session —
        engine RUNNING requires >=1 bound session; closing the last session keeps engine PAUSED-with-client, NOT OFF)
  engine pause() --> all sessions' SOCKS down (sessions retained, identities preserved)
  engine shutdown()/ERROR --> all sessions invalid (NotRunning on use)
```

Rules:

- `isReady` definition unchanged (default session ready). New:
  `session.isUsable ⇔ engine hasClient && session !closed && its port bound`.
- `socksPort = 0` unchanged (each session may bind ephemeral; each reports its
  actual port). Fixed-port collision on session create → typed `Bind{port}` on
  that call only (engine state untouched).
- Config identity unchanged and engine-scoped: `dataDir/stateDir/cacheDir/
  bridges(+enabled)` change → full rebuild (all sessions die, callers
  recreate); `socksPort` → default-session rebind only (sessions keep ports
  unless colliding); address/timeout change → internal live-reconfigure, no
  state transition.
- Dormant (experimental) would add no new engine state — a power overlay:
  `goDormant()` legal in RUNNING/PAUSED (timers stop, auto-wake on use);
  illegal in STARTING/BOOTSTRAPPING (no-op + log) to avoid bootstrap races.
- Hosting (deferred) would need `RUNNING_WITH_SERVICES`-style sub-state and is
  one reason it is deferred — the current 7-state machine cannot honestly
  represent publish/accept health.
- Invariants extended: PAUSED requires client + 100 + **zero** bound session
  ports; RUNNING requires client + 100 + **≥1** bound session port; ERROR
  carries typed `lastError` + zero ports (all sessions); OFF holds no
  client/sessions/ports.

---

## 9. Security / privacy considerations

For every proposed capability, the footgun and the safe-default response:

1. **Cross-identity circuit sharing (default today).** Footgun: two accounts in
   one app exit via the same circuit (linkable). Response: 0.3 makes the safe
   thing easy — one session per app-level identity (documented 1:1 mapping +
   sample `TorController`), distinct SOCKS endpoints so HTTP stacks cannot
   accidentally pool across identities. Never pool `OkHttpClient` connection
   pools across sessions.
2. **`isolate_every_stream` temptation.** Upstream warns explicitly (perf +
   network load, no extra privacy). Response: not exposed; docs state one
   isolation group per identity, not per request.
3. **Guard/state correlation across `isolated_client()`s and restarts.**
   Shared guardmgr/dirmgr/state + pause/resume token survival mean sessions are
   circuit-partitioned, not network-unlinkable. Response: document honestly;
   never promise "new identity"; apps needing rotation must cold-restart *and*
   rotate state dirs (with the UX cost stated).
4. **Bridge/PT secrets in logs.** Bridge lines contain fingerprints/endpoints;
   PT args may contain secrets. Response: Rust logs redact bridge material
   (follow the `sensitive()` pattern at `client.rs:1591`); Kotlin `logs` flow
   is debug-only; never persist `ArtiConfig` with bridges to unprotected prefs
   (document; app-owned storage).
5. **Onion keys.** Deferred with hosting/auth — but pre-commit: `HsIdKeypair`
   must live in the platform keystore (Android Keystore / iOS Keychain), never
   in plain files or logs; ephemeral-only option for throwaway identities.
6. **Clearnet fallback.** ArtiTor is fail-closed (streams terminate on OFF;
   SOCKS down = no traffic, not direct). Response: keep; document that apps
   must not implement "Tor failed → retry direct" and must rebuild HTTP clients
   on `status` transitions rather than caching a direct fallback.
7. **Stale-stream use after Tor OFF.** `pause/shutdown/ERROR` aborts tracked
   connections; sessions add per-session tracking so `close()` is equally
   fail-closed. HTTP clients must observe `status`/session usability and stop
   issuing requests (app responsibility, documented).
8. **Reconnect correlation.** Pause/resume preserves guards/circuits by design
   (fast toggle); apps must not treat resume as a fresh identity signal.
9. **Experimental API leakage.** `error_detail`, `experimental-api`, `geoip`,
   `restricted-discovery` void semver or enable fingerprintable behaviours
   (exit-country pinning). Response: none cross the FFI boundary; exhaustive
   Kotlin `when`s always have `else`.
10. **SOCKS surface expansion.** N listeners = N loopback ports (127.0.0.1
    only, never 0.0.0.0); cap session count (e.g. 8) to bound port/circuit
    pressure; document that localhost-only binding is a security property, not
    an implementation detail.

---

## 10. Android limitations

- **Sessions/SOCKS:** no constraint — loopback listeners + per-session ports
  work under the existing `minSdk 26`, 16 KB-page-aligned `.so`s, JNA/UniFFI
  wiring, and consumer R8 rules. Session count cap also bounds FD/circuit
  pressure under Doze-throttled radios.
- **Dormant:** `set_dormant(Soft)` helps (padding/downloads/preemptive off,
  auto-wake on use) but does not survive Doze: the OS may still suspend the
  process; wake is then a normal re-bootstrap path, not an Arti guarantee.
  Keep dormant as power *hint*, not lifecycle promise.
- **Unmanaged PT:** feasible — bundle per-ABI PT binaries as app assets,
  extract with exec permission, point `proxy_addr` at the app-started loopback
  PT. Risks are integration, not Arti: scoped-storage exec variance, per-device
  background kills of the PT process, Play Store review surface for bundled
  censorship-circumvention binaries.
- **Managed PT:** feasible where `exec` allowed (same binary story, Arti spawns
  via `Command`); needs `pt_state` under app storage + crash/restart UX (no
  auto-respawn upstream — next lookup re-spawns). Dogfood before public.
- **Onion hosting:** no OS primitive blocks it, but Doze/background limits make
  always-publish unreliable; descriptor staleness + re-publication storms on
  network transitions; battery cost of intro keepalives + optional PoW/Full
  vanguards. Not a 0.3 promise.
- **Testing:** `:tor:connectedAndroidDeviceTest` exists and compiles
  (`assembleAndroidDeviceTest` green) but has **never run live** (no
  adb/device in CI environments to date) — same standing follow-up as 0.2/0.46
  reports. 0.3 must run it on hardware (cold start, pause/resume, sessions,
  `.onion`, bridges if credentials available).

## 11. iOS limitations

- **Sessions/SOCKS:** no constraint beyond today's KMP/UniFFI path —
  `iosArm64`/`iosSimulatorArm64` static archives, SQLite symbol isolation
  (283→0 globals, hook preserved), `URLSession` SOCKS reconfiguration per
  session endpoint (app-owned `TorURLSession` rebuild on port change, as today).
  Kotlin/Native strict memory-model makes stateful stream objects delicate —
  one more reason direct streams are deferred while listener-style sessions are
  fine.
- **Background/suspension:** iOS may suspend the app at any time; ArtiTor
  cannot guarantee publish/accept/connected behaviour while suspended. `pause()`
  on background (app policy) + `resume()`/`start()` on foreground remains the
  correct pattern; dormant `Soft` does not substitute for it. Document app
  responsibility; long-lived sessions/streams across suspension are
  best-effort, not guaranteed.
- **Managed PT: infeasible.** Sandbox forbids spawning bundled executables
  (`fork`/`exec` denied → `ChildSpawnFailed`); no App Store path for helper
  PT binaries. **Do not roadmap managed PT on iOS.**
- **Unmanaged PT:** possible only with an in-process or NetworkExtension
  sidecar PT already listening on loopback — neither exists in scope. Deferred
  to a dedicated PT-distribution design, not 0.3.
- **Onion hosting:** additionally unreliable for the same suspension reasons
  plus descriptor availability while not running. Deferred.
- **Testing:** live E2E verified on iOS simulator for 0.2 and 0.46 (cold
  start→100%→SOCKS HTTP 200→pause→resume→2nd 200→shutdown→2nd cold→3rd 200).
  0.3 must extend that script with: session create (distinct ports),
  session-close, `.onion` fetch, bridge-config validation failure (bad line →
  `Config`), dormant dogfood (internal), and error-bucket assertions. Device
  (non-simulator) run remains unverified — schedule before release.

---

## 12. Test strategy

Every `ADOPT_0_3` candidate must be meaningfully verifiable on supported
platforms; otherwise it is not stable-0.3 material.

- **Isolation sessions (the hard one — do NOT assert different exit IPs).**
  Tor may legitimately choose the same exit for two identities; exit-IP
  comparison is flaky by design. Deterministic approaches (in priority order):
  1. *Rust unit/integration (no network):* build two `isolated_client()`s (or
     two `new_isolation_group()` prefs) over an unbootstrapped client and
     assert via `circmgr` internals that `isolation()` differs
     (owner_token/stream_isolation mismatch → `compatible()==false`), i.e. the
     sharing rule itself; plus `set_isolation(same)` shares and
     `isolated_client` never shares even with equal stream tokens.
  2. *Rust SOCKS-level (no Tor network):* bind two session listeners on
     loopback with distinct prefs; assert each accepted socket is dispatched
     with its session's prefs/client (mock `connect_with_prefs` target or
     record prefs identity per connection).
  3. *Live (attended, heuristic only):* N sessions × M fetches through real Tor;
     assert all succeed and each session's endpoint is distinct; treat
     circuit-level non-sharing as covered by (1), not by exit diversity.
  4. *Kotlin facade fakes:* session registry (create/close/list, cap
     enforcement, close-idempotent, shutdown-invalidates) with a fake FFI —
     deterministic, no network. Plus `checkLifecycleInvariants` extensions for
     session-port rules.
- **Bridges.** Rust: table of good/bad lines (direct, PT-shaped without
  `pt-client` → `NoCompileTimeSupport`, empty, garbage) → typed `Config` with
  zero network. Live (optional, secret-safe): real bridge bootstrap only with
  CI-injected credentials (never committed); otherwise `enabled=ON + []` →
  `Config` and `enabled=OFF + lines` → ignores lines (no network). Kotlin:
  `BridgesEnabled` mapping + back-compat (`bridges` shape unchanged).
- **Unmanaged PT (experimental only).** Local mock PT: a loopback SOCKS5 stub
  acting as the "transport" + `proxy_addr` config → bridge fetch through the
  mock (proves `transports` plumbing + `k=v` handshake encoding). Real PT
  integration deferred with managed distribution.
- **Dormant (experimental/dogfood).** Assert `set_dormant(Soft)` stops
  padding/downloads/preemptive tasks (scheduler/task-handle observation in Rust
  tests) and that a subsequent `connect` auto-wakes (`Normal`); Kotlin asserts
  no state-machine transition on dormant toggle (overlay, not state) and no
  stranded waiters when `pause()` races dormant.
- **Onion client (no-op).** Live: public `.onion` fetch via SOCKS (e.g. a stable
  test onion address) on simulator + Android hardware; negative: invalid
  `.onion` → `TargetRejected`, `.onion` with `allowOnionAddrs=false` →
  `TargetRejected`, all without new API. Rust: address-filter unit tests.
- **Errors.** Table-driven: each upstream `ErrorKind` representative → expected
  Kotlin bucket (including `Unknown` fallback for a synthetic future variant);
  adversarial-message tests (kind-not-text, as in 0.2 `ErrorMappingTest`).
- **Address/timeout knobs.** Rust: `allow_local_addrs=false` + local target →
  `LocalAddress`; timeout override shortens a black-holed BEGIN (mock); live:
  defaults unchanged (10s) unless app overrides.
- **Sessions on device.** Android hardware + iOS simulator: create 2 sessions
  (distinct ports), fetch via each, close one (other unaffected), pause (both
  down), resume (both back without re-bootstrap — wall clock ≪ cold start),
  shutdown (both dead). This is the 0.3 E2E gate.
- **What is NOT tested in 0.3:** hosting (no service→descriptor→remote-connect
  test — deferred with the feature), managed PT on iOS (untestable — rejected),
  RPC (rejected), exhaustive advanced-config matrix (rejected by design).

---

## 13. Versioning / source compatibility

- 0.3.0 is **minor, additive**: `ArtiConfig` gains defaulted fields
  (`bridgesEnabled`, `allowOnionAddrs`, `allowLocalAddrs`, timeouts) —
  existing call sites compile unchanged; `ArtiTorClient` gains methods, loses
  none; `ArtiException` gains subclasses (existing `catch` clauses keep
  working; exhaustive `when` over the sealed class in *app* code would break —
  mitigate with a `@SinceKotlin`-style doc note and a non-sealed fallback
  discussion: keep `sealed` + document that new subclasses are a minor-version
  possibility, matching upstream's own `#[non_exhaustive]` posture).
- FFI enums (`TorState`, `ErrorKind` mirror) must tolerate unknown variants
  across the boundary (UniFFI records/enums are versioned independently of
  Kotlin) — same rule as upstream `non_exhaustive`.
- Config-identity rebuild semantics are a **compatibility promise**: anything
  that rebuilt in 0.2 still rebuilds in 0.3 (new fields are either
  whitelisted-live (address/timeouts) or rebuild-triggering (bridges+enabled)).
- `TorStatus.socksPort` remains the *default* session's port (back-compat for
  single-session apps); multi-session apps use `session.socksEndpoint`.
- No `ArtiTorClient` rename, no package move (`com.yet.tor` + internal
  `com.yet.tor.ffi`), no `commonMain` dependency beyond coroutines.
- 0.3.0 ships **one** Maven artifact (`io.github.yet300:tor:0.3.0`) with the
  **unchanged** Cargo feature set (see §4) — no flavor matrix in 0.3.

---

## 14. Implementation phases

Derived from dependency order (foundation first, riskiest-distribution last),
not from the brief's example ordering. Each phase is independently auditable,
lands with tests + docs, and leaves `main` releasable.

```text
0.3 Phase 1 — session/isolation foundation (no new Cargo features)
  Rust: session table, per-session SOCKS listeners, handle_socks(prefs),
        connect_with_prefs dispatch, per-session tracking + abort.
  Kotlin: TorIsolationSession, create/close/list, defaultSession, invariants.
  Tests: Rust isolation-rule units + SOCKS-dispatch tests; Kotlin fake-registry
         tests; live 2-session E2E (simulator).
  Audit gate: §3.1 semantics + §9 footguns documented.

0.3 Phase 2 — bridges validation + enabled tri-state (no new Cargo features)
  Rust: pre-bootstrap BridgeConfigBuilder validation, BoolOrAuto mapping,
        typed Config errors, redacted logging.
  Kotlin: BridgesEnabled enum, ArtiConfig defaults, back-compat tests.
  Tests: good/bad line tables; ON+empty/OFF+lines matrix.

0.3 Phase 3 — errors + address/timeout knobs (no new Cargo features)
  Rust: ErrorKind→bucket mapping in notify_error; address_filter +
        stream_timeouts application + whitelisted live-reconfigure helper.
  Kotlin: 5 new ArtiException subclasses, mapping tests, ArtiConfig fields.
  Tests: kind-table incl. Unknown fallback; adversarial-message tests.

0.3 Phase 4 — onion-client proof + dormant dogfood (internal)
  Docs/tests: .onion-over-SOCKS E2E (no API); internal set_dormant wiring
        behind a non-public flag; race notes.
  Tests: onion positive/negative matrix; dormant stop/wake assertions.

0.3 Phase 5 — release hardening
  Android hardware E2E (sessions, onion, bridges-negative, pause/resume timing);
        iOS simulator full script + device run if available; binary-size check
        (expect ~0 feature-driven growth); docs (README, samples TorController).
  Gate: all §12 ADOPT_0_3 rows green; §13 compat notes published.
```

Explicitly **not** in any 0.3 phase: unmanaged-PT public API, managed-PT
distribution, hosting, vanguards knob, direct streams, SOCKS auth, RPC,
full-reconfigure surface, module split. Those are §15.

---

## 15. Deferred capabilities

What should NOT enter 0.3 (with where it goes):

- **Onion-service hosting** → later release / separate experimental module
  (`:tor-onion-service` or `tor-hs` feature artifact). Needs `tor-hsservice`+
  `keymgr` linkage, RendRequest FFI, keystore UX, always-on story (§3.7).
- **Authenticated onion client** → experimental module (needs `keymgr` +
  `experimental-api`, key-material FFI).
- **Managed PT distribution** → later release, Android/desktop only
  (`:tor-pt` artifact + app-side binary delivery); iOS managed stays REJECTED.
- **Unmanaged PT public API** → experimental flag post-0.3 (after mock-PT
  integration proves the plumbing).
- **Vanguards knob** → later (with hosting or hardened-client model).
- **Public direct streams (`TorStream`)** → later experimental module (after
  copy-overhead measurement + cancellation/lifetime proof).
- **SOCKS username/password multiplexing** → later alternative to per-session
  ports (needs auth negotiation + credential lifecycle + HTTP-stack survey).
- **Full `reconfigure` surface / advanced config passthrough** → never in full
  (opinionated subsets only, per §3.12).
- **RPC** → never (embedded KMP has no controller; extra attack surface).
- **`isolate_every_stream`, raw tokens/prefs/streams/config over FFI,
  `geoip` exit pinning, `error_detail`** → never (abuse / semver / stability).

---

## 16. Open questions

Only real unresolved upstream/product questions (no filler):

1. **Session cap & port strategy:** what small-N cap (proposed 8) and port
   allocation (all-ephemeral vs app-pinned default) best fit BitChat's actual
   identity count? Needs product input, not upstream research.
2. **Upstream `reconfigure` documentation gap (arti#1721):** the
   non-reconfigurable list is explicitly incomplete — do we need an upstream
   issue/MR reference pin before whitelisting even address/timeouts live, or is
   `CheckAllOrNothing` dry-run + rebuild-fallback sufficient? (Audit leans
   sufficient, but the 0.3 implementer must re-check against the 0.3-locked
   arti-client sources.)
3. **Arti 2.6 → next generation drift:** this audit pins 0.46.0; if upstream
   stabilises any `experimental-api` used here as EXPERIMENTAL (dormant
   excluded — already stable; onion-auth, `hs_circ_pool`, `wait_for_stop`) in a
   later 0.4x, the EXPERIMENTAL→ADOPT promotion needs a re-audit, not an
   assumption.
4. **PT binary provenance for Android:** if/when managed PT leaves dogfood, who
   builds/signs/updates per-ABI `obfs4proxy`/`snowflake-client` and how are
   Play Store / reproducible-build requirements met? Out of scope for 0.3 but
   the blocking question for any PT milestone.
5. **Onion-hosting keystore:** platform-keystore (Keystore/Keychain) vs Arti
   native file keystore vs ephemeral — undecided until hosting is scheduled;
   do not pre-commit in 0.3 types.

---

## 17. Final recommendation — PROPOSED 0.3.0 SCOPE

```text
PROPOSED 0.3.0 SCOPE

ADOPT_0_3:
- Isolation sessions: opaque TorIsolationSession handles, one TorClient shared,
  per-session SOCKS endpoints (per-session ports), Root+groups via
  isolated_client()/new_isolation_group(); defaultSession = back-compat endpoint.
- Bridges: keep List<String> wire shape; Rust-side BridgeConfigBuilder
  validation (typed Config on malformed lines) + BridgesEnabled AUTO/ON/OFF
  tri-state; rebuild-on-change semantics unchanged.
- Ordinary .onion client: no new API; document + e2e-test SOCKS .onion
  (stable onion-service-client, allow_onion_addrs default true).
- Richer errors: +5 additive ArtiException buckets (Network, ExitFailed,
  TargetRejected, Storage, BootstrapRequired) mapped from stable ErrorKind only,
  Unknown/Runtime fallback; existing 7 variants unchanged.
- Core config knobs: allowOnionAddrs, allowLocalAddrs, connect/resolve timeouts
  (Arti defaults preserved); whitelisted live-reconfigure internally, rebuild
  otherwise.
- Bootstrap status/events foundation as-is (no new event bus).

ADOPT_INTERNAL:
- connect_with_prefs dispatch inside handle_socks (per-session isolation binding).
- TorClient::reconfigure limited to address_filter + stream_timeouts
  (CheckAllOrNothing dry-run first).
- Dormant-mode wiring (set_dormant) behind a non-public dogfood flag.
- with_prefs / raw IsolationToken / StreamPrefs stay Rust-side only.

EXPERIMENTAL:
- Dormant public API (goDormant/wake as power overlay, NOT folded into pause()).
- Unmanaged PT plumbing (proxy_addr loopback transports; desktop/Android first).
- Authenticated onion-client keys (experimental-api + keymgr; needs keystore story).

DEFER:
- Onion-service hosting (launch/create/RendRequest; needs tor-hsservice + keymgr
  + state-dir + always-on story + own module/artifact).
- Vanguards knob (defaults suffice for client-only 0.3).
- Public direct TorStream API (needs overhead + cancellation + lifetime proof).
- SOCKS username/password auth multiplexing (alternative to per-session ports).
- Full reconfigure surface; typed Bridge record; TorEvent bus beyond status/logs.
- Managed PT distribution (Android/desktop; after binary provenance + crash UX).

REJECT:
- RPC (tor-rpcbase; out-of-process control plane, no embedded use case).
- Managed PT on iOS (sandbox forbids child processes).
- isolate_every_stream public exposure (network-abuse + no-privacy-gain warning).
- Raw IsolationToken / StreamPrefs / DataStream / TorClientConfig over UniFFI.
- error_detail / experimental-api leakage into stable API; geoip exit pinning.
- ArtiTorClient rename; full advanced-config passthrough; BitChat-specific policy
  in the library.
```

**Single-artifact 0.3:** ship `io.github.yet300:tor:0.3.0` with the **unchanged**
0.46 feature set (`tokio, rustls, compression, bridge-client,
onion-service-client, static-sqlite`). All `ADOPT_0_3` items need zero new Cargo
features (proven by §4's matrix); PT/hosting/vanguards stay out precisely
because Maven consumers cannot opt out of already-compiled native features
(Strategy A for the lean core now; Strategies B/C — `:tor-pt` / `:tor-hs`
artifacts — only when those capabilities graduate from DEFER/EXPERIMENTAL).

---

## Appendix A — Sources & traceability

- Upstream crate sources (local registry, exact 0.46.0):
  `arti-client/{src/{client,config,err,lib,rpc,status,address,builder}.rs,
  Cargo.toml[.orig]}`; `tor-circmgr/src/isolation.rs`;
  `tor-proto/src/client/stream/data.rs`; `tor-config/src/{lib.rs,err.rs}`;
  `tor-error/src/lib.rs`; `tor-guardmgr/src/{lib.rs,config.rs,bridge/config.rs,
  vanguards.rs}`; `tor-dirmgr/src/{lib.rs,bridgedesc.rs,storage/sqlite.rs}`;
  `tor-chanmgr/src/{lib.rs,factory.rs}`; docs.rs `tor-ptmgr@0.46.0`,
  `tor-hsservice@0.46.0` (not in lockfile — fetched as source views).
- Local baseline: `rust/arti-kmp-ffi/{src/lib.rs,Cargo.toml,Cargo.lock,
  uniffi.toml}`; `tor/build.gradle.kts` (Ubique 1.2.1, `com.yet.tor.ffi`,
  Apple sqlite hook, R8/JNA rules); `gradle/libs.versions.toml`;
  `tor/src/commonMain/.../ArtiTorClient.kt`.
- Measurement: read-only `cargo tree` probes in `/tmp/featprobe*` (baseline
  565 unique → +pt 574(+1 crate) / +hss 582(+5) / +vg 570(+0) / full 578(+6) /
  +rpc 587(+8)); committed baseline untouched; exact `.so`/`.a` rebuilds
  deferred as disproportionate for an audit (dominant PT/hosting costs are
  external binaries and integration, quantified in §5).
- Prior reports: 0.43→0.46 binary table (+1.0–1.5% Android `.so`, +3.2% Apple
  `.a`), SQLite 283→0 globals, 16 KB alignment, 14 Rust + 45 simulator tests,
  live iOS E2E — all cited, none re-claimed as new work.

## Appendix B — Classification definitions (as applied)

- **ADOPT_0_3:** stable upstream (no `__is_experimental`), mobile-feasible on
  both OSes (or the single exception-free path), architecturally appropriate
  (thin engine), UniFFI risk Low, testable on supported platforms in 0.3 scope.
- **ADOPT_INTERNAL:** same stability, but premature or risky as public API;
  valuable as Rust/Kotlin implementation detail now.
- **EXPERIMENTAL:** depends on `__is_experimental`-gated APIs or stable-but-
  operationally-heavy features needing dogfood/distribution proof; never in
  stable 0.3 surface.
- **DEFER:** valid but too large/risky/low-utility for 0.3; candidate for later
  with its own module/artifact/test story.
- **REJECT:** architecturally wrong for ArtiTor (wrong layer, abuse-prone,
  platform-impossible, or semver-voiding).

*End of original audit (2026-09-29) — no production code changed.*

---

## Remediation addendum (2026-09-29) — architecture freeze corrections

This addendum does NOT erase the research above. It records binding
corrections that supersede the §6/§7/§8/§13/§14 proposals wherever they
conflict. The normative implementation contract is
`docs/design/ARTITOR_0_3_API_FREEZE.md`; this audit remains the evidence
base, the freeze document is the build instruction.

Research retained as-is: isolation semantics (§3.1 owner×stream conjunction,
shared `ClientShared`, token lifetime), bridge line syntax/validation (§3.3),
PT mobile feasibility (§3.4), dormant-vs-pause distinction (§3.5,
`pause != dormant`), onion-client-already-works (§3.6), hosting/vanguards/RPC
cost analysis (§3.7/§3.8/§3.11), `ErrorKind` stability contract (§3.10), feature
matrix (§4), binary impact (§5).

### R1. Session ownership (supersedes §6 `TorIsolationSession` sketch + §7 `SocksSession{Arc<TorClient>}`)

REMEDIATED: exported session objects MUST NOT own a strong `Arc<TorClient>`,
runtime, listener, or connection tasks. The §7 sketch storing
`SessionState{client: Arc<TorClient>, …}` inside the UniFFI `SocksSession`
object violates the 0.2 lifecycle contract (`shutdown()` → drop client +
runtime → OFF) because a Kotlin-retained handle would keep Tor alive past
shutdown. Frozen model: engine owns all strong references
(`root_client`, per-session `isolated_client()` handles, listeners, tasks,
registry); FFI session object owns only `(id, generation, Weak<Shared>)`.
Stale handles are logically invalid after `close()`/shutdown/generation bump
and never resurrect on cold restart. See freeze §§4–6.

### R2. No closeable default session (supersedes §6 `defaultSession`, §8 session rules)

REMEDIATED: `val ArtiTorClient.defaultSession: TorIsolationSession` is
WITHDRAWN. A closeable default session contradicts `TorStatus.socksPort /
isReady / RUNNING` (closing it would destroy the endpoint the engine claims
is ready). Frozen model: root SOCKS is owned exclusively by `ArtiTorClient`
(created by `start`/`resume`, removed by `pause`/`shutdown`,
`TorStatus.socksPort` remains the back-compat representation); only
*additional* isolation contexts are `TorIsolationSession` objects. See freeze
§§3–6.

### R3. Session endpoint is dynamic state (supersedes §6 `val socksEndpoint`, §8 "sessions retained, identities preserved")

REMEDIATED: `val socksEndpoint: TorSocksEndpoint` as an immutable session
property is WITHDRAWN. Per-session listeners bind ephemeral port `0`; the port
may change on every `resume()` and is unavailable while PAUSED/CLOSED/
INVALIDATED. Frozen model: `TorIsolationSession.status:
StateFlow<TorIsolationSessionStatus(state, socksEndpoint?))>` with
`ACTIVE/PAUSED/CLOSED/INVALIDATED`; endpoint transitions are atomic; apps must
rebuild HTTP/WebSocket clients on emission and never reuse a stale port. Same
port across resume is explicitly NOT guaranteed. See freeze §§3, 6.

### R4. One isolation primitive (narrows §1/§3.1 "isolated_client() or new_isolation_group()")

REMEDIATED: Phase 1 uses exactly one primitive — `TorClient::isolated_client()`
per session, with plain `isolated.connect(target)` inside the SOCKS accept
loop. `StreamPrefs::new_isolation_group()` / `connect_with_prefs` are NOT
combined with it in Phase 1 (redundant owner×stream complexity with no
demonstrated requirement). Sibling sessions never share circuits because owner
tokens differ; guards/directory/circmgr/runtime stay shared by design. See
freeze §7.

### R5. Error evolution without new subclasses (supersedes §6 "+5 ArtiException buckets", §13 sealed+note)

REMEDIATED: adding `Network/ExitFailed/TargetRejected/Storage/
BootstrapRequired` as sealed subclasses is WITHDRAWN as a 0.3 stable API. It
breaks exhaustive `when` in downstream app code on recompilation (source-
incompatible for a minor). Frozen model (design B+C hybrid): keep the existing
7 `ArtiException` classes with zero subclasses added; add `abstract val kind:
TorErrorKind` (stable enum with `UNKNOWN` fallback, `else`-required matching);
classification originates only from `tor_error::ErrorKind::kind()`; no
`ErrorDetail`, no message parsing, future upstream variants map to `UNKNOWN`
without a breaking release. Session-lifecycle failures reuse existing classes
(`Runtime`/`NotRunning`) with dedicated `kind` values (`SESSION_CLOSED`,
`SESSION_INVALIDATED`), never a new subclass. See freeze §8.

### R6. No live reconfigure in stable 0.3 (narrows §1/§3.9/§3.12 "ADOPT_INTERNAL 2-section live-reconfigure")

REMEDIATED: the whitelisted live-`reconfigure()` for `address_filter` +
`stream_timeouts` is WITHDRAWN from the stable 0.3 implementation scope. The
upstream non-reconfigurable list is explicitly incompletely documented
(arti#1721), and ArtiTor already has a deterministic rebuild contract that
covers these fields. Frozen rule: `socksPort`-only change → listener rebind;
every other public `ArtiConfig` change → teardown + rebuild. Live
`reconfigure()` remains future/internal research only. See freeze §9.

### R7. `allowLocalAddrs` excluded (narrows §1/§3.12/§6 `allowLocalAddrs`)

REMEDIATED: `allowLocalAddrs` is WITHDRAWN from the stable 0.3 core. No normal
mobile KMP consumer needs localhost/LAN Tor targeting; the SSRF-style misuse
surface outweighs the hypothetical use case. `allowOnionAddrs` stays (obvious
Tor consumer purpose). `allowLocalAddrs` may return as advanced/future-only
with an explicit use case. See freeze §9.

### R8. Dormant out of 0.3 implementation (narrows §1/§3.5/§14 Phase 4 "dormant dogfood")

REMEDIATED: all dormant implementation work (dogfood flag, internal
`set_dormant` FFI, public `goDormant/wake`) is WITHDRAWN from every 0.3
implementation phase and moved to a 0.4 candidate. The §3.5 research and the
`pause != dormant, never silently coupled` conclusion are retained. Stable 0.3
already expands lifecycle surface enough via sessions. See freeze §§2, 11.

### R9. Session cap "8" withdrawn (supersedes §6 "e.g. 8", §9 "cap (e.g. 8)", §16 Q1)

REMEDIATED: the number 8 was an undocumented sketch constant and is WITHDRAWN
as a frozen public limit. Frozen rule: no public constant in the 0.3 API; Rust
enforces an internal, non-contractual safety cap (FD/listener/task/circuit
pressure) chosen at implementation time; exceeding it fails `create` with the
existing `ArtiException.Runtime` (kind `RUNTIME`) and is documented as an
implementation detail, not a compatibility promise. See freeze §10.

### R10. Phases revised (supersedes §14)

REMEDIATED: §14's phase contents (notably Phase 3 mixing errors+knobs and
Phase 4 mixing onion+dormant) are superseded by the freeze §11 decomposition:
Phase 1 sessions-only foundation; Phase 2 bridges-only; Phase 3 errors-only
(after the §R5 freeze); Phase 4 onion/config polish (no reconfigure, no
dormant); Phase 5 full audit. Dormant/PT/hosting/RPC appear in no 0.3
implementation phase. The next implementation prompt must be generated from
freeze §11, not from §14 above.


## Historical status at Phase-1 closure — 2026-09-30

This document is historical capability research, not authority to implement
its original proposals. The remediation addendum above and
`docs/design/ARTITOR_0_3_API_FREEZE.md` supersede conflicting recommendations
(including strong session ownership, defaultSession, combined isolation
primitives, new exception subclasses, live reconfigure, dormant work and cap 8).
Phase 1 is CLOSED at production SHA
`86da3c8dc71025278df66d588126cc0bef475b49`, independently accepted as
PASS WITH FOLLOW-UPS. Its implemented cap is private and conservative at 32;
Phase-5 measurement/reassessment remains pending. The separate structural
Rust modularization does not adopt deferred capabilities or start Phase 2.
