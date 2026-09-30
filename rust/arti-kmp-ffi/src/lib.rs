//! Thin UniFFI surface over `arti-client` (Tor in Rust) for Kotlin Multiplatform.
//!
//! Design goals (see repo docs/adr and docs/api-lifecycle-bitchat.md):
//! - The async tokio runtime is owned *inside* this crate; callers never block their main thread.
//! - Bootstrap progress is a first-class signal sourced from `TorClient::bootstrap_events()`
//!   (`BootstrapStatus::as_frac()`), NOT scraped from log lines.
//! - Lifecycle splits bootstrap from SOCKS: [pause] keeps the client; [resume] rebinds SOCKS;
//!   [shutdown] tears everything down.
//! - rustls only, no OpenSSL.
//! - No platform-specific code (no JNI, no android_logger): portable to Android, iOS, desktop.

use std::sync::{Arc, Mutex, Weak};
mod config;
mod engine;
mod error;
mod logging;
mod socks;
use engine::{close_session_handle, Inner, Shared};

uniffi::setup_scaffolding!();

// ============================================================================
// Public FFI types
// ============================================================================

/// High-level lifecycle state, mirrored 1:1 into Kotlin `TorState`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum TorState {
    Off,
    Starting,
    Bootstrapping,
    Running,
    /// Bootstrapped client kept; SOCKS listener is down.
    Paused,
    Stopping,
    Error,
}

/// Caller-supplied configuration. Paths are provided by the caller.
///
/// `socks_port == 0` binds an ephemeral port; the actual port is reported via
/// [StatusListener::on_status].
#[derive(Debug, Clone, uniffi::Record)]
pub struct ArtiConfig {
    pub data_dir: String,
    pub socks_port: u16,
    #[uniffi(default = [])]
    pub bridges: Vec<String>,
    #[uniffi(default = None)]
    pub state_dir: Option<String>,
    #[uniffi(default = None)]
    pub cache_dir: Option<String>,
}

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum ArtiError {
    #[error("already running")]
    AlreadyRunning,
    #[error("not running")]
    NotRunning,
    #[error("configuration error: {msg}")]
    Config { msg: String },
    #[error("failed to bind SOCKS on port {port}: {msg}")]
    Bind { port: u16, msg: String },
    #[error("bootstrap failed: {msg}")]
    Bootstrap { msg: String },
    #[error("runtime error: {msg}")]
    Runtime { msg: String },
}

/// Typed error discriminant for asynchronous failure notification.
///
/// [`ArtiError`] itself is a UniFFI `Error` type and therefore cannot travel as a
/// callback argument; this mirror record carries the same information across the
/// [`StatusListener::on_error`] callback so the Kotlin layer can reconstruct the
/// declared public type *without* parsing status strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ErrorKind {
    AlreadyRunning,
    NotRunning,
    Config,
    Bind,
    Bootstrap,
    Runtime,
}

/// Typed asynchronous failure payload (see [`ErrorKind`]).
#[derive(Debug, Clone, uniffi::Record)]
pub struct ArtiErrorDetail {
    pub kind: ErrorKind,
    /// Only meaningful when `kind == Bind`; `None` otherwise.
    pub port: Option<u16>,
    pub msg: String,
}

/// Status sink implemented on the Kotlin/Swift side. Invoked from worker
/// threads inside the owned runtime; implementations must be thread-safe.
///
/// Ordering guarantee: when an asynchronous operation fails, `on_error` is
/// invoked with the typed failure *before* the corresponding
/// `on_status(Error, ..)` report, on the same worker thread.
/// Per-session lifecycle state, mirrored into Kotlin
/// `TorIsolationSessionState`.
///
/// `Closed` = caller explicitly closed the session; `Invalidated` = the engine
/// lifecycle destroyed its generation (shutdown / rebuild / restart). They are
/// distinct terminal states in the public Kotlin latch. Native diagnostic
/// tombstones are bounded; a pruned CLOSED snapshot degrades to INVALIDATED.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum SessionState {
    Active,
    Paused,
    Closed,
    Invalidated,
}

