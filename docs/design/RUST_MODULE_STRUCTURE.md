# Rust module structure

The accepted production baseline is
`86da3c8dc71025278df66d588126cc0bef475b49`. This extraction preserves behavior,
public API, UniFFI checksums, ownership and the separate lifecycle fences.

```text
src/
├── lib.rs                   # Public types, objects, callbacks, scaffolding
├── ffi.rs                   # Exported engine facade, included at crate root
├── config.rs                # Exact config identity and directory resolution
├── error.rs                 # Typed callback error conversion
├── logging.rs               # Process-global tracing/panic forwarding
├── socks.rs                 # SOCKS5 framing and safe categorical diagnostic
├── engine/
│   ├── mod.rs               # Shared/Inner ownership, epochs, gate, publication
│   ├── lifecycle.rs         # Start/pause/resume/shutdown and ERROR ordering
│   ├── bootstrap.rs         # Owned cold worker and bootstrap progress
│   ├── root_socks.rs        # Root listener and ready-before-session barrier
│   ├── dispatch.rs          # Common selected-client CONNECT tunnel
│   ├── test_hooks.rs        # cfg(test) deterministic scheduling seams
│   └── session/
│       ├── mod.rs           # Registry/resources, demotion, terminal teardown, rebind
│       ├── admission.rs     # Create transaction and engine session accessors
│       └── listener.rs      # Revision-checked accept/failure and isolated dispatch
└── tests/
    ├── mod.rs               # Internal engine child, cfg(test)
    ├── support.rs           # Offline engine fixtures and reusable recorders
    ├── config.rs
    ├── lifecycle.rs
    ├── sessions.rs
    ├── races.rs             # Admission × pause/error/shutdown/replacement
    ├── publication.rs       # ACTIVE callback/barrier/revision races
    ├── errors.rs            # Demotion → typed error → fresh engine status
    ├── ownership.rs         # Strong resource/task release
    └── socks.rs             # Framing, dispatch identity, stale listener failure
```

`lib.rs` includes `ffi.rs` so the UniFFI macro expansion remains at the
original crate-root path. Moving exported impls to a child module changed
UniFFI checksums during verification. The final facade has the original
signatures/attributes/docs and regenerates byte-identical bindings. Each
facade method directly delegates to its `ffi_*` engine implementation.

`engine/mod.rs` includes `lifecycle.rs` and `session/admission.rs` at engine
scope. These coherent source units can access parent-private Shared/Inner
fields; their `pub(super)` entry points are visible to the root facade.
All other engine source units are ordinary child modules. This keeps the
ownership boundary small without making state fields crate-public.

The engine owns runtime, worker, root client, session registry, isolated
clients, listeners and tasks. `SocksSession` owns only id, generation and
Weak<Shared>. Session resources are engine-scoped; sessions are a child domain
rather than an independent subsystem importing engine internals in both
directions. Registry mutation helpers collect pending notifications; callers
release the gate/locks before foreign callbacks. Existing guard scopes and
explicit drops are retained.

Dependency direction: public data/protocol/config/logging → authoritative
engine state → lifecycle orchestration → bootstrap/root/session workers →
shared selected-client dispatch → SOCKS protocol. Workers consult parent
state; lifecycle calls session demotion/invalidation/rebind. No protocol
parser contains lifecycle policy. Generation, client_epoch, worker revision,
engine publication revision and per-session status revision remain distinct.

Tests are mounted via `#[cfg(test)] #[path = "../tests/mod.rs"] mod tests`
inside engine. Their private-state access stays within the engine tree.
Keep one-off listeners beside their race/error family, shared fixtures in
support, and scheduling hooks cfg(test). Do not collapse implementations or
tests back into lib.rs or widen authoritative fields to simplify imports.