/// Atomic `(state, endpoint)` snapshot for one isolation session.
///
/// Invariant: `port != None` iff `state == Active`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct SessionStatusFfi {
    pub state: SessionState,
    pub port: Option<u16>,
    pub revision: u64,
}

/// Live-session descriptor for `list_sessions` snapshots.
///
/// Only live (non-`Closed`, non-`Invalidated`) sessions are listed; the root
/// endpoint is never part of this list.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SessionInfo {
    pub id: String,
    pub state: SessionState,
    pub port: Option<u16>,
}

/// Per-session status sink implemented on the Kotlin side.
///
/// The engine invokes this synchronously (same thread) for the initial
/// `create_session` publication and from runtime worker threads for later
/// transitions. Publications may be delayed/reordered across threads: revision
/// is monotonic at mutation, and consumers reject older/equal revisions.
/// Callbacks may synchronously re-enter accessors and lifecycle operations.
#[uniffi::export(callback_interface)]
pub trait SessionStatusListener: Send + Sync {
    fn on_session_status(
        &self,
        session_id: String,
        state: SessionState,
        port: Option<u16>,
        revision: u64,
    );
}

#[uniffi::export(callback_interface)]
pub trait StatusListener: Send + Sync {
    /// `bootstrap_percent` is 0..=100. `socks_port` is `Some` only once the
    /// local SOCKS listener is actually bound and accepting.
    /// `summary` is a short UI-oriented string (may be empty).
    fn on_status(
        &self,
        state: TorState,
        bootstrap_percent: u32,
        socks_port: Option<u16>,
        summary: String,
    );
    fn on_log(&self, line: String);
    /// Typed asynchronous failure. Always precedes the `Error` status report
    /// for the same failure; never parse `summary` strings to recover types.
    fn on_error(&self, error: ArtiErrorDetail);
}

/// Exported UniFFI isolation-session handle — deliberately lightweight and
/// non-owning.
///
/// Owns ONLY `(id, generation, Weak<Shared>)` for registry access. It MUST
/// NOT (and does not) hold: `TorClient` (strong), Tokio runtime, listener
/// task, connection tasks, or engine state. A Kotlin-retained `SocksSession`
/// after `shutdown()` therefore keeps no native Tor resources alive:
/// `Weak::upgrade()` fails and the handle reports `Invalidated`.
#[derive(uniffi::Object)]
pub struct SocksSession {
    id: String,
    generation: u64,
    weak: Weak<Shared>,
}

impl std::fmt::Debug for SocksSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SocksSession")
            .field("id", &self.id)
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}

#[uniffi::export]
impl SocksSession {
    /// Opaque diagnostic id (unique within the process generation; NOT a Tor
    /// identity, NOT a privacy identity).
    pub fn id(&self) -> String {
        self.id.clone()
    }

    /// Engine generation this handle belongs to (stale-handle barrier).
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Synchronous, idempotent, never throws. First call stops the session
    /// listener, aborts its tracked connections, publishes `Closed(null)`,
    /// and releases the strong isolated client. Later calls, calls on
    /// `Invalidated` handles, and calls on stale/unknown handles are no-ops.
    pub fn close_session(&self) {
        close_session_handle(&self.weak, &self.id, self.generation);
    }

    /// Current snapshot without throwing in any state (live snapshot,
    /// recorded terminal state, or `Invalidated` for stale/unknown handles).
    pub fn status_snapshot(&self) -> SessionStatusFfi {
        match self.weak.upgrade() {
            Some(shared) => shared.session_snapshot_for(&self.id, self.generation),
            None => SessionStatusFfi {
                state: SessionState::Invalidated,
                port: None,
                revision: u64::MAX,
            },
        }
    }
}

#[derive(uniffi::Object)]
pub struct ArtiTor {
    inner: Mutex<Inner>,
}

// Keep UniFFI macro expansion at crate root: module paths contribute to checksums.
include!("ffi.rs");
