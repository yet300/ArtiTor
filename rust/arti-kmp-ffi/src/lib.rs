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

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Once, Weak};

use arti_client::config::TorClientConfigBuilder;
use arti_client::TorClient;
use futures::StreamExt;
#[cfg(test)]
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::runtime::Runtime;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tor_rtcompat::PreferredRuntime;

mod socks;
#[cfg(test)]
use socks::{parse_socks_request, SocksRequestError};

uniffi::setup_scaffolding!();

// ============================================================================
// Isolation sessions (0.3 Phase 1) — internal limits and identity
// ============================================================================

/// Private conservative safety cap based on listener/task/client resource accounting.
/// Non-contractual; Phase 5 must measure incremental FD, memory and task costs
/// on Android devices and iOS devices/simulators and empirically revisit 32.
const MAX_SESSIONS: usize = 32;

/// Process-local opaque session-id counter.
///
/// Session ids are diagnostic only: unique within the process (hence within
/// any engine generation), never derived from `IsolationToken`, never
/// persisted, never reused. Uniqueness is the soft barrier; the
/// `(id, generation)` check against the live registry is the hard barrier
/// against stale-handle resurrection.
static SESSION_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

fn next_session_id() -> String {
    let n = SESSION_ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("sess-{n:016x}")
}

// ============================================================================
// tracing -> StatusListener.on_log forwarding
// ============================================================================

static LOG_SINK: Mutex<Option<Arc<dyn StatusListener>>> = Mutex::new(None);
static TRACING_INIT: Once = Once::new();

struct ForwardLayer;

struct MsgVisitor {
    msg: String,
    extra: String,
}

impl tracing::field::Visit for MsgVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.msg = format!("{value:?}");
        } else {
            self.extra
                .push_str(&format!(" {}={:?}", field.name(), value));
        }
    }
}

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for ForwardLayer {
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let listener = LOG_SINK.lock().unwrap().clone();
        let Some(listener) = listener else {
            return;
        };
        let mut visitor = MsgVisitor {
            msg: String::new(),
            extra: String::new(),
        };
        event.record(&mut visitor);
        let meta = event.metadata();
        listener.on_log(format!(
            "{} [{}]{} {}",
            meta.level(),
            meta.target(),
            visitor.extra,
            visitor.msg
        ));
    }
}

fn init_tracing() {
    use tracing_subscriber::filter::LevelFilter;
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    use tracing_subscriber::Layer;
    TRACING_INIT.call_once(|| {
        let _ = tracing_subscriber::registry()
            .with(ForwardLayer.with_filter(LevelFilter::INFO))
            .try_init();
        std::panic::set_hook(Box::new(|info| {
            let listener = LOG_SINK.lock().unwrap().clone();
            if let Some(listener) = listener {
                listener.on_log(format!("PANIC: {info}"));
            }
        }));
    });
}

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

impl From<&ArtiError> for ArtiErrorDetail {
    fn from(e: &ArtiError) -> Self {
        match e {
            ArtiError::AlreadyRunning => Self {
                kind: ErrorKind::AlreadyRunning,
                port: None,
                msg: "already running".into(),
            },
            ArtiError::NotRunning => Self {
                kind: ErrorKind::NotRunning,
                port: None,
                msg: "not running".into(),
            },
            ArtiError::Config { msg } => Self {
                kind: ErrorKind::Config,
                port: None,
                msg: msg.clone(),
            },
            ArtiError::Bind { port, msg } => Self {
                kind: ErrorKind::Bind,
                port: Some(*port),
                msg: msg.clone(),
            },
            ArtiError::Bootstrap { msg } => Self {
                kind: ErrorKind::Bootstrap,
                port: None,
                msg: msg.clone(),
            },
            ArtiError::Runtime { msg } => Self {
                kind: ErrorKind::Runtime,
                port: None,
                msg: msg.clone(),
            },
        }
    }
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

/// Typed pending session notification returned by lifecycle mutation
/// helpers. The caller dispatches these after releasing all locks.
#[derive(Clone)]
struct PendingSessionNotification {
    id: String,
    listener: Arc<dyn SessionStatusListener>,
    state: SessionState,
    port: Option<u16>,
    revision: u64,
}

/// Outcome of a pause operation, distinguishing normal RUNNING→PAUSED
/// from bootstrap cancellation (STARTING/BOOTSTRAPPING→OFF).
enum PauseOutcome {
    Paused {
        sessions: Vec<PendingSessionNotification>,
        listener: Option<Arc<dyn StatusListener>>,
    },
    CancelledBootstrap {
        sessions: Vec<PendingSessionNotification>,
        listener: Option<Arc<dyn StatusListener>>,
    },
}

// ============================================================================
// Shared handles between lifecycle methods and async tasks
// ============================================================================

/// Engine-owned session resources.
///
/// Lives in the [`Shared`] registry; NEVER in the exported [`SocksSession`]
/// handle. Dropping a `SessionRuntime` (close / pause-teardown / shutdown)
/// releases the strong isolated `TorClient`, the listener task, and all
/// tracked connection tasks for that session only.
struct SessionRuntime {
    /// Engine generation that created this session. Must match both the
    /// owning [`Shared::generation`] and the handle's generation for any
    /// operation; id equality alone never validates.
    generation: u64,
    /// The session's sole isolation mechanism: `root.isolated_client()`.
    /// Retained across pause (identity preserved), dropped at close/shutdown.
    isolated: Arc<TorClient<PreferredRuntime>>,
    /// Live state: only `Active` or `Paused` is stored. Terminal states are
    /// published then recorded in [`Shared::tombstones`], never stored here.
    state: SessionState,
    status_revision: u64,
    /// Actual bound loopback port when `Active`; `None` when `Paused`.
    port: Option<u16>,
    /// Signals the session accept loop to exit (close / pause / shutdown).
    shutdown_tx: Option<oneshot::Sender<()>>,
    /// Session accept-loop task (or in-flight rebind task on resume).
    listener_task: Option<JoinHandle<()>>,
    /// Per-connection handlers accepted on this session's listener.
    connections: Vec<JoinHandle<()>>,
    /// Kotlin status sink for atomic `(state, endpoint)` publications.
    status_listener: Option<Arc<dyn SessionStatusListener>>,
}

impl SessionRuntime {
    fn snapshot(&self) -> SessionStatusFfi {
        SessionStatusFfi {
            state: self.state,
            port: self.port,
            revision: self.status_revision,
        }
    }

    /// Abort listener + tracked connections (fail-closed). Synchronous;
    /// never awaits. Safe to call redundantly.
    fn abort_all(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        if let Some(h) = self.listener_task.take() {
            h.abort();
        }
        for h in self.connections.drain(..) {
            h.abort();
        }
    }
}

struct Shared {
    /// Engine generation this `Shared` belongs to. Bumped on every shutdown;
    /// sessions (and handles) carrying an older generation are stale.
    generation: u64,
    /// Lock order: inner -> transition_gate -> state/client -> sessions -> tombstones.
    /// No callbacks or await while held.
    transition_gate: Mutex<()>,
    client_epoch: AtomicU64,
    worker_revision: AtomicU64,
    client: Mutex<Option<Arc<TorClient<PreferredRuntime>>>>,
    listener: Mutex<Option<Arc<dyn StatusListener>>>,
    /// Actual bound SOCKS port (0 = not listening).
    bound_port: AtomicU16,
    bootstrap_done: AtomicBool,
    /// Last reported engine lifecycle state (updated by `report`, including
    /// via `notify_error`). Gates session creation and rebind publication.
    engine_state: Mutex<TorState>,
    /// Signals the active SOCKS loop to exit (pause / shutdown).
    socks_shutdown: Mutex<Option<oneshot::Sender<()>>>,
    /// Per-connection SOCKS handlers. Aborted on pause/shutdown (fail-closed:
    /// Tor OFF means no traffic is retained; see `pause` docs).
    connections: Mutex<Vec<JoinHandle<()>>>,
    /// Live isolation sessions: engine-owned strong resources.
    ///
    /// Lock ordering (mandatory): `ArtiTor.inner` → `Shared.sessions`, never
    /// the reverse. Never `.await` (or `block_on`) while holding this lock;
    /// never invoke FFI callbacks while holding it (clone listener refs under
    /// the lock, publish after release).
    sessions: Mutex<HashMap<String, SessionRuntime>>,
    /// Terminal states of removed sessions, keyed by unique session id.
    /// Lets `session_status` distinguish `Closed` from `Invalidated` after
    /// the strong `SessionRuntime` is gone. Ids are never reused, so this map
    /// cannot confuse two sessions. Cleared implicitly when this `Shared` is
    /// replaced on shutdown (post-shutdown lookups fail at `Weak` upgrade and
    /// report `Invalidated`, which the Kotlin latch reconciles: a handle that
    /// already latched `Closed` keeps it).
    tombstones: Mutex<HashMap<String, (SessionState, u64)>>,
    #[cfg(test)]
    test_listener_probe: Mutex<Option<Arc<TestListenerProbe>>>,
    #[cfg(test)]
    test_create_decision_gate: Mutex<Option<Arc<TestCommitGate>>>,
    #[cfg(test)]
    test_rebind_decision_gate: Mutex<Option<Arc<TestCommitGate>>>,
    #[cfg(test)]
    test_accept_failure: Mutex<Option<String>>,
    #[cfg(test)]
    test_dispatch_clients: Mutex<Vec<(Option<String>, usize)>>,
}

impl Shared {
    fn new(generation: u64) -> Arc<Self> {
        Arc::new(Self {
            generation,
            transition_gate: Mutex::new(()),
            client_epoch: AtomicU64::new(1),
            worker_revision: AtomicU64::new(1),
            client: Mutex::new(None),
            listener: Mutex::new(None),
            bound_port: AtomicU16::new(0),
            bootstrap_done: AtomicBool::new(false),
            engine_state: Mutex::new(TorState::Off),
            socks_shutdown: Mutex::new(None),
            connections: Mutex::new(Vec::new()),
            sessions: Mutex::new(HashMap::new()),
            tombstones: Mutex::new(HashMap::new()),
            #[cfg(test)]
            test_listener_probe: Mutex::new(None),
            #[cfg(test)]
            test_create_decision_gate: Mutex::new(None),
            #[cfg(test)]
            test_rebind_decision_gate: Mutex::new(None),
            #[cfg(test)]
            test_accept_failure: Mutex::new(None),
            #[cfg(test)]
            test_dispatch_clients: Mutex::new(vec![]),
        })
    }

    fn set_listener(&self, listener: Arc<dyn StatusListener>) {
        *self.listener.lock().unwrap() = Some(listener.clone());
        *LOG_SINK.lock().unwrap() = Some(listener);
    }

    fn listener(&self) -> Option<Arc<dyn StatusListener>> {
        self.listener.lock().unwrap().clone()
    }

    fn engine_state(&self) -> TorState {
        *self.engine_state.lock().unwrap()
    }

    fn report_worker(
        &self,
        revision: u64,
        state: TorState,
        pct: u32,
        port: Option<u16>,
        summary: impl Into<String>,
    ) -> bool {
        {
            let _transition = self.transition_gate.lock().unwrap();
            if self.worker_revision.load(Ordering::SeqCst) != revision {
                return false;
            }
            *self.engine_state.lock().unwrap() = state;
            if let Some(p) = port {
                self.bound_port.store(p, Ordering::SeqCst);
            }
        }
        if let Some(l) = self.listener() {
            l.on_status(state, pct, port, summary.into());
        }
        true
    }

    fn client(&self) -> Option<Arc<TorClient<PreferredRuntime>>> {
        self.client.lock().unwrap().clone()
    }

    #[cfg(test)]
    fn set_client(&self, c: Option<Arc<TorClient<PreferredRuntime>>>) {
        let _transition = self.transition_gate.lock().unwrap();
        self.set_client_under_gate(c);
    }

    fn set_client_under_gate(&self, c: Option<Arc<TorClient<PreferredRuntime>>>) {
        self.client_epoch.fetch_add(1, Ordering::SeqCst);
        *self.client.lock().unwrap() = c;
    }

    fn install_socks_shutdown(&self) -> oneshot::Receiver<()> {
        let (tx, rx) = oneshot::channel();
        *self.socks_shutdown.lock().unwrap() = Some(tx);
        rx
    }

    fn signal_socks_shutdown(&self) {
        if let Some(tx) = self.socks_shutdown.lock().unwrap().take() {
            let _ = tx.send(());
        }
    }

    /// Track a per-connection SOCKS handler so pause/shutdown can terminate it.
    fn track_connection(&self, handle: JoinHandle<()>) {
        let mut conns = self.connections.lock().unwrap();
        // Opportunistic prune so the vec cannot grow without bound.
        conns.retain(|h| !h.is_finished());
        conns.push(handle);
    }

    /// Abort all live SOCKS connection handlers (fail-closed Tor OFF).
    fn abort_connections(&self) {
        let mut conns = self.connections.lock().unwrap();
        for h in conns.drain(..) {
            h.abort();
        }
    }

    /// Typed async failure notification: `on_error` (typed) first, then the
    /// `Error` status report and a log line. Callers must not encode the error
    /// type only into the summary string.
    ///
    /// Engine-ERROR session rule (freeze §6 matrix): on entry into `Error`,
    /// every live session is frozen as `Paused`-with-null-endpoint (identities
    /// kept); `Invalidated` follows only on the subsequent shutdown/rebuild.
    /// No session may expose an `Active` endpoint while the engine is in
    /// `Error`.
    #[cfg(test)]
    fn notify_error(&self, error: &ArtiError, bootstrap_pct: u32) {
        self.notify_worker_error(None, error, bootstrap_pct);
    }

    fn notify_worker_error(&self, expected: Option<u64>, error: &ArtiError, bootstrap_pct: u32) {
        let (pending, listener) = {
            let _transition = self.transition_gate.lock().unwrap();
            if expected.is_some_and(|rev| self.worker_revision.load(Ordering::SeqCst) != rev) {
                return;
            }
            self.worker_revision.fetch_add(1, Ordering::SeqCst);
            *self.engine_state.lock().unwrap() = TorState::Error;
            self.bound_port.store(0, Ordering::SeqCst);
            self.signal_socks_shutdown();
            self.abort_connections();
            (
                self.demote_sessions_to_paused("engine error"),
                self.listener(),
            )
        };
        for n in pending {
            n.listener
                .on_session_status(n.id, n.state, n.port, n.revision);
        }
        if let Some(l) = listener {
            l.on_error(ArtiErrorDetail::from(error));
            l.on_status(
                TorState::Error,
                bootstrap_pct,
                None,
                format!("error: {error}"),
            );
            l.on_log(format!("ERROR: {error}"));
        }
    }

    /// Freeze every live session to `Paused` (endpoint null), aborting its
    /// listener and tracked connections but retaining its isolated client
    /// (isolation identity preserved). Publishes only genuine `Active → Paused`
    /// transitions. No-op when the registry is empty.
    ///
    /// Returns typed pending notifications for the caller to dispatch after
    /// releasing all locks (callback re-entry safety). Does NOT invoke any
    /// FFI callbacks itself.
    fn demote_sessions_to_paused(&self, _reason: &str) -> Vec<PendingSessionNotification> {
        let mut sessions = self.sessions.lock().unwrap();
        let mut out = Vec::new();
        for (id, entry) in sessions.iter_mut() {
            entry.abort_all();
            if entry.state == SessionState::Active {
                entry.state = SessionState::Paused;
                entry.port = None;
                entry.status_revision += 1;
                if let Some(l) = entry.status_listener.clone() {
                    out.push(PendingSessionNotification {
                        id: id.clone(),
                        listener: l,
                        state: SessionState::Paused,
                        port: None,
                        revision: entry.status_revision,
                    });
                }
            } else {
                entry.state = SessionState::Paused;
                entry.port = None;
            }
        }
        out
    }

    /// Publish `Invalidated` (endpoint null) for every live session, abort all
    /// session listeners/connections, and drop all strong `SessionRuntime`
    /// resources (isolated clients released). Each live session publishes
    /// exactly once; already-removed (`Closed`) sessions are untouched.
    /// Must be called BEFORE root Tor resources are released.
    ///
    /// Returns typed pending notifications for the caller to dispatch after
    /// releasing all locks (callback re-entry safety). Does NOT invoke any
    /// FFI callbacks itself.
    fn invalidate_all_sessions(&self) -> Vec<PendingSessionNotification> {
        let mut sessions = self.sessions.lock().unwrap();
        let mut tombstones = self.tombstones.lock().unwrap();
        let mut out = Vec::new();
        for (id, mut entry) in sessions.drain() {
            entry.abort_all();
            entry.status_revision += 1;
            tombstones.insert(
                id.clone(),
                (SessionState::Invalidated, entry.status_revision),
            );
            if let Some(l) = entry.status_listener.clone() {
                out.push(PendingSessionNotification {
                    id: id.clone(),
                    listener: l,
                    state: SessionState::Invalidated,
                    port: None,
                    revision: entry.status_revision,
                });
            }
        }
        if tombstones.len() > MAX_SESSIONS * 4 {
            tombstones.clear();
        }
        out
    }

    /// Snapshot lookup for one handle: `(id, generation)` must match a live
    /// registry entry. Returns the entry snapshot, the recorded terminal state
    /// for a removed session, or `Invalidated` for unknown/stale handles.
    fn session_snapshot_for(&self, sid: &str, generation: u64) -> SessionStatusFfi {
        let _transition = self.transition_gate.lock().unwrap();
        if generation != self.generation {
            return SessionStatusFfi {
                state: SessionState::Invalidated,
                port: None,
                revision: u64::MAX,
            };
        }
        if let Some(entry) = self.sessions.lock().unwrap().get(sid) {
            if entry.generation == generation {
                return entry.snapshot();
            }
            // Id collision across generations: never resurrect.
            return SessionStatusFfi {
                state: SessionState::Invalidated,
                port: None,
                revision: u64::MAX,
            };
        }
        match self.tombstones.lock().unwrap().get(sid) {
            Some(&(terminal, revision)) => SessionStatusFfi {
                state: terminal,
                port: None,
                revision,
            },
            None => SessionStatusFfi {
                state: SessionState::Invalidated,
                port: None,
                revision: u64::MAX,
            },
        }
    }
}

/// Close the session identified by `(sid, generation)` against the `Shared`
/// behind `weak`.
///
/// Synchronous, idempotent, never fails: unknown ids, generation mismatches
/// (stale handles, including across restart), already-removed entries, and
/// dead engines (`Weak` upgrade failure after shutdown) are all no-ops. The
/// first close stops the listener, aborts tracked connections, publishes
/// `Closed(null)`, records the tombstone, and releases the strong isolated
/// client. Root and sibling sessions are untouched.
fn close_session_handle(weak: &Weak<Shared>, sid: &str, generation: u64) {
    let Some(shared) = weak.upgrade() else {
        return;
    };
    let transition = shared.transition_gate.lock().unwrap();
    let removed = {
        let mut sessions = shared.sessions.lock().unwrap();
        match sessions.get(sid) {
            Some(entry) if entry.generation == generation => sessions.remove(sid),
            _ => None,
        }
    };
    let Some(mut entry) = removed else {
        return;
    };
    entry.abort_all();
    let listener = entry.status_listener.clone();
    let revision = entry.status_revision + 1;
    // Release the strong isolated client before publishing (no Tor resources
    // retained past close).
    drop(entry);
    {
        let mut tombstones = shared.tombstones.lock().unwrap();
        tombstones.insert(sid.to_owned(), (SessionState::Closed, revision));
        if tombstones.len() > MAX_SESSIONS * 4 {
            tombstones.clear();
        }
    }
    drop(transition);
    if let Some(l) = listener {
        l.on_session_status(sid.to_owned(), SessionState::Closed, None, revision);
    }
}

// ============================================================================
// ArtiTor object
// ============================================================================

#[cfg(test)]
#[derive(Default)]
struct TestSignal {
    value: Mutex<usize>,
    changed: std::sync::Condvar,
}

#[cfg(test)]
impl TestSignal {
    fn signal(&self) {
        *self.value.lock().unwrap() += 1;
        self.changed.notify_all();
    }

    fn wait(&self) {
        self.wait_for(1);
    }

    fn wait_for(&self, count: usize) {
        let mut value = self.value.lock().unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while *value < count {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            let (next, timeout) = self.changed.wait_timeout(value, remaining).unwrap();
            value = next;
            assert!(
                !timeout.timed_out() || *value >= count,
                "deterministic test gate timed out"
            );
        }
    }
}

#[cfg(test)]
#[derive(Default)]
struct TestCommitGate {
    arrived: TestSignal,
    release: TestSignal,
    bound_port: Mutex<Option<u16>>,
}

#[cfg(test)]
impl TestCommitGate {
    fn arrive_and_wait(&self, port: Option<u16>) {
        *self.bound_port.lock().unwrap() = port;
        self.arrived.signal();
        self.release.wait();
    }
}

/// Observes the actual publication barrier receiver, rather than inferring
/// listener execution from TCP backlog acceptance or elapsed time.
#[cfg(test)]
#[derive(Default)]
struct TestListenerProbe {
    barrier: Mutex<Option<Arc<Mutex<oneshot::Receiver<()>>>>>,
    polled: TestSignal,
    dispatch_checked: TestSignal,
    dispatch_count: Arc<std::sync::atomic::AtomicUsize>,
}

#[cfg(test)]
impl TestListenerProbe {
    fn assert_barrier_pending(&self) {
        self.polled.wait();
        let barrier = self.barrier.lock().unwrap().as_ref().unwrap().clone();
        assert!(
            matches!(
                barrier.lock().unwrap().try_recv(),
                Err(oneshot::error::TryRecvError::Empty)
            ),
            "listener barrier released before ACTIVE callback completed"
        );
        assert_eq!(self.dispatch_count.load(Ordering::SeqCst), 0);
    }
}

#[cfg(test)]
async fn await_test_session_barrier(rx: oneshot::Receiver<()>, shared: &Shared) {
    let probe = shared.test_listener_probe.lock().unwrap().clone();
    if let Some(probe) = probe {
        let rx = Arc::new(Mutex::new(rx));
        *probe.barrier.lock().unwrap() = Some(rx.clone());
        let _ = std::future::poll_fn(|cx| {
            let poll = std::future::Future::poll(std::pin::Pin::new(&mut *rx.lock().unwrap()), cx);
            probe.polled.signal();
            poll
        })
        .await;
    } else {
        let _ = rx.await;
    }
}

struct Inner {
    runtime: Option<Runtime>,
    /// In-flight cold start (bootstrap + SOCKS) or a resume SOCKS task.
    worker: Option<JoinHandle<()>>,
    last_config: Option<ArtiConfig>,
    shared: Arc<Shared>,
    /// Monotonically increasing engine generation. Starts at 1; bumped on
    /// every shutdown. A session created in generation N is valid only in N:
    /// liveness checks compare `(id, generation)` against the live registry,
    /// so a stale handle can never attach to a new session even on id
    /// collision (ids are additionally never reused).
    generation: u64,
    /// Test-only hook: when set, `create_session` waits on this condvar
    /// after the initial state check but before the registry commit.
    /// This allows deterministic testing of create-vs-pause and
    /// create-vs-shutdown races. Never set in production.
    #[cfg(test)]
    test_commit_gate: Option<Arc<TestCommitGate>>,
    #[cfg(test)]
    test_spawn_cold_failure: bool,
    /// Test-only hook: counts dispatches in `run_session_listener`.
    /// Never set in production.
    #[cfg(test)]
    test_dispatch_count: Option<Arc<std::sync::atomic::AtomicUsize>>,
}

impl Default for Inner {
    fn default() -> Self {
        let generation = 1;
        Self {
            runtime: None,
            worker: None,
            last_config: None,
            shared: Shared::new(generation),
            generation,
            #[cfg(test)]
            test_commit_gate: None,
            #[cfg(test)]
            test_spawn_cold_failure: false,
            #[cfg(test)]
            test_dispatch_count: None,
        }
    }
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

#[uniffi::export]
impl ArtiTor {
    #[uniffi::constructor]
    pub fn new() -> Arc<Self> {
        // Process-global note: `LOG_SINK` routes tracing output to the most
        // recently installed listener. Only ONE ArtiTor instance per process is
        // supported; a second instance steals log routing (documented, not
        // silently multi-instance). Status/error callbacks remain per-instance.
        Arc::new(Self {
            inner: Mutex::new(Inner::default()),
        })
    }

    pub fn version(&self) -> String {
        format!(
            "arti-kmp-ffi {} (arti-client 0.46, rustls)",
            env!("CARGO_PKG_VERSION")
        )
    }

    /// True when a bootstrapped TorClient is held (RUNNING or PAUSED).
    pub fn has_client(&self) -> bool {
        let inner = self.inner.lock().unwrap();
        inner.shared.bootstrap_done.load(Ordering::SeqCst) && inner.shared.client().is_some()
    }

    /// True when SOCKS is bound and bootstrap is complete.
    pub fn is_ready(&self) -> bool {
        let inner = self.inner.lock().unwrap();
        inner.shared.bootstrap_done.load(Ordering::SeqCst)
            && inner.shared.bound_port.load(Ordering::SeqCst) != 0
    }

    /// Currently bound SOCKS port, if listening.
    pub fn socks_port(&self) -> Option<u16> {
        let p = self
            .inner
            .lock()
            .unwrap()
            .shared
            .bound_port
            .load(Ordering::SeqCst);
        if p == 0 {
            None
        } else {
            Some(p)
        }
    }

    /// Bootstrap (if needed) and bring up the local SOCKS proxy.
    ///
    /// Returns immediately; progress via `listener`.
    /// Readiness == `on_status(Running, 100, Some(port), _)`.
    /// Asynchronous failures are reported via `on_error` (typed) followed by
    /// `on_status(Error, ..)`.
    ///
    /// If a client is already bootstrapped and SOCKS is down (paused):
    /// - only `socks_port` changed → rebind SOCKS without re-bootstrapping;
    /// - TorClient-defining config (`data_dir`/`state_dir`/`cache_dir`/`bridges`,
    ///   including non-empty → empty) changed → the old client is torn down and
    ///   a new client is bootstrapped.
    /// If SOCKS is already up → [ArtiError::AlreadyRunning].
    pub fn start(
        &self,
        config: ArtiConfig,
        listener: Box<dyn StatusListener>,
    ) -> Result<(), ArtiError> {
        let mut inner = self.inner.lock().unwrap();
        let listener: Arc<dyn StatusListener> = Arc::from(listener);
        init_tracing();
        let _ = rustls::crypto::ring::default_provider().install_default();
        inner.shared.set_listener(listener);
        let shared = inner.shared.clone();
        let transition = shared.transition_gate.lock().unwrap();

        // SOCKS already up.
        if inner.shared.bound_port.load(Ordering::SeqCst) != 0 {
            return Err(ArtiError::AlreadyRunning);
        }
        // ERROR start is an explicit replacement even if the failing worker
        // has not yet returned from its last callback.
        if shared.engine_state() == TorState::Error {
            if let Some(worker) = inner.worker.take() {
                worker.abort();
            }
        }
        // Worker still running (bootstrapping or SOCKS without port yet).
        if let Some(ref w) = inner.worker {
            if !w.is_finished() {
                return Err(ArtiError::AlreadyRunning);
            }
            inner.worker = None;
        }

        // Resume path: a bootstrapped client is held, only SOCKS is down.
        if inner.shared.bootstrap_done.load(Ordering::SeqCst) && inner.shared.client().is_some() {
            let needs_new_client = shared.engine_state() == TorState::Error
                || inner
                    .last_config
                    .as_ref()
                    .is_some_and(|old| tor_client_config_changed(old, &config));
            if needs_new_client {
                // TorClient-defining config changed: discard the old client and
                // fall through to a cold bootstrap. Never keep a client built
                // with different bridges/dirs. A new TorClient means new
                // isolation roots: all sessions are INVALIDATED (freeze §9
                // rebuild rule); callers recreate them explicitly.
                let pending = inner.shared.invalidate_all_sessions();
                inner.shared.set_client_under_gate(None);
                inner.shared.bootstrap_done.store(false, Ordering::SeqCst);
                inner.shared.bound_port.store(0, Ordering::SeqCst);
                inner.shared.abort_connections();
                inner.last_config = Some(config.clone());
                let result = spawn_cold(&mut inner, config);
                drop(transition);
                drop(inner);
                for n in pending {
                    n.listener
                        .on_session_status(n.id, n.state, n.port, n.revision);
                }
                return result;
            } else {
                // SOCKS-only change (at most the port): rebind root + live
                // paused sessions, no bootstrap. Sessions keep no port
                // stability promise (ephemeral rebinds may differ).
                // Must wait for root bind to succeed before rebinding sessions.
                inner.last_config = Some(config.clone());
                let port = config.socks_port;
                let ready = spawn_socks(&mut inner, port)?;
                let runtime_handle = inner
                    .runtime
                    .as_ref()
                    .map(|rt| rt.handle().clone())
                    .ok_or_else(|| ArtiError::Runtime {
                        msg: "no runtime".into(),
                    })?;
                let shared = inner.shared.clone();
                drop(transition);
                drop(inner);
                let handle_for_task = runtime_handle.clone();
                runtime_handle.spawn(async move {
                    wait_root_then_rebind_sessions(ready, shared, handle_for_task).await;
                });
                return Ok(());
            }
        }

        // Cold start (also used when the client-defining config changed, and
        // when recovering from ERROR): a new TorClient is bootstrapped, so
        // any sessions frozen as PAUSED by an earlier ERROR entry are
        // INVALIDATED here (freeze §6: INVALIDATED on subsequent
        // shutdown/start). Fresh starts observe an empty registry (no-op).
        let pending = inner.shared.invalidate_all_sessions();
        inner.shared.set_client_under_gate(None);
        inner.last_config = Some(config.clone());
        let result = spawn_cold(&mut inner, config);
        drop(transition);
        drop(inner);
        for n in pending {
            n.listener
                .on_session_status(n.id, n.state, n.port, n.revision);
        }
        result
    }

    /// Re-bind SOCKS using the last configuration. Requires a bootstrapped client.
    ///
    /// Root is rebound first; then every live non-closed session is rebound
    /// on a fresh ephemeral port (`Paused → Active(new endpoint)`, published
    /// atomically). Ports MAY change across pause/resume — never assert
    /// equality. A session that fails to rebind stays `Paused` (null
    /// endpoint) with a diagnostic log while the engine stays `Running` if
    /// root bound; a root bind failure drives the engine to `Error` and
    /// freezes all sessions as `Paused`/null (never `Active` on an errored
    /// engine).
    pub fn resume(&self, listener: Box<dyn StatusListener>) -> Result<(), ArtiError> {
        let rebind = {
            let mut inner = self.inner.lock().unwrap();
            let listener: Arc<dyn StatusListener> = Arc::from(listener);
            inner.shared.set_listener(listener);
            let shared = inner.shared.clone();
            let _transition = shared.transition_gate.lock().unwrap();

            if inner.shared.bound_port.load(Ordering::SeqCst) != 0 {
                return Err(ArtiError::AlreadyRunning);
            }
            if let Some(ref w) = inner.worker {
                if !w.is_finished() {
                    return Err(ArtiError::AlreadyRunning);
                }
                inner.worker = None;
            }
            if !inner.shared.bootstrap_done.load(Ordering::SeqCst)
                || inner.shared.client().is_none()
            {
                return Err(ArtiError::NotRunning);
            }
            let port = inner
                .last_config
                .as_ref()
                .map(|c| c.socks_port)
                .unwrap_or(0);
            let ready = spawn_socks(&mut inner, port)?;
            let runtime_handle = inner
                .runtime
                .as_ref()
                .map(|rt| rt.handle().clone())
                .ok_or_else(|| ArtiError::Runtime {
                    msg: "no runtime".into(),
                })?;
            let shared = inner.shared.clone();
            (ready, shared, runtime_handle)
        };
        // Root rebind kicked; wait for root bind to succeed, then rebind sessions.
        let (ready, shared, handle) = rebind;
        if let Some(rt) = self.inner.lock().unwrap().runtime.as_ref() {
            rt.spawn(async move {
                wait_root_then_rebind_sessions(ready, shared, handle).await;
            });
        }
        Ok(())
    }

    /// Stop the SOCKS listener but keep the bootstrapped client and runtime.
    ///
    /// Deterministic pause-during-bootstrap semantics (see
    /// docs/api-lifecycle-bitchat.md §2.1): if no bootstrapped client is held
    /// yet (STARTING/BOOTSTRAPPING), the bootstrap worker is aborted, the
    /// partial/unbootstrapped client is discarded, all associated work
    /// (including SOCKS connection handlers) is stopped, and the state
    /// transitions to OFF — never left in BOOTSTRAPPING with no worker.
    /// All live SOCKS streams are terminated (fail-closed Tor OFF); new
    /// connections are no longer accepted.
    pub fn pause(&self) {
        let outcome = {
            let mut inner = self.inner.lock().unwrap();
            let shared = inner.shared.clone();
            let _transition = shared.transition_gate.lock().unwrap();
            shared.worker_revision.fetch_add(1, Ordering::SeqCst);
            inner.shared.signal_socks_shutdown();
            inner.shared.bound_port.store(0, Ordering::SeqCst);
            inner.shared.abort_connections();
            if let Some(w) = inner.worker.take() {
                w.abort();
            }
            if inner.shared.bootstrap_done.load(Ordering::SeqCst) && inner.shared.client().is_some()
            {
                *inner.shared.engine_state.lock().unwrap() = TorState::Paused;
                let sessions = inner.shared.demote_sessions_to_paused("engine pause");
                let listener = inner.shared.listener();
                PauseOutcome::Paused { sessions, listener }
            } else {
                let sessions = inner.shared.invalidate_all_sessions();
                inner.shared.set_client_under_gate(None);
                inner.shared.bootstrap_done.store(false, Ordering::SeqCst);
                inner.shared.bound_port.store(0, Ordering::SeqCst);
                *inner.shared.engine_state.lock().unwrap() = TorState::Off;
                let listener = inner.shared.listener();
                PauseOutcome::CancelledBootstrap { sessions, listener }
            }
        };
        match outcome {
            PauseOutcome::Paused { sessions, listener } => {
                for n in sessions {
                    n.listener
                        .on_session_status(n.id, n.state, n.port, n.revision);
                }
                if let Some(l) = listener {
                    l.on_status(TorState::Paused, 100, None, "paused".into());
                    l.on_log("SOCKS paused; TorClient retained".into());
                }
            }
            PauseOutcome::CancelledBootstrap { sessions, listener } => {
                for n in sessions {
                    n.listener
                        .on_session_status(n.id, n.state, n.port, n.revision);
                }
                if let Some(l) = listener {
                    l.on_status(TorState::Off, 0, None, "bootstrap cancelled".into());
                    l.on_log("bootstrap cancelled by pause; client discarded".into());
                }
            }
        }
    }

    /// Full teardown: SOCKS, client, and tokio runtime.
    ///
    /// Postcondition: OFF, no client, no SOCKS listener, no worker, runtime
    /// released. All live SOCKS streams are terminated. Idempotent.
    pub fn shutdown(&self) {
        let (session_pending, old_listener) = {
            let mut inner = self.inner.lock().unwrap();
            let shared = inner.shared.clone();
            let _transition = shared.transition_gate.lock().unwrap();
            shared.worker_revision.fetch_add(1, Ordering::SeqCst);
            *shared.engine_state.lock().unwrap() = TorState::Off;
            let session_pending = inner.shared.invalidate_all_sessions();
            inner.shared.signal_socks_shutdown();
            inner.shared.abort_connections();
            if let Some(w) = inner.worker.take() {
                w.abort();
            }
            inner.shared.set_client_under_gate(None);
            inner.shared.bootstrap_done.store(false, Ordering::SeqCst);
            inner.shared.bound_port.store(0, Ordering::SeqCst);
            inner.last_config = None;
            if let Some(rt) = inner.runtime.take() {
                rt.shutdown_background();
            }
            let old_listener = inner.shared.listener();
            inner.generation = inner.generation.wrapping_add(1).max(1);
            inner.shared = Shared::new(inner.generation);
            if let Some(ours) = old_listener.clone() {
                let mut sink = LOG_SINK.lock().unwrap();
                if sink.as_ref().is_some_and(|s| Arc::ptr_eq(s, &ours)) {
                    *sink = None;
                }
            }
            (session_pending, old_listener)
        };
        for n in session_pending {
            n.listener
                .on_session_status(n.id, n.state, n.port, n.revision);
        }
        if let Some(l) = old_listener {
            l.on_status(TorState::Off, 0, None, String::new());
        }
    }

    /// Deprecated alias for [Self::shutdown] (0.1.x compatibility).
    pub fn stop(&self) {
        self.shutdown();
    }

    /// Create an additional circuit-isolated SOCKS session.
    ///
    /// - While `Running`: derives `root.isolated_client()`, binds an
    ///   ephemeral loopback listener synchronously, registers atomically,
    ///   publishes `Active(endpoint)`, and returns the non-owning handle.
    ///   Bind failure returns [`ArtiError::Bind`] with no leaked registry
    ///   entry, no retained isolated client, and the engine untouched.
    /// - While `Paused`: derives the isolated client and registers as
    ///   `Paused(endpoint=null)` without binding; the listener binds on the
    ///   next successful engine `resume()`.
    /// - While `Starting`/`Bootstrapping`/`Stopping`/`Error`/`Off`: rejected
    ///   with [`ArtiError::NotRunning`] (never queued).
    /// - Over the internal safety cap: [`ArtiError::Runtime`] with
    ///   `"session limit reached"`.
    ///
    /// Locking: engine → session ordering; no await/block while holding
    /// either lock (the loopback bind uses blocking `std::net`, the accept
    /// loop is spawned onto the runtime handle). A pause/shutdown racing the
    /// bind is detected at registration: the just-bound socket is dropped and
    /// the session registers as `Paused`, or everything is dropped with
    /// `NotRunning` after a generation change.
    pub fn create_session(
        &self,
        listener: Box<dyn SessionStatusListener>,
    ) -> Result<Arc<SocksSession>, ArtiError> {
        let listener: Arc<dyn SessionStatusListener> = Arc::from(listener);
        // Snapshot engine state under the engine lock; release before any
        // syscalls or spawns.
        let (shared, generation, client_epoch, root, runtime, want_active) = {
            let inner = self.inner.lock().unwrap();
            let shared = inner.shared.clone();
            let _transition = shared.transition_gate.lock().unwrap();
            let want_active = match shared.engine_state() {
                TorState::Running => true,
                TorState::Paused => false,
                _ => return Err(ArtiError::NotRunning),
            };
            if shared.sessions.lock().unwrap().len() >= MAX_SESSIONS {
                return Err(ArtiError::Runtime {
                    msg: "session limit reached".into(),
                });
            }
            let root = shared.client().ok_or(ArtiError::NotRunning)?;
            let runtime = inner
                .runtime
                .as_ref()
                .ok_or_else(|| ArtiError::Runtime {
                    msg: "no runtime".into(),
                })?
                .handle()
                .clone();
            (
                shared.clone(),
                shared.generation,
                shared.client_epoch.load(Ordering::SeqCst),
                root,
                runtime,
                want_active,
            )
        };

        // The session's sole isolation mechanism: exactly one
        // `isolated_client()` per session. Session SOCKS traffic uses this
        // handle's plain `connect(target)` — never the root client's.
        let isolated = root.isolated_client();

        // Synchronous ephemeral loopback bind (blocking std socket: no
        // runtime, no await, no lock held). Never 0.0.0.0; no caller ports.
        // The port travels alongside the socket (the task takes ownership).
        let (bound_sock, bound_port): (Option<std::net::TcpListener>, Option<u16>) = if want_active
        {
            match std::net::TcpListener::bind("127.0.0.1:0") {
                Ok(sock) => {
                    let port = sock.local_addr().map(|a| a.port()).unwrap_or(0);
                    sock.set_nonblocking(true).map_err(|e| ArtiError::Bind {
                        port: 0,
                        msg: e.to_string(),
                    })?;
                    (Some(sock), Some(port))
                }
                Err(e) => {
                    // No registry entry, no retained client, engine
                    // untouched, siblings untouched.
                    return Err(ArtiError::Bind {
                        port: 0,
                        msg: e.to_string(),
                    });
                }
            }
        } else {
            (None, None)
        };

        let sid = next_session_id();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let mut shutdown_tx_opt = Some(shutdown_tx);

        // For ACTIVE sessions, we need a barrier to ensure the listener task
        // doesn't accept connections until after registry commit.
        // For PAUSED sessions, no listener task is spawned at all.
        let barrier = if want_active {
            Some(oneshot::channel())
        } else {
            None
        };

        let (barrier_tx, barrier_rx) = match barrier {
            Some((tx, rx)) => (Some(tx), Some(rx)),
            None => (None, None),
        };

        // Spawn the accept loop only for ACTIVE sessions.
        // For PAUSED sessions, no task is spawned; the session registers without a listener.
        let test_dispatch_count = {
            #[cfg(test)]
            {
                let inner = self.inner.lock().unwrap();
                inner.test_dispatch_count.clone()
            }
            #[cfg(not(test))]
            {
                None
            }
        };
        let listener_task = if want_active {
            let task_shared = shared.clone();
            let task_listener = listener.clone();
            let task_sid = sid.clone();
            let task_barrier = barrier_rx.unwrap();
            let task_sock = bound_sock.unwrap();
            Some(runtime.spawn(async move {
                // Wait for registry commit before accepting connections.
                #[cfg(test)]
                await_test_session_barrier(task_barrier, &task_shared).await;
                #[cfg(not(test))]
                let _ = task_barrier.await;
                run_session_listener(
                    task_sid,
                    generation,
                    1,
                    Some(task_sock),
                    Arc::downgrade(&task_shared),
                    task_listener,
                    shutdown_rx,
                    test_dispatch_count,
                )
                .await;
            }))
        } else {
            // PAUSED: no listener task, no socket bound yet.
            None
        };
        let mut listener_task_opt = listener_task;

        // Test hook: wait on the commit gate if set (deterministic race testing).
        #[cfg(test)]
        {
            let gate = self.inner.lock().unwrap().test_commit_gate.clone();
            if let Some(gate) = gate {
                gate.arrive_and_wait(bound_port);
            }
        }

        // Registration: re-validate generation, engine liveness, state, and
        // cap under lock. This is the ATOMIC admission + registration point.
        // The check-and-insert for the session limit is done under the engine
        // lock so two concurrent creates cannot both pass the cap check.
        // A pause/shutdown racing the bind is observed here: the just-bound
        // socket is dropped (task aborted) and the session registers as
        // `Paused`, or everything is dropped with `NotRunning` after a
        // generation change (shutdown/restart).
        enum Outcome {
            Active { port: u16 },
            Paused,
            Gone(ArtiError),
        }
        let outcome = {
            let inner = self.inner.lock().unwrap();
            #[cfg(test)]
            let decision_gate = shared.test_create_decision_gate.lock().unwrap().clone();
            #[cfg(test)]
            if let Some(gate) = &decision_gate {
                gate.arrive_and_wait(bound_port);
            }
            let _transition = shared.transition_gate.lock().unwrap();
            #[cfg(test)]
            if decision_gate.is_some() {
                assert!(
                    shared.transition_gate.try_lock().is_err(),
                    "final create transaction must own transition gate"
                );
            }
            if !Arc::ptr_eq(&inner.shared, &shared)
                || inner.generation != generation
                || shared.client_epoch.load(Ordering::SeqCst) != client_epoch
            {
                Outcome::Gone(ArtiError::NotRunning)
            } else {
                match shared.engine_state() {
                    TorState::Running if want_active && bound_port.is_some() => {
                        let mut sessions = shared.sessions.lock().unwrap();
                        if sessions.len() >= MAX_SESSIONS {
                            Outcome::Gone(ArtiError::Runtime {
                                msg: "session limit reached".into(),
                            })
                        } else {
                            let port = bound_port.unwrap();
                            let entry = SessionRuntime {
                                generation,
                                isolated,
                                state: SessionState::Active,
                                status_revision: 1,
                                port: Some(port),
                                shutdown_tx: shutdown_tx_opt.take(),
                                listener_task: listener_task_opt.take(),
                                connections: Vec::new(),
                                status_listener: Some(listener.clone()),
                            };
                            sessions.insert(sid.clone(), entry);
                            Outcome::Active { port }
                        }
                    }
                    TorState::Running | TorState::Paused => {
                        let mut sessions = shared.sessions.lock().unwrap();
                        if sessions.len() >= MAX_SESSIONS {
                            Outcome::Gone(ArtiError::Runtime {
                                msg: "session limit reached".into(),
                            })
                        } else {
                            let entry = SessionRuntime {
                                generation,
                                isolated,
                                state: SessionState::Paused,
                                status_revision: 1,
                                port: None,
                                shutdown_tx: None,
                                listener_task: None,
                                connections: Vec::new(),
                                status_listener: Some(listener.clone()),
                            };
                            sessions.insert(sid.clone(), entry);
                            Outcome::Paused
                        }
                    }
                    _ => Outcome::Gone(ArtiError::NotRunning),
                }
            }
        };

        match outcome {
            Outcome::Gone(e) => {
                if let Some(task) = listener_task_opt {
                    task.abort();
                }
                if let Some(tx) = shutdown_tx_opt {
                    let _ = tx.send(());
                }
                if let Some(tx) = barrier_tx {
                    let _ = tx.send(());
                }
                Err(e)
            }
            Outcome::Paused => {
                if let Some(task) = listener_task_opt {
                    task.abort();
                }
                listener.on_session_status(sid.clone(), SessionState::Paused, None, 1);
                Ok(Arc::new(SocksSession {
                    id: sid,
                    generation,
                    weak: Arc::downgrade(&shared),
                }))
            }
            Outcome::Active { port } => {
                listener.on_session_status(sid.clone(), SessionState::Active, Some(port), 1);
                if let Some(tx) = barrier_tx {
                    let _ = tx.send(());
                }
                Ok(Arc::new(SocksSession {
                    id: sid,
                    generation,
                    weak: Arc::downgrade(&shared),
                }))
            }
        }
    }

    /// Snapshot of live (non-`Closed`, non-`Invalidated`) additional sessions.
    /// The root endpoint is never included.
    pub fn list_sessions(&self) -> Vec<SessionInfo> {
        let inner = self.inner.lock().unwrap();
        let sessions = inner.shared.sessions.lock().unwrap();
        sessions
            .iter()
            .map(|(id, entry)| SessionInfo {
                id: id.clone(),
                state: entry.state,
                port: entry.port,
            })
            .collect()
    }

    /// Close one session (`== SocksSession.close_session()`). No-op on
    /// unknown/closed/stale handles; never throws.
    pub fn close_session(&self, session: &SocksSession) {
        close_session_handle(&session.weak, &session.id, session.generation);
    }

    /// Current snapshot for one handle (live, terminal-tombstone, or
    /// `Invalidated` for stale/unknown handles). Never throws.
    pub fn session_status(&self, session: &SocksSession) -> SessionStatusFfi {
        session.status_snapshot()
    }
}

/// Internal signal used by `run_socks` to notify that root bind succeeded
/// and session rebinds may begin.
struct RootBindReady {
    rx: oneshot::Receiver<()>,
}

fn spawn_socks(inner: &mut Inner, socks_port: u16) -> Result<RootBindReady, ArtiError> {
    let runtime = inner.runtime.as_ref().ok_or_else(|| ArtiError::Runtime {
        msg: "no runtime".into(),
    })?;
    let client = inner.shared.client().ok_or(ArtiError::NotRunning)?;
    let shared = inner.shared.clone();
    let (ready_tx, ready_rx) = oneshot::channel();
    let revision = shared.worker_revision.fetch_add(1, Ordering::SeqCst) + 1;
    let task = runtime.spawn(async move {
        if let Err(e) =
            run_socks_worker(client, socks_port, shared.clone(), Some(ready_tx), revision).await
        {
            let pct = if shared.bootstrap_done.load(Ordering::SeqCst) {
                100
            } else {
                0
            };
            // Typed notification first; never rely on parsing the summary.
            shared.notify_worker_error(Some(revision), &e, pct);
        }
    });
    inner.worker = Some(task);
    Ok(RootBindReady { rx: ready_rx })
}

/// Wait for root bind to succeed, then rebind paused sessions.
async fn wait_root_then_rebind_sessions(
    ready: RootBindReady,
    shared: Arc<Shared>,
    handle: tokio::runtime::Handle,
) {
    // Wait for root bind to succeed (signaled by run_socks after reporting RUNNING).
    if ready.rx.await.is_ok() {
        rebind_paused_sessions(&shared, &handle);
    }
}

/// Spawn a cold bootstrap + SOCKS task on the owned runtime, creating the
/// runtime on first use. The task reports typed failures via `on_error`.
fn spawn_cold(inner: &mut Inner, config: ArtiConfig) -> Result<(), ArtiError> {
    #[cfg(test)]
    if inner.test_spawn_cold_failure {
        return Err(ArtiError::Runtime {
            msg: "injected spawn_cold failure".into(),
        });
    }
    *inner.shared.engine_state.lock().unwrap() = TorState::Starting;
    let runtime = match inner.runtime.take() {
        Some(rt) => rt,
        None => tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(|e| ArtiError::Runtime { msg: e.to_string() })?,
    };

    let shared = inner.shared.clone();
    shared.bootstrap_done.store(false, Ordering::SeqCst);
    shared.set_client_under_gate(None);
    shared.bound_port.store(0, Ordering::SeqCst);

    let revision = shared.worker_revision.fetch_add(1, Ordering::SeqCst) + 1;
    let task = runtime.spawn(async move {
        if let Err(e) = cold_start(config, shared.clone(), revision).await {
            shared.notify_worker_error(Some(revision), &e, 0);
        }
    });

    inner.runtime = Some(runtime);
    inner.worker = Some(task);
    Ok(())
}

/// TorClient-defining configuration: changing any of these requires a new
/// TorClient/bootstrap. `socks_port` is deliberately excluded (rebind only).
/// Bridge comparison is exact: non-empty → empty counts as a change.
fn tor_client_config_changed(old: &ArtiConfig, new: &ArtiConfig) -> bool {
    old.data_dir != new.data_dir
        || old.state_dir != new.state_dir
        || old.cache_dir != new.cache_dir
        || old.bridges != new.bridges
}

// ============================================================================
// Cold start + SOCKS
// ============================================================================

fn resolve_dirs(config: &ArtiConfig) -> (PathBuf, PathBuf) {
    let data = PathBuf::from(&config.data_dir);
    let state = config
        .state_dir
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or_else(|| data.join("state"));
    let cache = config
        .cache_dir
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or_else(|| data.join("cache"));
    (state, cache)
}

async fn cold_start(
    config: ArtiConfig,
    shared: Arc<Shared>,
    revision: u64,
) -> Result<(), ArtiError> {
    if !shared.report_worker(revision, TorState::Starting, 0, None, "starting") {
        return Ok(());
    }
    if let Some(l) = shared.listener() {
        l.on_log(format!(
            "starting arti: data_dir={}, socks_port={}",
            config.data_dir, config.socks_port
        ));
    }

    let (state_dir, cache_dir) = resolve_dirs(&config);
    std::fs::create_dir_all(&state_dir).ok();
    std::fs::create_dir_all(&cache_dir).ok();

    let mut builder = TorClientConfigBuilder::from_directories(state_dir, cache_dir);
    if !config.bridges.is_empty() {
        for b in &config.bridges {
            builder
                .bridges()
                .bridges()
                .push(b.parse().map_err(|e| ArtiError::Config {
                    msg: format!("bad bridge line: {e}"),
                })?);
        }
    }
    let tor_config = builder
        .build()
        .map_err(|e| ArtiError::Config { msg: e.to_string() })?;

    if let Some(l) = shared.listener() {
        l.on_log("config built; creating unbootstrapped client".into());
    }

    let client = TorClient::builder()
        .config(tor_config)
        .create_unbootstrapped()
        .map_err(|e| ArtiError::Runtime { msg: e.to_string() })?;
    {
        let _transition = shared.transition_gate.lock().unwrap();
        if shared.worker_revision.load(Ordering::SeqCst) != revision {
            return Ok(());
        }
        shared.set_client_under_gate(Some(client.clone()));
    }

    if let Some(l) = shared.listener() {
        l.on_log("client created; starting bootstrap".into());
    }

    let mut events = client.bootstrap_events();
    // Drive bootstrap and progress reports in THIS task (no detached child):
    // aborting the worker therefore stops all bootstrap work deterministically
    // and cannot leave a leaked progress loop reporting BOOTSTRAPPING forever.
    // Scoped so the bootstrap future (which borrows `client`) is dropped
    // before `client` moves into `run_socks` below.
    {
        let bootstrap_fut = client.bootstrap();
        tokio::pin!(bootstrap_fut);
        let mut events_done = false;
        loop {
            tokio::select! {
                status_opt = events.next(), if !events_done => {
                    match status_opt {
                        Some(status) => {
                            let pct = (status.as_frac() * 100.0).round() as u32;
                            if !shared.report_worker(
                                revision,
                                TorState::Bootstrapping,
                                pct.min(99),
                                None,
                                format!("bootstrapping {pct}%"),
                            ) { return Ok(()); }
                        }
                        None => events_done = true,
                    }
                }
                res = &mut bootstrap_fut => {
                    if let Some(l) = shared.listener() {
                        l.on_log(format!("bootstrap() returned: ok={}", res.is_ok()));
                    }
                    res.map_err(|e| ArtiError::Bootstrap { msg: e.to_string() })?;
                    break;
                }
            }
        }
    }
    {
        let _transition = shared.transition_gate.lock().unwrap();
        if shared.worker_revision.load(Ordering::SeqCst) != revision {
            return Ok(());
        }
        shared.bootstrap_done.store(true, Ordering::SeqCst);
    }
    if let Some(l) = shared.listener() {
        l.on_log("bootstrap complete".into());
    }

    run_socks_worker(client, config.socks_port, shared, None, revision).await
}

#[cfg(test)]
async fn run_socks(
    client: Arc<TorClient<PreferredRuntime>>,
    socks_port: u16,
    shared: Arc<Shared>,
    ready_tx: Option<oneshot::Sender<()>>,
) -> Result<(), ArtiError> {
    let revision = shared.worker_revision.load(Ordering::SeqCst);
    run_socks_worker(client, socks_port, shared, ready_tx, revision).await
}

async fn run_socks_worker(
    client: Arc<TorClient<PreferredRuntime>>,
    socks_port: u16,
    shared: Arc<Shared>,
    ready_tx: Option<oneshot::Sender<()>>,
    revision: u64,
) -> Result<(), ArtiError> {
    let addr = SocketAddr::from(([127, 0, 0, 1], socks_port));
    let socks = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| ArtiError::Bind {
            port: socks_port,
            msg: e.to_string(),
        })?;
    let actual_port = socks
        .local_addr()
        .map_err(|e| ArtiError::Runtime {
            msg: format!("local_addr: {e}"),
        })?
        .port();

    if let Some(l) = shared.listener() {
        l.on_log(format!("SOCKS listening on 127.0.0.1:{actual_port}"));
    }
    let mut shutdown_rx = {
        let _transition = shared.transition_gate.lock().unwrap();
        if shared.worker_revision.load(Ordering::SeqCst) != revision {
            return Ok(());
        }
        let rx = shared.install_socks_shutdown();
        *shared.engine_state.lock().unwrap() = TorState::Running;
        shared.bound_port.store(actual_port, Ordering::SeqCst);
        rx
    };
    if let Some(l) = shared.listener() {
        l.on_status(
            TorState::Running,
            100,
            Some(actual_port),
            "proxy ready".into(),
        );
    }

    // Signal that root bind succeeded; session rebinds may now begin.
    if let Some(tx) = ready_tx {
        let _ = tx.send(());
    }

    loop {
        tokio::select! {
            _ = &mut shutdown_rx => {
                if let Some(l) = shared.listener() {
                    l.on_log("SOCKS shutdown signal".into());
                }
                break;
            }
            accept = socks.accept() => {
                match accept {
                    Ok((stream, _peer)) => {
                        let _transition = shared.transition_gate.lock().unwrap();
                        if shared.worker_revision.load(Ordering::SeqCst) != revision || shared.engine_state() != TorState::Running { break; }
                        let client = client.clone();
                        #[cfg(test)]
                        shared.test_dispatch_clients.lock().unwrap().push((None, Arc::as_ptr(&client) as usize));
                        let conn_shared = shared.clone();
                        // Tracked so pause/shutdown can terminate live streams
                        // (fail-closed Tor OFF); aborted via abort_connections.
                        shared.track_connection(tokio::spawn(async move {
                            if let Err(e) = handle_socks(stream, client).await {
                                if let Some(l) = conn_shared.listener() {
                                    l.on_log(format!("socks conn error: {e}"));
                                }
                            }
                        }));
                    }
                    Err(e) => {
                        if let Some(l) = shared.listener() {
                            l.on_log(format!("socks accept error: {e}"));
                        }
                        break;
                    }
                }
            }
        }
    }
    Ok(())
}

/// Rebind every live `Paused` session after engine resume (or a SOCKS-only
/// root rebind): fresh ephemeral loopback port per session, `Paused →
/// Active(new endpoint)` published atomically.
///
/// Per-session failure policy: the failed session stays `Paused` (null
/// endpoint) with a safe diagnostic log; the engine stays `Running` if root
/// bound; siblings continue. No retry API in Phase 1 — the next normal
/// pause/resume cycle or close/recreate retries. Never escalates into engine
/// `Error`. If the engine is concurrently in a terminal/error state (or the
/// entry vanished via close/shutdown), the just-bound socket is dropped with
/// no publication — no resurrection, no leak.
fn rebind_paused_sessions(shared: &Arc<Shared>, runtime: &tokio::runtime::Handle) {
    // Collect candidates under lock; release before binds/spawns.
    // (Dispatch always uses the registry entry's isolated client, so no
    // client Arc travels with the candidate.)
    let candidates: Vec<(String, u64, u64, Arc<dyn SessionStatusListener>)> = {
        let sessions = shared.sessions.lock().unwrap();
        sessions
            .iter()
            .filter(|(_, e)| e.state == SessionState::Paused)
            .map(|(id, e)| {
                (
                    id.clone(),
                    e.generation,
                    e.status_revision,
                    e.status_listener.clone(),
                )
            })
            .collect::<Vec<_>>()
            .into_iter()
            .filter_map(|(id, gen, rev, l)| l.map(|l| (id, gen, rev, l)))
            .collect()
    };
    for (sid, generation, previous_revision, listener) in candidates {
        let (sock, port) = match std::net::TcpListener::bind("127.0.0.1:0") {
            Ok(s) => match s.local_addr() {
                Ok(a) => {
                    if s.set_nonblocking(true).is_err() {
                        if let Some(l) = shared.listener() {
                            l.on_log(format!("session={sid} rebind failed: nonblocking"));
                        }
                        continue;
                    }
                    (s, a.port())
                }
                Err(e) => {
                    if let Some(l) = shared.listener() {
                        l.on_log(format!("session={sid} rebind failed: local_addr: {e}"));
                    }
                    continue;
                }
            },
            Err(e) => {
                // Optional-session bind failure: logged, secret-free; engine
                // and siblings unaffected.
                if let Some(l) = shared.listener() {
                    l.on_log(format!("session={sid} rebind failed: {e}"));
                }
                continue;
            }
        };
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let (barrier_tx, barrier_rx) = oneshot::channel();
        let task_shared = shared.clone();
        let task_listener = listener.clone();
        let task_sid = sid.clone();
        let listener_task = runtime.spawn(async move {
            #[cfg(test)]
            await_test_session_barrier(barrier_rx, &task_shared).await;
            #[cfg(not(test))]
            let _ = barrier_rx.await;
            run_session_listener(
                task_sid,
                generation,
                previous_revision + 1,
                Some(sock),
                Arc::downgrade(&task_shared),
                task_listener,
                shutdown_rx,
                None,
            )
            .await;
        });
        // Commit under lock: entry still present, generation match, engine
        // still in a state where ACTIVE is honest. A racing close/shutdown or
        // an ERROR entry (root rebind failed asynchronously) drops the socket
        // with no ACTIVE publication.
        let mut shutdown_tx = Some(shutdown_tx);
        let mut listener_task = Some(listener_task);
        #[cfg(test)]
        let decision_gate = shared.test_rebind_decision_gate.lock().unwrap().clone();
        #[cfg(test)]
        if let Some(gate) = &decision_gate {
            gate.arrive_and_wait(Some(port));
        }
        let commit = {
            let _transition = shared.transition_gate.lock().unwrap();
            #[cfg(test)]
            if decision_gate.is_some() {
                assert!(
                    shared.transition_gate.try_lock().is_err(),
                    "final rebind transaction must own transition gate"
                );
            }
            let usable = matches!(shared.engine_state(), TorState::Running);
            let mut sessions = shared.sessions.lock().unwrap();
            match sessions.get_mut(&sid) {
                Some(entry)
                    if entry.generation == generation
                        && entry.state == SessionState::Paused
                        && entry.status_revision == previous_revision
                        && usable =>
                {
                    entry.status_revision += 1;
                    entry.port = Some(port);
                    entry.state = SessionState::Active;
                    entry.shutdown_tx = shutdown_tx.take();
                    entry.listener_task = listener_task.take();
                    Some(entry.status_revision)
                }
                _ => None,
            }
        };
        if let Some(revision) = commit {
            listener.on_session_status(sid, SessionState::Active, Some(port), revision);
            let _ = barrier_tx.send(());
        } else {
            if let Some(h) = listener_task {
                h.abort();
            }
            if let Some(tx) = shutdown_tx {
                let _ = tx.send(());
            }
        }
    }
}

/// A dead listener may demote only the exact ACTIVE instance that owns it.
fn session_listener_failed(weak: &Weak<Shared>, sid: &str, generation: u64, revision: u64) {
    let Some(shared) = weak.upgrade() else { return };
    let pending = {
        let _transition = shared.transition_gate.lock().unwrap();
        let mut sessions = shared.sessions.lock().unwrap();
        match sessions.get_mut(sid) {
            Some(entry)
                if entry.generation == generation
                    && entry.state == SessionState::Active
                    && entry.status_revision == revision =>
            {
                entry.abort_all();
                entry.state = SessionState::Paused;
                entry.port = None;
                entry.status_revision += 1;
                entry
                    .status_listener
                    .clone()
                    .map(|listener| PendingSessionNotification {
                        id: sid.to_owned(),
                        listener,
                        state: SessionState::Paused,
                        port: None,
                        revision: entry.status_revision,
                    })
            }
            _ => None,
        }
    };
    if let Some(n) = pending {
        n.listener
            .on_session_status(n.id, n.state, n.port, n.revision);
    }
}

/// Per-session SOCKS accept loop.
///
/// Each accepted connection is dispatched through THIS session's isolated
/// client (`isolated.connect(target)` — never the root client's), with the
/// connection task tracked in the session's registry entry so session close,
/// engine pause, and shutdown terminate exactly this session's connections.
/// Connections accepted after the entry vanished (close/shutdown race) are
/// dropped immediately (fail-closed).
///
/// `std_listener` is `None` only for sessions created while `Paused` (no
/// listener bound yet; binds on next resume) — the task then exits at once.
async fn run_session_listener(
    sid: String,
    generation: u64,
    listener_revision: u64,
    std_listener: Option<std::net::TcpListener>,
    weak: Weak<Shared>,
    _status_listener: Arc<dyn SessionStatusListener>,
    mut shutdown_rx: oneshot::Receiver<()>,
    _test_dispatch_count: Option<Arc<std::sync::atomic::AtomicUsize>>,
) {
    #[cfg(test)]
    let test_probe = weak
        .upgrade()
        .and_then(|shared| shared.test_listener_probe.lock().unwrap().clone());
    #[cfg(test)]
    let _test_dispatch_count =
        _test_dispatch_count.or_else(|| test_probe.as_ref().map(|p| p.dispatch_count.clone()));
    let socks = match std_listener {
        Some(sl) => match tokio::net::TcpListener::from_std(sl) {
            Ok(t) => t,
            Err(e) => {
                session_listener_failed(&weak, &sid, generation, listener_revision);
                if let Some(shared) = weak.upgrade() {
                    if let Some(l) = shared.listener() {
                        l.on_log(format!(
                            "session={sid} listener convert failed: {}",
                            e.kind()
                        ));
                    }
                }
                return;
            }
        },
        None => return,
    };

    loop {
        tokio::select! {
            _ = &mut shutdown_rx => break,
            accept = async {
                #[cfg(test)]
                if let Some(shared) = weak.upgrade() {
                    let mut injected = shared.test_accept_failure.lock().unwrap();
                    if injected.as_deref() == Some(&sid) {
                        injected.take();
                        return Err(std::io::Error::other("injected accept failure"));
                    }
                }
                socks.accept().await
            } => {
                match accept {
                    Ok((stream, _peer)) => {
                        let Some(shared) = weak.upgrade() else { break };
                        let _transition = shared.transition_gate.lock().unwrap();
                        let usable = shared.engine_state() == TorState::Running;
                        let mut sessions = shared.sessions.lock().unwrap();
                        match sessions.get_mut(&sid) {
                            Some(entry)
                                if entry.generation == generation
                                    && entry.state == SessionState::Active
                                    && entry.status_revision == listener_revision
                                    && usable =>
                            {
                                if let Some(ref counter) = _test_dispatch_count {
                                    counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                                }
                                let client = entry.isolated.clone();
                                #[cfg(test)]
                                shared.test_dispatch_clients.lock().unwrap().push((Some(sid.clone()), Arc::as_ptr(&client) as usize));
                                let log_weak = weak.clone();
                                let log_sid = sid.clone();
                                let handle = tokio::spawn(async move {
                                    if let Err(e) = handle_socks(stream, client).await {
                                        if let Some(shared) = log_weak.upgrade() {
                                            if let Some(l) = shared.listener() {
                                                l.on_log(format!(
                                                    "session={log_sid} conn error: {e}"
                                                ));
                                            }
                                        }
                                    }
                                });
                                entry.connections.retain(|h| !h.is_finished());
                                entry.connections.push(handle);
                                #[cfg(test)]
                                if let Some(probe) = &test_probe {
                                    probe.dispatch_checked.signal();
                                }
                            }
                            _ => {
                                drop(stream);
                                #[cfg(test)]
                                if let Some(probe) = &test_probe {
                                    probe.dispatch_checked.signal();
                                }
                            }
                        }
                    }
                    Err(e) => {
                        session_listener_failed(&weak, &sid, generation, listener_revision);
                        if let Some(shared) = weak.upgrade() {
                            if let Some(l) = shared.listener() {
                                l.on_log(format!("session={sid} accept error: {e}"));
                            }
                        }
                        break;
                    }
                }
            }
        }
    }
}

/// Minimal SOCKS5 CONNECT handler tunnelling through the Tor client.
/// Ported from the proven bitchat android wrapper (CONNECT only).
async fn handle_socks(
    mut stream: tokio::net::TcpStream,
    client: Arc<TorClient<PreferredRuntime>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (host, port) = socks::read_connect_request(&mut stream).await?;

    let tor_stream = match client.connect((host.as_str(), port)).await {
        Ok(s) => s,
        Err(e) => {
            stream
                .write_all(&[0x05, 0x05, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
                .await?;
            return Err(socks::safe_connect_diagnostic(&e).into());
        }
    };
    stream
        .write_all(&[0x05, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
        .await?;

    let (mut cr, mut cw) = stream.split();
    let (mut tr, mut tw) = tor_stream.split();
    let c2t = async { tokio::io::copy(&mut cr, &mut tw).await };
    let t2c = async { tokio::io::copy(&mut tr, &mut cw).await };
    tokio::select! {
        _ = c2t => {}
        _ = t2c => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;
    use std::time::Duration;

    fn test_config() -> ArtiConfig {
        ArtiConfig {
            data_dir: "/tmp/artitor-test".into(),
            socks_port: 0,
            bridges: vec![],
            state_dir: None,
            cache_dir: None,
        }
    }

    #[test]
    fn identical_config_needs_no_new_client() {
        let a = test_config();
        assert!(!tor_client_config_changed(&a, &a));
    }

    #[test]
    fn socks_port_only_needs_no_new_client() {
        let a = test_config();
        let mut b = test_config();
        b.socks_port = 19050;
        assert!(!tor_client_config_changed(&a, &b));
    }

    #[test]
    fn data_dir_change_needs_new_client() {
        let a = test_config();
        let mut b = test_config();
        b.data_dir = "/tmp/other".into();
        assert!(tor_client_config_changed(&a, &b));
    }

    #[test]
    fn state_dir_override_change_needs_new_client() {
        let a = test_config();
        let mut b = test_config();
        b.state_dir = Some("/tmp/other-state".into());
        assert!(tor_client_config_changed(&a, &b));
    }

    #[test]
    fn cache_dir_override_change_needs_new_client() {
        let a = test_config();
        let mut b = test_config();
        b.cache_dir = Some("/tmp/other-cache".into());
        assert!(tor_client_config_changed(&a, &b));
    }

    #[test]
    fn bridges_empty_to_nonempty_needs_new_client() {
        let a = test_config();
        let mut b = test_config();
        b.bridges = vec!["obfs4 1.2.3.4:443 FINGERPRINT".into()];
        assert!(tor_client_config_changed(&a, &b));
    }

    #[test]
    fn bridges_nonempty_to_empty_needs_new_client() {
        // Regression: the old resume path silently kept the bootstrapped client
        // (built with bridges) when the new config cleared them.
        let mut a = test_config();
        a.bridges = vec!["obfs4 1.2.3.4:443 FINGERPRINT".into()];
        let b = test_config();
        assert!(tor_client_config_changed(&a, &b));
    }

    #[test]
    fn same_bridges_need_no_new_client() {
        let mut a = test_config();
        a.bridges = vec!["obfs4 1.2.3.4:443 FINGERPRINT".into()];
        let mut b = test_config();
        b.bridges = a.bridges.clone();
        assert!(!tor_client_config_changed(&a, &b));
    }

    #[test]
    fn error_detail_preserves_declared_types() {
        // No string parsing: kind discriminant + typed port carry the type.
        let cases: Vec<(ArtiError, ErrorKind, Option<u16>)> = vec![
            (ArtiError::AlreadyRunning, ErrorKind::AlreadyRunning, None),
            (ArtiError::NotRunning, ErrorKind::NotRunning, None),
            (
                ArtiError::Config { msg: "bad".into() },
                ErrorKind::Config,
                None,
            ),
            (
                ArtiError::Bind {
                    port: 9050,
                    msg: "taken".into(),
                },
                ErrorKind::Bind,
                Some(9050),
            ),
            (
                ArtiError::Bootstrap {
                    msg: "no consensus".into(),
                },
                ErrorKind::Bootstrap,
                None,
            ),
            (
                ArtiError::Runtime { msg: "boom".into() },
                ErrorKind::Runtime,
                None,
            ),
        ];
        for (err, kind, port) in cases {
            let d = ArtiErrorDetail::from(&err);
            assert_eq!(d.kind, kind, "kind for {err}");
            assert_eq!(d.port, port, "port for {err}");
            assert!(!d.msg.is_empty(), "msg for {err}");
        }
    }

    #[test]
    fn resolve_dirs_defaults_and_overrides() {
        let c = test_config();
        let (s, ca) = resolve_dirs(&c);
        assert_eq!(s, PathBuf::from("/tmp/artitor-test/state"));
        assert_eq!(ca, PathBuf::from("/tmp/artitor-test/cache"));

        let mut c2 = test_config();
        c2.state_dir = Some("/s".into());
        c2.cache_dir = Some("/c".into());
        let (s2, c3) = resolve_dirs(&c2);
        assert_eq!(s2, PathBuf::from("/s"));
        assert_eq!(c3, PathBuf::from("/c"));
    }

    struct Recorder {
        statuses: StdMutex<Vec<(TorState, u32, Option<u16>, String)>>,
        errors: StdMutex<Vec<ArtiErrorDetail>>,
    }

    impl StatusListener for Recorder {
        fn on_status(
            &self,
            state: TorState,
            bootstrap_percent: u32,
            socks_port: Option<u16>,
            summary: String,
        ) {
            self.statuses
                .lock()
                .unwrap()
                .push((state, bootstrap_percent, socks_port, summary));
        }
        fn on_log(&self, _line: String) {}
        fn on_error(&self, error: ArtiErrorDetail) {
            self.errors.lock().unwrap().push(error);
        }
    }

    fn test_client(name: &str) -> Arc<TorClient<PreferredRuntime>> {
        // Unique dirs per test: parallel tests must not share dir.sqlite3.
        let dir = std::env::temp_dir().join(format!(
            "artitor-unit-{}-{}-{:?}",
            std::process::id(),
            name,
            std::thread::current().id()
        ));
        let cfg = TorClientConfigBuilder::from_directories(dir.join("state"), dir.join("cache"))
            .build()
            .expect("test config builds offline");
        TorClient::builder()
            .config(cfg)
            .create_unbootstrapped()
            .expect("unbootstrapped client needs no network")
    }

    fn test_shared(rec: Arc<Recorder>) -> Arc<Shared> {
        let shared = Shared::new(1);
        shared.set_listener(rec);
        shared
    }

    #[tokio::test]
    async fn fixed_port_collision_is_typed_bind() {
        // Occupy a loopback port, then SOCKS-bind the same port: must fail
        // with ArtiError::Bind carrying the port — no Tor network involved.
        let occupied = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("occupy loopback port");
        let port = occupied.local_addr().unwrap().port();
        let rec = Arc::new(Recorder {
            statuses: StdMutex::new(vec![]),
            errors: StdMutex::new(vec![]),
        });
        let shared = test_shared(rec);
        let err = run_socks(test_client("bind-collision"), port, shared, None)
            .await
            .expect_err("colliding port must fail");
        match err {
            ArtiError::Bind { port: p, .. } => assert_eq!(p, port),
            other => panic!("expected Bind, got {other}"),
        }
    }

    #[tokio::test]
    async fn ephemeral_port_binds_and_reports_running() {
        let rec = Arc::new(Recorder {
            statuses: StdMutex::new(vec![]),
            errors: StdMutex::new(vec![]),
        });
        let shared = test_shared(rec.clone());
        let client = test_client("ephemeral");
        let task = tokio::spawn(async move { run_socks(client, 0, shared, None).await });
        // Wait for the listener to report a real port (no Tor bootstrap needed
        // for the accept loop itself).
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        let actual = loop {
            let p = rec
                .statuses
                .lock()
                .unwrap()
                .iter()
                .filter(|(s, _, _, _)| *s == TorState::Running)
                .filter_map(|(_, _, port, _)| *port)
                .next();
            if let Some(p) = p {
                break p;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "ephemeral SOCKS never reported Running"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        };
        assert_ne!(actual, 0, "ephemeral port must be non-zero");
        task.abort();
        let _ = tokio::time::timeout(Duration::from_secs(5), task).await;
    }

    #[test]
    fn notify_error_delivers_typed_detail_before_status() {
        #[derive(Debug, PartialEq)]
        enum RecordedEvent {
            Error(ErrorKind),
            Status(TorState),
        }
        struct OrderedRecorder(StdMutex<Vec<RecordedEvent>>);
        impl StatusListener for OrderedRecorder {
            fn on_status(&self, state: TorState, _: u32, _: Option<u16>, _: String) {
                self.0.lock().unwrap().push(RecordedEvent::Status(state));
            }
            fn on_error(&self, error: ArtiErrorDetail) {
                self.0
                    .lock()
                    .unwrap()
                    .push(RecordedEvent::Error(error.kind));
            }
            fn on_log(&self, _: String) {}
        }
        let rec = Arc::new(OrderedRecorder(StdMutex::new(vec![])));
        let shared = Shared::new(1);
        shared.set_listener(rec.clone());
        shared.notify_error(
            &ArtiError::Bootstrap {
                msg: "no consensus".into(),
            },
            42,
        );
        assert_eq!(
            *rec.0.lock().unwrap(),
            vec![
                RecordedEvent::Error(ErrorKind::Bootstrap),
                RecordedEvent::Status(TorState::Error)
            ]
        );
    }

    // ========================================================================
    // 0.3 Phase 1 — isolation session foundation tests
    // ========================================================================

    #[derive(Clone)]
    struct SessionRecorder {
        events: Arc<StdMutex<Vec<(String, SessionState, Option<u16>)>>>,
    }

    impl SessionRecorder {
        fn new() -> Self {
            Self {
                events: Arc::new(StdMutex::new(Vec::new())),
            }
        }

        fn events(&self) -> Vec<(String, SessionState, Option<u16>)> {
            self.events.lock().unwrap().clone()
        }

        fn last_for(&self, sid: &str) -> Option<(SessionState, Option<u16>)> {
            self.events
                .lock()
                .unwrap()
                .iter()
                .rev()
                .find(|(id, _, _)| id == sid)
                .map(|(_, s, p)| (*s, *p))
        }
    }

    impl SessionStatusListener for SessionRecorder {
        fn on_session_status(
            &self,
            session_id: String,
            state: SessionState,
            port: Option<u16>,
            _revision: u64,
        ) {
            self.events.lock().unwrap().push((session_id, state, port));
        }
    }

    /// Engine harness with an owned runtime, a bootstrapped (unbootstrapped
    /// in reality — no Tor network) root client, and the engine reporting
    /// RUNNING. No network involved; SOCKS binds are loopback-local.
    fn running_engine(name: &str) -> (Arc<ArtiTor>, SessionRecorder) {
        let engine = ArtiTor::new();
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("test runtime");
        {
            let mut inner = engine.inner.lock().unwrap();
            inner.runtime = Some(rt);
            let shared = inner.shared.clone();
            let _runtime_context = inner.runtime.as_ref().unwrap().enter();
            shared.set_client(Some(test_client(name)));
            shared.bootstrap_done.store(true, Ordering::SeqCst);
            *shared.engine_state.lock().unwrap() = TorState::Running;
        }
        (engine, SessionRecorder::new())
    }

    /// Re-arm an engine whose runtime was consumed by `shutdown()` (models
    /// restart into a new generation): fresh runtime + fresh root client on
    /// the CURRENT (post-shutdown) generation.
    fn revive_engine(engine: &ArtiTor, name: &str) {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("test runtime");
        let mut inner = engine.inner.lock().unwrap();
        inner.runtime = Some(rt);
        let shared = inner.shared.clone();
        shared.set_client(Some(test_client(name)));
        shared.bootstrap_done.store(true, Ordering::SeqCst);
        *shared.engine_state.lock().unwrap() = TorState::Running;
    }

    async fn wait_for_session_state(
        rec: &SessionRecorder,
        sid: &str,
        want: SessionState,
        timeout: Duration,
    ) -> Option<u16> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if let Some((state, port)) = rec.last_for(sid) {
                if state == want {
                    return port;
                }
            }
            if tokio::time::Instant::now() >= deadline {
                return None;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    // --- Isolation primitive ------------------------------------------------

    #[test]
    fn isolated_client_handles_are_distinct() {
        // Each session derives exactly one `root.isolated_client()`; the
        // handles must be pairwise distinct objects (distinct owner tokens by
        // upstream construction, client.rs:1466).
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("test runtime");
        rt.block_on(async {
            let root = test_client("iso-distinct");
            let a = root.isolated_client();
            let b = root.isolated_client();
            assert!(!Arc::ptr_eq(&root, &a), "session A != root");
            assert!(!Arc::ptr_eq(&root, &b), "session B != root");
            assert!(!Arc::ptr_eq(&a, &b), "session A != session B");
        });
    }

    #[test]
    fn fresh_isolation_tokens_are_mutually_incompatible() {
        // Lowest-level accessible primitive: distinct owner tokens never
        // share circuits; a token is always compatible with itself.
        use arti_client::isolation::Isolation;
        use arti_client::IsolationToken;
        let t1 = IsolationToken::new();
        let t2 = IsolationToken::new();
        assert!(t1.compatible(&t1), "reflexive");
        assert!(!t1.compatible(&t2), "distinct owners incompatible");
        assert!(!t2.compatible(&t1), "symmetric");
        assert_ne!(t1, IsolationToken::no_isolation());
        assert_ne!(t2, IsolationToken::no_isolation());
    }

    #[test]
    fn stream_isolation_requires_matching_owner_and_stream() {
        // Models the frozen conjunction rule (isolation.rs: two streams share
        // a circuit iff owner token AND stream prefs both match):
        // - same owner + same stream -> MAY share (within one session);
        // - different owners + SAME stream token -> MUST NOT share
        //   (root vs session, session A vs session B).
        use arti_client::isolation::{Isolation, StreamIsolation};
        use arti_client::IsolationToken;
        let owner_root = IsolationToken::new();
        let owner_a = IsolationToken::new();
        let owner_b = IsolationToken::new();
        let stream_token = IsolationToken::new();
        let mk = |owner: IsolationToken| {
            StreamIsolation::builder()
                .owner_token(owner)
                .stream_isolation(Box::new(stream_token) as Box<dyn Isolation>)
                .build()
                .expect("StreamIsolation builds")
        };
        let root_iso = mk(owner_root);
        let a_iso = mk(owner_a);
        let b_iso = mk(owner_b);
        let a_iso_again = mk(owner_a);
        assert!(
            a_iso.compatible(&a_iso_again),
            "streams inside one session MAY share circuits"
        );
        assert!(
            !a_iso.compatible(&b_iso),
            "session A vs session B MUST NOT share circuits"
        );
        assert!(
            !root_iso.compatible(&a_iso),
            "root vs session MUST NOT share circuits"
        );
        assert!(
            !b_iso.compatible(&root_iso),
            "session vs root MUST NOT share circuits (symmetric)"
        );
    }

    // --- Session id ----------------------------------------------------------

    #[test]
    fn session_ids_are_unique_and_opaque() {
        let a = next_session_id();
        let b = next_session_id();
        assert_ne!(a, b, "unique within process generation");
        assert!(!a.is_empty() && !b.is_empty());
    }

    // --- Registry / lifecycle -------------------------------------------------

    #[tokio::test]
    async fn create_while_running_binds_and_registers_active() {
        let (engine, rec) = running_engine("create-active");
        let session = engine
            .create_session(Box::new(rec.clone()))
            .expect("create while RUNNING");
        let snap = engine.session_status(&session);
        assert_eq!(snap.state, SessionState::Active);
        let port = snap.port.expect("ACTIVE carries an endpoint");
        assert_ne!(port, 0);
        // Loopback listener really accepts.
        let probe = tokio::net::TcpStream::connect(("127.0.0.1", port)).await;
        assert!(probe.is_ok(), "session listener must accept");
        drop(probe);
        // Live snapshot lists exactly this session.
        let list = engine.list_sessions();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, session.id());
        assert_eq!(list[0].state, SessionState::Active);
        assert_eq!(list[0].port, Some(port));
        // Registry holds a distinct isolated client (dispatch proof by
        // construction: the accept loop clones the entry's client).
        let (root, isolated) = {
            let inner = engine.inner.lock().unwrap();
            let sessions = inner.shared.sessions.lock().unwrap();
            let entry = sessions.get(&session.id()).expect("registered");
            assert_eq!(entry.generation, session.generation());
            (inner.shared.client().unwrap(), entry.isolated.clone())
        };
        assert!(!Arc::ptr_eq(&root, &isolated));
        engine.close_session(&session);
        engine.shutdown();
    }

    #[tokio::test]
    async fn create_while_paused_registers_paused_without_listener() {
        let (engine, rec) = running_engine("create-paused");
        engine.pause();
        assert_eq!(
            engine.inner.lock().unwrap().shared.engine_state(),
            TorState::Paused
        );
        let session = engine
            .create_session(Box::new(rec.clone()))
            .expect("create while PAUSED");
        let snap = engine.session_status(&session);
        assert_eq!(snap.state, SessionState::Paused);
        assert_eq!(snap.port, None, "PAUSED carries no endpoint");
        engine.close_session(&session);
        engine.shutdown();
    }

    #[tokio::test]
    async fn create_rejected_without_live_client() {
        let (engine, rec) = running_engine("create-rejected");
        for state in [
            TorState::Off,
            TorState::Starting,
            TorState::Bootstrapping,
            TorState::Stopping,
            TorState::Error,
        ] {
            {
                let inner = engine.inner.lock().unwrap();
                *inner.shared.engine_state.lock().unwrap() = state;
            }
            let err = engine
                .create_session(Box::new(rec.clone()))
                .expect_err("must reject");
            assert!(
                matches!(err, ArtiError::NotRunning),
                "state {state:?} must reject with NotRunning, got {err}"
            );
        }
        // Rejected creations leak nothing.
        assert!(engine.list_sessions().is_empty());
        engine.shutdown();
    }

    #[tokio::test]
    async fn close_is_idempotent_and_never_throws() {
        let (engine, rec) = running_engine("close-idem");
        let session = engine.create_session(Box::new(rec.clone())).unwrap();
        let sid = session.id();
        session.close_session();
        assert_eq!(engine.session_status(&session).state, SessionState::Closed);
        // Second close: no-op, no second publication.
        session.close_session();
        engine.close_session(&session);
        let closed_events = rec
            .events()
            .into_iter()
            .filter(|(id, s, _)| id == &sid && *s == SessionState::Closed)
            .count();
        assert_eq!(closed_events, 1, "CLOSED published exactly once");
        assert!(engine.list_sessions().is_empty(), "closed leaves the list");
        engine.shutdown();
    }

    #[tokio::test]
    async fn unknown_and_stale_handles_are_noop_invalidated() {
        let (engine, rec) = running_engine("stale-noop");
        let rt_handle = {
            let inner = engine.inner.lock().unwrap();
            inner.runtime.as_ref().unwrap().handle().clone()
        };
        let _ = rt_handle;
        let live = engine
            .create_session(Box::new(rec.clone()))
            .expect("live session");
        let live_id = live.id();
        let weak = {
            let inner = engine.inner.lock().unwrap();
            Arc::downgrade(&inner.shared)
        };

        // Unknown id: no-op close, INVALIDATED status.
        let ghost = SocksSession {
            id: "sess-deadbeefdeadbeef".into(),
            generation: live.generation(),
            weak: weak.clone(),
        };
        ghost.close_session();
        assert_eq!(ghost.status_snapshot().state, SessionState::Invalidated);

        // Right id, wrong generation (id collision across restart): never
        // attaches, never disturbs the live entry.
        let impostor = SocksSession {
            id: live_id.clone(),
            generation: live.generation().wrapping_add(1000),
            weak: weak.clone(),
        };
        assert_eq!(impostor.status_snapshot().state, SessionState::Invalidated);
        impostor.close_session();
        assert_eq!(
            engine.session_status(&live).state,
            SessionState::Active,
            "impostor close must not disturb the live entry"
        );

        // Dead engine (Weak upgrade failure): no-op, INVALIDATED.
        let orphan = SocksSession {
            id: live_id.clone(),
            generation: live.generation(),
            weak: Weak::new(),
        };
        orphan.close_session();
        assert_eq!(orphan.status_snapshot().state, SessionState::Invalidated);

        engine.close_session(&live);
        engine.shutdown();
    }

    #[tokio::test]
    async fn shutdown_invalidates_all_and_releases_strong_refs() {
        let (engine, rec) = running_engine("shutdown-inv");
        let a = engine.create_session(Box::new(rec.clone())).unwrap();
        let b = engine.create_session(Box::new(rec.clone())).unwrap();
        let (gen, old_shared, weak_iso_a, weak_root) = {
            let inner = engine.inner.lock().unwrap();
            let sessions = inner.shared.sessions.lock().unwrap();
            let iso: Arc<TorClient<PreferredRuntime>> =
                sessions.get(&a.id()).unwrap().isolated.clone();
            let wa = Arc::downgrade(&iso);
            drop(iso);
            let root: Arc<TorClient<PreferredRuntime>> = inner.shared.client().clone().unwrap();
            let wr = Arc::downgrade(&root);
            drop(root);
            (inner.generation, inner.shared.clone(), wa, wr)
        };
        engine.shutdown();
        // Registry drained and tombstoned on the old generation (checked
        // while the test's own clone keeps it observable).
        assert!(
            old_shared.sessions.lock().unwrap().is_empty(),
            "registry drained"
        );
        // Aborted listener tasks release their `Shared` clones promptly but
        // asynchronously; wait (bounded) for full release rather than
        // asserting on a single yield.
        // Note: a.weak/b.weak point to old_shared which we still hold, so they
        // won't fail. We only wait for engine-owned strong refs (isolated clients, root).
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            if weak_iso_a.upgrade().is_none() && weak_root.upgrade().is_none() {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "engine-owned strong refs must be released after shutdown"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        // Retained handles keep nothing alive: Weak upgrades fail while the
        // SocksSession objects themselves are still held.
        // Note: a.weak/b.weak point to old_shared which we still hold.
        assert!(weak_iso_a.upgrade().is_none());
        assert!(weak_root.upgrade().is_none());
        assert_eq!(a.status_snapshot().state, SessionState::Invalidated);
        assert_eq!(b.status_snapshot().state, SessionState::Invalidated);
        // Engine-owned strong refs are gone (isolated clients + root).
        assert!(
            weak_iso_a.upgrade().is_none(),
            "session strong client refs must reach zero"
        );
        assert!(weak_root.upgrade().is_none(), "root client must be dropped");
        // Generation bumped; old (id, generation) never validates again.
        let new_gen = engine.inner.lock().unwrap().generation;
        assert_ne!(new_gen, gen, "cold generation differs");
        assert_eq!(
            old_shared
                .session_snapshot_for(&a.id(), a.generation())
                .state,
            SessionState::Invalidated
        );
        let _ = rec;
    }

    #[tokio::test]
    async fn restart_never_resurrects_stale_sessions() {
        let (engine, rec) = running_engine("restart-stale");
        let a = engine.create_session(Box::new(rec.clone())).unwrap();
        let a_id = a.id();
        let a_gen = a.generation();
        engine.shutdown();
        // Restart into a new generation (shutdown + cold start, no session
        // restoration by design).
        revive_engine(&engine, "restart-stale-2");
        assert_eq!(
            a.status_snapshot().state,
            SessionState::Invalidated,
            "stale handle stays terminal"
        );
        a.close_session();
        assert_eq!(
            a.status_snapshot().state,
            SessionState::Invalidated,
            "close on INVALIDATED stays INVALIDATED (never CLOSED)"
        );
        let c = engine.create_session(Box::new(rec.clone())).unwrap();
        assert_ne!(c.id(), a_id, "fresh ids under the new generation");
        assert!(engine.list_sessions().iter().all(|s| s.id != a_id));
        // The stale (id, generation) pair validates nowhere.
        let inner = engine.inner.lock().unwrap();
        assert_eq!(
            inner.shared.session_snapshot_for(&a_id, a_gen).state,
            SessionState::Invalidated
        );
        drop(inner);
        engine.close_session(&c);
        engine.shutdown();
    }

    #[tokio::test]
    async fn session_limit_is_enforced_without_engine_damage() {
        let (engine, rec) = running_engine("sess-limit");
        let mut handles = Vec::new();
        for _ in 0..MAX_SESSIONS {
            handles.push(engine.create_session(Box::new(rec.clone())).unwrap());
        }
        assert_eq!(engine.list_sessions().len(), MAX_SESSIONS);
        let err = engine
            .create_session(Box::new(rec.clone()))
            .expect_err("over the cap");
        match err {
            ArtiError::Runtime { msg } => assert!(
                msg.contains("session limit reached"),
                "unexpected message: {msg}"
            ),
            other => panic!("expected Runtime limit, got {other}"),
        }
        // Engine untouched: still RUNNING, existing sessions live.
        assert_eq!(
            engine.inner.lock().unwrap().shared.engine_state(),
            TorState::Running
        );
        assert_eq!(engine.list_sessions().len(), MAX_SESSIONS);
        // Closing one frees a slot.
        handles.pop().unwrap().close_session();
        let _replacement = engine
            .create_session(Box::new(rec.clone()))
            .expect("slot freed");
        for h in handles {
            h.close_session();
        }
        engine.shutdown();
    }

    // --- Connection ownership --------------------------------------------------

    #[tokio::test]
    async fn close_aborts_only_own_connections() {
        let (engine, rec) = running_engine("conn-own");
        let a = engine.create_session(Box::new(rec.clone())).unwrap();
        let b = engine.create_session(Box::new(rec.clone())).unwrap();

        // Plant one pending connection task per surface: root, A, B.
        let (a_tx, a_rx) = oneshot::channel::<()>();
        let (b_tx, b_rx) = oneshot::channel::<()>();
        let (r_tx, r_rx) = oneshot::channel::<()>();
        {
            let inner = engine.inner.lock().unwrap();
            inner.shared.track_connection(tokio::spawn(async move {
                let _ = r_rx.await;
            }));
            let mut sessions = inner.shared.sessions.lock().unwrap();
            sessions
                .get_mut(&a.id())
                .unwrap()
                .connections
                .push(tokio::spawn(async move {
                    let _ = a_rx.await;
                }));
            sessions
                .get_mut(&b.id())
                .unwrap()
                .connections
                .push(tokio::spawn(async move {
                    let _ = b_rx.await;
                }));
        }

        a.close_session();
        tokio::time::sleep(Duration::from_millis(100)).await;
        {
            let inner = engine.inner.lock().unwrap();
            let sessions = inner.shared.sessions.lock().unwrap();
            assert!(!sessions.contains_key(&a.id()), "A removed");
            let b_entry = sessions.get(&b.id()).expect("B retained");
            assert_eq!(b_entry.connections.len(), 1);
            assert!(
                !b_entry.connections[0].is_finished(),
                "B connection survives A's close"
            );
            let root_conns = inner.shared.connections.lock().unwrap();
            assert_eq!(root_conns.len(), 1);
            assert!(!root_conns[0].is_finished(), "root survives A's close");
        }
        let _ = (r_tx, a_tx, b_tx);
        b.close_session();
        engine.shutdown();
    }

    // --- Pause / resume ---------------------------------------------------------

    #[tokio::test]
    async fn pause_freezes_sessions_synchronously_and_keeps_identity() {
        let (engine, rec) = running_engine("pause-freeze");
        let a = engine.create_session(Box::new(rec.clone())).unwrap();
        let b = engine.create_session(Box::new(rec.clone())).unwrap();
        let port_a = engine.session_status(&a).port.expect("A active");
        let port_b = engine.session_status(&b).port.expect("B active");
        // Retain client Arcs to prove identity preservation across pause.
        let (iso_a_before, iso_b_before) = {
            let inner = engine.inner.lock().unwrap();
            let sessions = inner.shared.sessions.lock().unwrap();
            (
                sessions.get(&a.id()).unwrap().isolated.clone(),
                sessions.get(&b.id()).unwrap().isolated.clone(),
            )
        };

        engine.pause();

        // Synchronous transition before return: no ACTIVE session remains.
        assert_eq!(a.status_snapshot().state, SessionState::Paused);
        assert_eq!(b.status_snapshot().state, SessionState::Paused);
        assert_eq!(a.status_snapshot().port, None);
        assert_eq!(b.status_snapshot().port, None);
        {
            let inner = engine.inner.lock().unwrap();
            let sessions = inner.shared.sessions.lock().unwrap();
            for sid in [&a.id(), &b.id()] {
                let entry = sessions.get(sid).expect("retained across pause");
                assert_eq!(entry.state, SessionState::Paused);
                assert_eq!(entry.port, None);
            }
            // Isolation identity preserved: same isolated Arcs retained.
            assert!(Arc::ptr_eq(
                &iso_a_before,
                &sessions.get(&a.id()).unwrap().isolated
            ));
            assert!(Arc::ptr_eq(
                &iso_b_before,
                &sessions.get(&b.id()).unwrap().isolated
            ));
            // Root client retained (no re-bootstrap on resume).
            assert!(inner.shared.client().is_some());
        }
        // Old ports refuse connections (listeners dropped).
        for port in [port_a, port_b] {
            let mut refused = false;
            for _ in 0..20 {
                match tokio::net::TcpStream::connect(("127.0.0.1", port)).await {
                    Ok(_) => tokio::time::sleep(Duration::from_millis(50)).await,
                    Err(_) => {
                        refused = true;
                        break;
                    }
                }
            }
            assert!(refused, "old session port {port} must refuse connections");
        }
        // PAUSED publications observed (Active -> Paused transitions).
        for sid in [a.id(), b.id()] {
            assert!(
                rec.events()
                    .iter()
                    .any(|(id, s, p)| id == &sid && *s == SessionState::Paused && *p == None),
                "missing PAUSED publication for {sid}"
            );
        }
        engine.shutdown();
    }

    #[tokio::test]
    async fn resume_rebinds_sessions_without_rebootstrap() {
        let (engine, rec) = running_engine("resume-rebind");
        let a = engine.create_session(Box::new(rec.clone())).unwrap();
        let port_before = engine.session_status(&a).port.expect("A active");
        let root_before = {
            let inner = engine.inner.lock().unwrap();
            inner.shared.client().unwrap()
        };
        let engine_listener = Arc::new(Recorder {
            statuses: StdMutex::new(vec![]),
            errors: StdMutex::new(vec![]),
        });

        engine.pause();
        assert_eq!(a.status_snapshot().state, SessionState::Paused);

        engine
            .resume(Box::new(Recorder {
                statuses: StdMutex::new(vec![]),
                errors: StdMutex::new(vec![]),
            }))
            .expect("resume");
        let _ = engine_listener;

        // Session returns ACTIVE with an endpoint (port MAY differ — never
        // asserted equal); wall clock far below a cold bootstrap, same root
        // client retained (no re-bootstrap).
        let port_after =
            wait_for_session_state(&rec, &a.id(), SessionState::Active, Duration::from_secs(5))
                .await
                .expect("session rebinds to ACTIVE");
        assert_ne!(port_after, 0);
        let _ = port_before;
        {
            let inner = engine.inner.lock().unwrap();
            assert!(Arc::ptr_eq(&root_before, &inner.shared.client().unwrap()));
            assert!(inner.shared.bootstrap_done.load(Ordering::SeqCst));
        }
        // New endpoint accepts connections.
        let probe = tokio::net::TcpStream::connect(("127.0.0.1", port_after)).await;
        assert!(probe.is_ok(), "rebound session listener must accept");
        engine.shutdown();
    }

    #[tokio::test]
    async fn engine_error_freezes_sessions_paused_with_null_endpoints() {
        // Root failure -> ERROR: no session may expose ACTIVE; endpoints null;
        // identities kept (PAUSED, not INVALIDATED).
        let (engine, rec) = running_engine("error-freeze");
        let a = engine.create_session(Box::new(rec.clone())).unwrap();
        let shared = engine.inner.lock().unwrap().shared.clone();
        shared.notify_error(
            &ArtiError::Bind {
                port: 9050,
                msg: "address in use".into(),
            },
            100,
        );
        assert_eq!(shared.engine_state(), TorState::Error);
        assert_eq!(a.status_snapshot().state, SessionState::Paused);
        assert_eq!(a.status_snapshot().port, None);
        assert!(engine.list_sessions().iter().all(|s| s.port.is_none()));
        // Identities kept: entry + isolated client still present.
        assert!(shared.sessions.lock().unwrap().contains_key(&a.id()));
        engine.shutdown();
        assert_eq!(a.status_snapshot().state, SessionState::Invalidated);
    }

    #[test]
    fn socks_request_parses_ipv4_target() {
        let buf = [0x05, 0x01, 0x00, 0x01, 1, 2, 3, 4, 0x1F, 0x90];
        assert_eq!(parse_socks_request(&buf, 10), Ok(("1.2.3.4".into(), 8080)));
    }

    #[test]
    fn socks_request_parses_hostname_target() {
        let host = b"example.com";
        let mut buf = vec![0x05, 0x01, 0x00, 0x03, host.len() as u8];
        buf.extend_from_slice(host);
        buf.extend_from_slice(&[0x00, 0x50]);
        let n = buf.len();
        assert_eq!(parse_socks_request(&buf, n), Ok(("example.com".into(), 80)));
    }

    #[test]
    fn socks_request_parses_ipv6_target() {
        // ::1 port 443.
        let mut buf = vec![0x05, 0x01, 0x00, 0x04];
        buf.extend_from_slice(&[0u8; 15]);
        buf.push(1);
        buf.extend_from_slice(&[0x01, 0xBB]);
        assert_eq!(buf.len(), 22);
        assert_eq!(
            parse_socks_request(&buf, 22),
            Ok(("0:0:0:0:0:0:0:1".into(), 443))
        );
    }

    #[test]
    fn socks_request_rejects_non_connect_truncated_and_unknown() {
        // BIND command.
        let bind = [0x05, 0x02, 0x00, 0x01, 127, 0, 0, 1, 0, 80];
        assert_eq!(
            parse_socks_request(&bind, 10),
            Err(SocksRequestError::UnsupportedCommand)
        );
        // Unknown address type.
        let atyp = [0x05, 0x01, 0x00, 0x05, 0, 0, 0, 0, 0, 0];
        assert_eq!(
            parse_socks_request(&atyp, 10),
            Err(SocksRequestError::UnsupportedAtyp)
        );
        // Truncated IPv6.
        let short6 = [0x05, 0x01, 0x00, 0x04, 0, 0, 0, 0, 0, 0];
        assert_eq!(
            parse_socks_request(&short6, 10),
            Err(SocksRequestError::TruncatedIpv6)
        );
        // Short request / bad version / domain overrun: malformed.
        assert_eq!(
            parse_socks_request(&[0x05, 0x01], 2),
            Err(SocksRequestError::Malformed)
        );
        let badver = [0x04, 0x01, 0x00, 0x01, 1, 2, 3, 4, 0, 80];
        assert_eq!(
            parse_socks_request(&badver, 10),
            Err(SocksRequestError::Malformed)
        );
        let mut over = vec![0x05, 0x01, 0x00, 0x03, 20, b'a', b'b'];
        over.extend_from_slice(&[0x00, 0x50]);
        let n = over.len();
        assert_eq!(
            parse_socks_request(&over, n),
            Err(SocksRequestError::Malformed)
        );
    }

    #[tokio::test]
    async fn socks_rejects_non_connect_command_without_tor() {
        // Wire-level rejection happens BEFORE any Tor use: no bootstrap, no
        // network. BIND must draw 0x07.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("loopback");
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (sock, _) = listener.accept().await.expect("accept");
            handle_socks(sock, test_client("socks-reject")).await
        });
        let mut client = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .expect("connect");
        #[cfg(test)]
        use tokio::io::AsyncReadExt;
        use tokio::io::AsyncWriteExt;
        client.write_all(&[0x05, 0x01, 0x00]).await.unwrap();
        let mut greeting = [0u8; 2];
        client.read_exact(&mut greeting).await.unwrap();
        assert_eq!(greeting, [0x05, 0x00]);
        client
            .write_all(&[0x05, 0x02, 0x00, 0x01, 127, 0, 0, 1, 0, 80])
            .await
            .unwrap();
        let mut reply = [0u8; 10];
        client.read_exact(&mut reply).await.unwrap();
        assert_eq!(reply[1], 0x07, "BIND must be refused with 0x07");
        let res = server.await.expect("server task");
        assert!(res.is_err(), "handler reports the rejection");
    }

    #[test]
    fn abort_connections_drains_tracked_tasks() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let rec = Arc::new(Recorder {
                statuses: StdMutex::new(vec![]),
                errors: StdMutex::new(vec![]),
            });
            let shared = test_shared(rec);
            assert!(
                shared.connections.lock().unwrap().is_empty(),
                "starts untracked"
            );
            let (tx, rx) = oneshot::channel::<()>();
            shared.track_connection(tokio::spawn(async move {
                let _ = rx.await;
            }));
            assert_eq!(shared.connections.lock().unwrap().len(), 1);
            shared.abort_connections();
            assert!(
                shared.connections.lock().unwrap().is_empty(),
                "pause/shutdown must not retain connection tasks"
            );
            let _ = tx;
        });
    }

    // ========================================================================
    // Concurrency / race remediation tests
    // ========================================================================

    #[derive(Clone)]
    struct ReentrantSessionListener {
        engine: Arc<ArtiTor>,
        events: Arc<StdMutex<Vec<(String, SessionState, Option<u16>)>>>,
    }

    impl SessionStatusListener for ReentrantSessionListener {
        fn on_session_status(
            &self,
            session_id: String,
            state: SessionState,
            port: Option<u16>,
            _revision: u64,
        ) {
            self.events.lock().unwrap().push((session_id, state, port));
            let _ = self.engine.has_client();
            let _ = self.engine.socks_port();
        }
    }

    #[derive(Clone)]
    struct ReentrantEngineListener {
        engine: Weak<ArtiTor>,
        statuses: Arc<StdMutex<Vec<TorState>>>,
        logs: Arc<StdMutex<Vec<String>>>,
        errors: Arc<StdMutex<Vec<ArtiErrorDetail>>>,
    }

    impl ReentrantEngineListener {
        fn new(engine: &Arc<ArtiTor>) -> Self {
            Self {
                engine: Arc::downgrade(engine),
                statuses: Arc::new(StdMutex::new(vec![])),
                logs: Arc::new(StdMutex::new(vec![])),
                errors: Arc::new(StdMutex::new(vec![])),
            }
        }

        fn reenter(&self) {
            if let Some(engine) = self.engine.upgrade() {
                let _ = engine.has_client();
                let _ = engine.socks_port();
            }
        }
    }

    impl StatusListener for ReentrantEngineListener {
        fn on_status(
            &self,
            state: TorState,
            _bootstrap_percent: u32,
            _socks_port: Option<u16>,
            _summary: String,
        ) {
            self.statuses.lock().unwrap().push(state);
            self.reenter();
        }
        fn on_log(&self, line: String) {
            self.logs.lock().unwrap().push(line);
            self.reenter();
        }
        fn on_error(&self, error: ArtiErrorDetail) {
            self.errors.lock().unwrap().push(error);
        }
    }

    #[tokio::test]
    async fn callback_reentry_does_not_deadlock() {
        let (engine, _rec) = running_engine("reentry");
        let listener = ReentrantSessionListener {
            engine: engine.clone(),
            events: Arc::new(StdMutex::new(vec![])),
        };
        let session = engine
            .create_session(Box::new(listener.clone()))
            .expect("create");
        assert_eq!(session.status_snapshot().state, SessionState::Active);
        engine.close_session(&session);
        engine.shutdown();
    }

    #[tokio::test]
    async fn callback_reentry_pause_does_not_deadlock() {
        let (engine, rec) = running_engine("reentry-pause");
        let session = engine.create_session(Box::new(rec.clone())).unwrap();
        let engine_listener = ReentrantEngineListener::new(&engine);
        engine
            .inner
            .lock()
            .unwrap()
            .shared
            .set_listener(Arc::new(engine_listener.clone()));
        engine.pause();
        assert_eq!(
            *engine_listener.statuses.lock().unwrap(),
            vec![TorState::Paused]
        );
        assert!(engine_listener
            .logs
            .lock()
            .unwrap()
            .iter()
            .any(|line| line == "SOCKS paused; TorClient retained"));
        assert_eq!(session.status_snapshot().state, SessionState::Paused);
        engine.shutdown();
    }

    #[tokio::test]
    async fn callback_reentry_shutdown_does_not_deadlock() {
        let (engine, rec) = running_engine("reentry-shutdown");
        let session = engine.create_session(Box::new(rec.clone())).unwrap();
        let engine_listener = ReentrantEngineListener::new(&engine);
        engine
            .inner
            .lock()
            .unwrap()
            .shared
            .set_listener(Arc::new(engine_listener.clone()));
        engine.shutdown();
        assert_eq!(
            *engine_listener.statuses.lock().unwrap(),
            vec![TorState::Off]
        );
        assert_eq!(session.status_snapshot().state, SessionState::Invalidated);
    }

    #[test]
    fn session_paused_callback_reentry_does_not_deadlock() {
        let (engine, _) = running_engine("session-reentry-pause");
        let listener = ReentrantSessionListener {
            engine: engine.clone(),
            events: Arc::new(StdMutex::new(vec![])),
        };
        let session = engine.create_session(Box::new(listener.clone())).unwrap();
        engine.pause();
        assert_eq!(
            listener.events.lock().unwrap().last(),
            Some(&(session.id(), SessionState::Paused, None))
        );
        assert_eq!(session.status_snapshot().state, SessionState::Paused);
        engine.shutdown();
    }

    #[test]
    fn session_invalidated_callback_reentry_does_not_deadlock() {
        let (engine, _) = running_engine("session-reentry-invalidate");
        let listener = ReentrantSessionListener {
            engine: engine.clone(),
            events: Arc::new(StdMutex::new(vec![])),
        };
        let session = engine.create_session(Box::new(listener.clone())).unwrap();
        engine.shutdown();
        assert_eq!(
            listener.events.lock().unwrap().last(),
            Some(&(session.id(), SessionState::Invalidated, None))
        );
        assert_eq!(session.status_snapshot().state, SessionState::Invalidated);
    }

    #[test]
    fn concurrent_create_at_max_limit_is_atomic() {
        assert_concurrent_cap(false);
    }

    #[test]
    fn concurrent_create_paused_at_max_limit_is_atomic() {
        assert_concurrent_cap(true);
    }

    fn assert_concurrent_cap(paused: bool) {
        let (engine, rec) = running_engine(if paused { "max-paused" } else { "max-running" });
        if paused {
            engine.pause();
        }
        let gate = install_commit_gate(&engine);
        let attempts = MAX_SESSIONS + 8;
        let mut threads = Vec::new();
        for _ in 0..attempts {
            let engine = engine.clone();
            let rec = rec.clone();
            threads.push(std::thread::spawn(move || {
                engine.create_session(Box::new(rec))
            }));
        }
        // Every create passed the initial empty-registry cap check before
        // any is permitted to commit. This forces the final admission check.
        gate.arrived.wait_for(attempts);
        assert!(engine.list_sessions().is_empty());
        gate.release.signal();
        let results: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
        assert_eq!(results.iter().filter(|r| r.is_ok()).count(), MAX_SESSIONS);
        assert_eq!(results.iter().filter(|r| matches!(r, Err(ArtiError::Runtime { msg }) if msg == "session limit reached")).count(), 8);
        assert_eq!(engine.list_sessions().len(), MAX_SESSIONS);
        assert!(engine.list_sessions().iter().all(|s| s.state
            == if paused {
                SessionState::Paused
            } else {
                SessionState::Active
            }));
        engine.shutdown();
    }

    fn install_commit_gate(engine: &ArtiTor) -> Arc<TestCommitGate> {
        let gate = Arc::new(TestCommitGate::default());
        engine.inner.lock().unwrap().test_commit_gate = Some(gate.clone());
        gate
    }

    fn install_listener_probe(engine: &ArtiTor) -> Arc<TestListenerProbe> {
        let probe = Arc::new(TestListenerProbe::default());
        let mut inner = engine.inner.lock().unwrap();
        inner.test_dispatch_count = Some(probe.dispatch_count.clone());
        *inner.shared.test_listener_probe.lock().unwrap() = Some(probe.clone());
        probe
    }

    #[test]
    fn create_vs_pause_race_deterministic() {
        let (engine, rec) = running_engine("create-vs-pause");
        let gate = install_commit_gate(&engine);
        let create_engine = engine.clone();
        let create_rec = rec.clone();
        let create = std::thread::spawn(move || create_engine.create_session(Box::new(create_rec)));
        gate.arrived.wait();
        assert!(gate.bound_port.lock().unwrap().is_some());
        assert!(engine.list_sessions().is_empty());
        engine.pause();
        gate.release.signal();
        let session = create
            .join()
            .unwrap()
            .expect("create remains live while paused");
        assert_eq!(session.status_snapshot().state, SessionState::Paused);
        assert_eq!(session.status_snapshot().port, None);
        assert_eq!(
            rec.events(),
            vec![(session.id(), SessionState::Paused, None)]
        );
        assert_eq!(engine.list_sessions().len(), 1);
        engine.shutdown();
    }

    #[test]
    fn create_vs_shutdown_race_deterministic() {
        let (engine, rec) = running_engine("create-vs-shutdown");
        let gate = install_commit_gate(&engine);
        let create_engine = engine.clone();
        let create_rec = rec.clone();
        let create = std::thread::spawn(move || create_engine.create_session(Box::new(create_rec)));
        gate.arrived.wait();
        assert!(gate.bound_port.lock().unwrap().is_some());
        assert!(engine.list_sessions().is_empty());
        engine.shutdown();
        gate.release.signal();
        assert!(matches!(create.join().unwrap(), Err(ArtiError::NotRunning)));
        assert!(
            rec.events().is_empty(),
            "uncommitted create publishes no ACTIVE"
        );
        assert!(engine.list_sessions().is_empty());
    }

    #[test]
    fn rebuild_spawn_cold_failure_still_notifies_invalidated() {
        assert_spawn_cold_failure_notifies(true);
    }

    #[test]
    fn cold_error_recovery_spawn_cold_failure_still_notifies_invalidated() {
        assert_spawn_cold_failure_notifies(false);
    }

    fn assert_spawn_cold_failure_notifies(rebuild: bool) {
        let (engine, rec) = running_engine(if rebuild {
            "rebuild-sync-failure"
        } else {
            "cold-sync-failure"
        });
        let session = engine.create_session(Box::new(rec.clone())).unwrap();
        engine.pause();
        let old_config = test_config();
        let mut config = old_config.clone();
        {
            let mut inner = engine.inner.lock().unwrap();
            inner.last_config = Some(old_config);
            inner.test_spawn_cold_failure = true;
            if rebuild {
                config.data_dir.push_str("-replacement");
            } else {
                // Model ERROR cold recovery after the root client was lost.
                inner.shared.set_client_under_gate(None);
                inner.shared.bootstrap_done.store(false, Ordering::SeqCst);
                *inner.shared.engine_state.lock().unwrap() = TorState::Error;
            }
        }
        let listener = ReentrantEngineListener::new(&engine);
        let result = engine.start(config, Box::new(listener));
        assert!(
            matches!(result, Err(ArtiError::Runtime { ref msg }) if msg == "injected spawn_cold failure")
        );
        assert_eq!(session.status_snapshot().state, SessionState::Invalidated);
        assert_eq!(session.status_snapshot().port, None);
        assert!(engine.list_sessions().is_empty());
        assert_eq!(
            rec.last_for(&session.id()),
            Some((SessionState::Invalidated, None))
        );
        assert_eq!(
            rec.events()
                .iter()
                .filter(|(_, s, _)| *s == SessionState::Invalidated)
                .count(),
            1
        );
        engine.shutdown();
    }

    #[test]
    fn resume_root_bind_failure_never_activates_sessions() {
        let (engine, rec) = running_engine("resume-real-bind-failure");
        let listener = ReentrantEngineListener::new(&engine);
        let shared = engine.inner.lock().unwrap().shared.clone();
        shared.set_listener(Arc::new(listener.clone()));
        let handle = engine
            .inner
            .lock()
            .unwrap()
            .runtime
            .as_ref()
            .unwrap()
            .handle()
            .clone();
        // Start a real root listener, then use its selected port as a fixed
        // configured port for resume. No Tor bootstrap is required.
        let (ready_tx, ready_rx) = oneshot::channel();
        let root_client = shared.client().unwrap();
        let root_shared = shared.clone();
        let worker = handle.spawn(async move {
            run_socks(root_client, 0, root_shared, Some(ready_tx))
                .await
                .unwrap();
        });
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(ready_rx).unwrap();
        let port = engine.socks_port().unwrap();
        {
            let mut inner = engine.inner.lock().unwrap();
            let mut config = test_config();
            config.socks_port = port;
            inner.last_config = Some(config);
        }
        let session = engine.create_session(Box::new(rec.clone())).unwrap();
        worker.abort();
        engine.pause();
        assert_eq!(
            engine.socks_port(),
            None,
            "pause must clear the published root endpoint"
        );
        let paused_history = rec.events().len();
        // Joining establishes socket closure before the external bind.
        rt.block_on(async {
            let _ = worker.await;
        });
        let occupied =
            std::net::TcpListener::bind(("127.0.0.1", port)).expect("occupy exact root port");
        engine.resume(Box::new(listener.clone())).unwrap();
        let resume_worker = engine.inner.lock().unwrap().worker.take().unwrap();
        rt.block_on(resume_worker).unwrap();
        assert_eq!(shared.engine_state(), TorState::Error);
        assert_eq!(
            listener.statuses.lock().unwrap().last(),
            Some(&TorState::Error)
        );
        assert!(listener
            .errors
            .lock()
            .unwrap()
            .iter()
            .any(|e| e.kind == ErrorKind::Bind && e.port == Some(port)));
        assert!(rec.events()[paused_history..]
            .iter()
            .all(|(_, s, _)| *s != SessionState::Active));
        assert_eq!(session.status_snapshot().state, SessionState::Paused);
        assert_eq!(session.status_snapshot().port, None);
        assert_eq!(engine.socks_port(), None);
        drop(occupied);
        engine.shutdown();
    }

    #[test]
    fn session_listener_does_not_dispatch_before_active_commit() {
        let (engine, rec) = running_engine("dispatch-precommit");
        let gate = install_commit_gate(&engine);
        let probe = install_listener_probe(&engine);
        let create_engine = engine.clone();
        let create_rec = rec.clone();
        let create = std::thread::spawn(move || create_engine.create_session(Box::new(create_rec)));
        gate.arrived.wait();
        let port = gate.bound_port.lock().unwrap().unwrap();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let mut stream = rt
            .block_on(tokio::net::TcpStream::connect(("127.0.0.1", port)))
            .unwrap();
        // Queue enough SOCKS bytes to exercise the real handler after release.
        rt.block_on(stream.write_all(&[0x05, 0x01, 0x00])).unwrap();
        probe.assert_barrier_pending();
        assert!(engine.list_sessions().is_empty());
        assert!(rec.events().is_empty());
        gate.release.signal();
        let session = create.join().unwrap().unwrap();
        let mut greeting = [0u8; 2];
        rt.block_on(stream.read_exact(&mut greeting)).unwrap();
        assert_eq!(greeting, [0x05, 0x00]);
        assert_eq!(probe.dispatch_count.load(Ordering::SeqCst), 1);
        assert_eq!(
            rec.last_for(&session.id()),
            Some((SessionState::Active, Some(port)))
        );
        engine.shutdown();
    }
    #[derive(Clone, Copy)]
    enum PublicationAction {
        Close,
        Pause,
    }

    struct PublicationReentryListener {
        engine: Weak<ArtiTor>,
        probe: Arc<TestListenerProbe>,
        events: SessionRecorder,
        action: PublicationAction,
        // Keep the listener runnable after the callback mutates the registry,
        // so this test observes the dispatch gate rather than task abortion.
        retained_task: Mutex<Option<(JoinHandle<()>, Option<oneshot::Sender<()>>)>>,
        traffic: Mutex<Option<std::net::TcpStream>>,
    }

    impl SessionStatusListener for Arc<PublicationReentryListener> {
        fn on_session_status(
            &self,
            sid: String,
            state: SessionState,
            port: Option<u16>,
            revision: u64,
        ) {
            self.events
                .on_session_status(sid.clone(), state, port, revision);
            let engine = self.engine.upgrade().unwrap();
            let _ = engine.has_client();
            let _ = engine.socks_port();
            if state != SessionState::Active {
                return;
            }
            let shared = engine.inner.lock().unwrap().shared.clone();
            let snapshot = shared.session_snapshot_for(&sid, shared.generation);
            assert_eq!(
                snapshot.state,
                SessionState::Active,
                "commit must precede callback"
            );
            assert_eq!(snapshot.port, port);
            self.probe.assert_barrier_pending();
            let mut traffic = std::net::TcpStream::connect(("127.0.0.1", port.unwrap())).unwrap();
            std::io::Write::write_all(&mut traffic, &[0x05, 0x01, 0x00]).unwrap();
            *self.traffic.lock().unwrap() = Some(traffic);
            {
                let mut sessions = shared.sessions.lock().unwrap();
                let entry = sessions.get_mut(&sid).unwrap();
                *self.retained_task.lock().unwrap() = Some((
                    entry.listener_task.take().unwrap(),
                    entry.shutdown_tx.take(),
                ));
            }
            match self.action {
                PublicationAction::Close => {
                    close_session_handle(&Arc::downgrade(&shared), &sid, shared.generation)
                }
                PublicationAction::Pause => engine.pause(),
            }
        }
    }

    fn assert_publication_reentry(rebind: bool, action: PublicationAction) {
        let (engine, events) = running_engine("publication-reentry");
        if rebind {
            engine.pause();
        }
        let probe = install_listener_probe(&engine);
        let listener = Arc::new(PublicationReentryListener {
            engine: Arc::downgrade(&engine),
            probe: probe.clone(),
            events: events.clone(),
            action,
            retained_task: Mutex::new(None),
            traffic: Mutex::new(None),
        });
        let session = engine.create_session(Box::new(listener.clone())).unwrap();
        if rebind {
            let inner = engine.inner.lock().unwrap();
            let shared = inner.shared.clone();
            let handle = inner.runtime.as_ref().unwrap().handle().clone();
            *shared.engine_state.lock().unwrap() = TorState::Running;
            drop(inner);
            rebind_paused_sessions(&shared, &handle);
        }
        // The queued SOCKS greeting is accepted only after ACTIVE publication
        // returns; its dispatch is rejected because that callback closed/paused.
        probe.dispatch_checked.wait();
        assert_eq!(probe.dispatch_count.load(Ordering::SeqCst), 0);
        let expected = match action {
            PublicationAction::Close => SessionState::Closed,
            PublicationAction::Pause => SessionState::Paused,
        };
        assert_eq!(session.status_snapshot().state, expected);
        assert_eq!(session.status_snapshot().port, None);
        assert_eq!(events.last_for(&session.id()), Some((expected, None)));
        let history: Vec<_> = events.events().into_iter().map(|(_, s, _)| s).collect();
        assert_eq!(
            history,
            if rebind {
                vec![SessionState::Paused, SessionState::Active, expected]
            } else {
                vec![SessionState::Active, expected]
            }
        );
        let (task, shutdown) = listener.retained_task.lock().unwrap().take().unwrap();
        task.abort();
        drop(shutdown);
        engine.shutdown();
    }

    #[test]
    fn create_active_callback_close_precedes_barrier_and_rejects_dispatch() {
        assert_publication_reentry(false, PublicationAction::Close);
    }

    #[test]
    fn create_active_callback_pause_precedes_barrier_and_rejects_dispatch() {
        assert_publication_reentry(false, PublicationAction::Pause);
    }

    #[test]
    fn rebind_active_callback_close_precedes_barrier_and_rejects_dispatch() {
        assert_publication_reentry(true, PublicationAction::Close);
    }

    #[test]
    fn rebind_active_callback_pause_precedes_barrier_and_rejects_dispatch() {
        assert_publication_reentry(true, PublicationAction::Pause);
    }

    // Permanent regressions promoted from the independent audit counterexamples.
    #[test]
    fn audit_create_crosses_failed_rebuild_without_generation_barrier() {
        let (engine, rec) = running_engine("audit-rebuild");
        engine.pause();
        let gate = install_commit_gate(&engine);
        let before = engine.inner.lock().unwrap().generation;
        let e = engine.clone();
        let create_rec = rec.clone();
        let thread = std::thread::spawn(move || e.create_session(Box::new(create_rec)));
        gate.arrived.wait();
        {
            let mut inner = engine.inner.lock().unwrap();
            inner.last_config = Some(test_config());
            inner.test_spawn_cold_failure = true;
        }
        let mut changed = test_config();
        changed.data_dir.push_str("-audit-new-client");
        let err = engine.start(
            changed,
            Box::new(Recorder {
                statuses: StdMutex::new(vec![]),
                errors: StdMutex::new(vec![]),
            }),
        );
        assert!(matches!(err, Err(ArtiError::Runtime { .. })));
        assert!(!engine.has_client());
        gate.release.signal();
        let result = thread.join().unwrap();
        assert!(
            engine.list_sessions().is_empty(),
            "failed rebuild admits no old-client session"
        );
        assert!(!engine.has_client());
        assert!(
            rec.events().is_empty(),
            "old-root create publishes no ACTIVE or PAUSED"
        );
        let after = engine.inner.lock().unwrap().generation;
        assert_eq!(before, after, "replacement uses the same engine generation");
        eprintln!(
            "AUDIT rebuild: before={before}, after={after}, create={result:?}, registry={:?}",
            engine.list_sessions()
        );
        engine.shutdown();
        assert!(
            matches!(result, Err(ArtiError::NotRunning)),
            "old-client creation must fail after replacement teardown"
        );
    }

    #[test]
    fn audit_start_after_error_must_invalidate_retained_sessions() {
        let (engine, rec) = running_engine("audit-error-start");
        let session = engine.create_session(Box::new(rec.clone())).unwrap();
        let shared = engine.inner.lock().unwrap().shared.clone();
        shared.notify_error(
            &ArtiError::Bind {
                port: 1,
                msg: "test".into(),
            },
            100,
        );
        engine
            .start(
                test_config(),
                Box::new(Recorder {
                    statuses: StdMutex::new(vec![]),
                    errors: StdMutex::new(vec![]),
                }),
            )
            .unwrap();
        let snap = session.status_snapshot();
        let events = rec.events();
        eprintln!("AUDIT start after ERROR: snapshot={snap:?}, events={events:?}");
        engine.shutdown();
        assert_eq!(
            snap.state,
            SessionState::Invalidated,
            "freeze matrix says subsequent start invalidates ERROR sessions"
        );
    }

    struct ErrorSnapshotListener {
        shared: Weak<Shared>,
        sid: String,
        generation: u64,
        observations: Arc<StdMutex<Vec<SessionStatusFfi>>>,
    }
    impl StatusListener for ErrorSnapshotListener {
        fn on_status(&self, state: TorState, _: u32, _: Option<u16>, _: String) {
            if state == TorState::Error {
                self.observations.lock().unwrap().push(
                    self.shared
                        .upgrade()
                        .unwrap()
                        .session_snapshot_for(&self.sid, self.generation),
                );
            }
        }
        fn on_error(&self, _: ArtiErrorDetail) {
            self.observations.lock().unwrap().push(
                self.shared
                    .upgrade()
                    .unwrap()
                    .session_snapshot_for(&self.sid, self.generation),
            );
        }
        fn on_log(&self, _: String) {}
    }
    #[test]
    fn audit_error_callbacks_must_observe_demoted_sessions() {
        let (engine, rec) = running_engine("audit-error-order");
        let session = engine.create_session(Box::new(rec)).unwrap();
        let shared = engine.inner.lock().unwrap().shared.clone();
        let observations = Arc::new(StdMutex::new(vec![]));
        shared.set_listener(Arc::new(ErrorSnapshotListener {
            shared: Arc::downgrade(&shared),
            sid: session.id(),
            generation: session.generation(),
            observations: observations.clone(),
        }));
        shared.notify_error(
            &ArtiError::Runtime {
                msg: "audit".into(),
            },
            100,
        );
        let got = observations.lock().unwrap().clone();
        eprintln!("AUDIT session snapshots during on_error/on_status(ERROR): {got:?}");
        engine.shutdown();
        assert!(
            got.iter()
                .all(|s| s.state == SessionState::Paused && s.port.is_none()),
            "ERROR observers must see PAUSED/null"
        );
    }

    #[test]
    fn final_rebind_decision_rejects_completed_pause() {
        let (engine, rec) = running_engine("final-rebind-pause");
        engine.pause();
        let session = engine.create_session(Box::new(rec.clone())).unwrap();
        let (shared, handle) = {
            let inner = engine.inner.lock().unwrap();
            (
                inner.shared.clone(),
                inner.runtime.as_ref().unwrap().handle().clone(),
            )
        };
        *shared.engine_state.lock().unwrap() = TorState::Running;
        let gate = Arc::new(TestCommitGate::default());
        *shared.test_rebind_decision_gate.lock().unwrap() = Some(gate.clone());
        let worker_shared = shared.clone();
        let worker = std::thread::spawn(move || rebind_paused_sessions(&worker_shared, &handle));
        gate.arrived.wait();
        engine.pause();
        gate.release.signal();
        worker.join().unwrap();
        assert_eq!(session.status_snapshot().state, SessionState::Paused);
        assert_eq!(session.status_snapshot().port, None);
        assert!(rec
            .events()
            .iter()
            .all(|(_, state, _)| *state != SessionState::Active));
        engine.shutdown();
    }

    #[test]
    fn final_create_decision_rejects_completed_error() {
        let (engine, rec) = running_engine("final-create-error");
        let shared = engine.inner.lock().unwrap().shared.clone();
        let gate = Arc::new(TestCommitGate::default());
        *shared.test_create_decision_gate.lock().unwrap() = Some(gate.clone());
        let e = engine.clone();
        let r = rec.clone();
        let worker = std::thread::spawn(move || e.create_session(Box::new(r)));
        gate.arrived.wait();
        shared.notify_error(
            &ArtiError::Runtime {
                msg: "final boundary".into(),
            },
            100,
        );
        gate.release.signal();
        assert!(matches!(worker.join().unwrap(), Err(ArtiError::NotRunning)));
        assert!(shared.sessions.lock().unwrap().is_empty());
        assert!(rec.events().is_empty());
        engine.shutdown();
    }

    #[test]
    fn delayed_active_publication_carries_older_revision_than_pause() {
        struct DelayedListener {
            gate: Arc<TestCommitGate>,
            events: Arc<StdMutex<Vec<(SessionState, u64)>>>,
        }
        impl SessionStatusListener for DelayedListener {
            fn on_session_status(
                &self,
                _: String,
                state: SessionState,
                port: Option<u16>,
                revision: u64,
            ) {
                if state == SessionState::Active {
                    self.gate.arrive_and_wait(port);
                }
                self.events.lock().unwrap().push((state, revision));
            }
        }
        let (engine, _) = running_engine("delayed-publication-revision");
        let gate = Arc::new(TestCommitGate::default());
        let events = Arc::new(StdMutex::new(vec![]));
        let e = engine.clone();
        let g = gate.clone();
        let ev = events.clone();
        let worker = std::thread::spawn(move || {
            e.create_session(Box::new(DelayedListener {
                gate: g,
                events: ev,
            }))
        });
        gate.arrived.wait();
        engine.pause();
        gate.release.signal();
        let session = worker.join().unwrap().unwrap();
        assert_eq!(
            *events.lock().unwrap(),
            vec![(SessionState::Paused, 2), (SessionState::Active, 1)]
        );
        assert_eq!(session.status_snapshot().revision, 2);
        engine.shutdown();
    }

    #[test]
    fn fatal_accept_error_demotes_only_current_listener() {
        struct InjectListener {
            shared: Weak<Shared>,
            paused: Arc<TestSignal>,
        }
        impl SessionStatusListener for InjectListener {
            fn on_session_status(&self, id: String, state: SessionState, _: Option<u16>, _: u64) {
                let shared = self.shared.upgrade().unwrap();
                assert!(
                    shared.transition_gate.try_lock().is_ok(),
                    "callback outside transaction"
                );
                if state == SessionState::Active {
                    *shared.test_accept_failure.lock().unwrap() = Some(id);
                }
                if state == SessionState::Paused {
                    self.paused.signal();
                }
            }
        }
        let (engine, _) = running_engine("fatal-accept");
        let shared = engine.inner.lock().unwrap().shared.clone();
        let paused = Arc::new(TestSignal::default());
        let session = engine
            .create_session(Box::new(InjectListener {
                shared: Arc::downgrade(&shared),
                paused: paused.clone(),
            }))
            .unwrap();
        paused.wait();
        assert_eq!(session.status_snapshot().state, SessionState::Paused);
        assert_eq!(session.status_snapshot().port, None);
        assert_eq!(session.status_snapshot().revision, 2);
        assert_eq!(shared.engine_state(), TorState::Running);
        engine.shutdown();
    }

    #[test]
    fn stale_listener_failure_cannot_demote_rebound_closed_or_invalidated_session() {
        let (engine, rec) = running_engine("stale-listener-failure");
        let session = engine.create_session(Box::new(rec.clone())).unwrap();
        let shared = engine.inner.lock().unwrap().shared.clone();
        let old_revision = session.status_snapshot().revision;
        engine.pause();
        *shared.engine_state.lock().unwrap() = TorState::Running;
        let handle = engine
            .inner
            .lock()
            .unwrap()
            .runtime
            .as_ref()
            .unwrap()
            .handle()
            .clone();
        rebind_paused_sessions(&shared, &handle);
        let current = session.status_snapshot();
        assert_eq!(current.state, SessionState::Active);
        session_listener_failed(
            &Arc::downgrade(&shared),
            &session.id(),
            session.generation(),
            old_revision,
        );
        assert_eq!(session.status_snapshot(), current);
        session.close_session();
        session_listener_failed(
            &Arc::downgrade(&shared),
            &session.id(),
            session.generation(),
            current.revision,
        );
        assert_eq!(session.status_snapshot().state, SessionState::Closed);
        let other = engine.create_session(Box::new(rec)).unwrap();
        let rev = other.status_snapshot().revision;
        engine.shutdown();
        session_listener_failed(
            &Arc::downgrade(&shared),
            &other.id(),
            other.generation(),
            rev,
        );
        assert_eq!(other.status_snapshot().state, SessionState::Invalidated);
    }

    #[test]
    fn actual_root_and_two_session_dispatches_select_distinct_client_handles() {
        let (engine, rec) = running_engine("dispatch-identities");
        let shared = engine.inner.lock().unwrap().shared.clone();
        let handle = engine
            .inner
            .lock()
            .unwrap()
            .runtime
            .as_ref()
            .unwrap()
            .handle()
            .clone();
        let root = shared.client().unwrap();
        let root_ptr = Arc::as_ptr(&root) as usize;
        let (ready_tx, ready_rx) = oneshot::channel();
        let root_shared = shared.clone();
        let worker = handle.spawn(async move {
            run_socks(root, 0, root_shared, Some(ready_tx))
                .await
                .unwrap();
        });
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(ready_rx).unwrap();
        let a = engine.create_session(Box::new(rec.clone())).unwrap();
        let b = engine.create_session(Box::new(rec)).unwrap();
        let (a_ptr, b_ptr) = {
            let entries = shared.sessions.lock().unwrap();
            (
                Arc::as_ptr(&entries[&a.id()].isolated) as usize,
                Arc::as_ptr(&entries[&b.id()].isolated) as usize,
            )
        };
        assert_ne!(root_ptr, a_ptr);
        assert_ne!(root_ptr, b_ptr);
        assert_ne!(a_ptr, b_ptr);
        for port in [
            engine.socks_port().unwrap(),
            a.status_snapshot().port.unwrap(),
            b.status_snapshot().port.unwrap(),
        ] {
            rt.block_on(async {
                let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
                    .await
                    .unwrap();
                stream.write_all(&[5, 1, 0]).await.unwrap();
                let mut reply = [0; 2];
                tokio::time::timeout(Duration::from_secs(5), stream.read_exact(&mut reply))
                    .await
                    .unwrap()
                    .unwrap();
                assert_eq!(reply, [5, 0]);
            });
        }
        assert_eq!(
            *shared.test_dispatch_clients.lock().unwrap(),
            vec![
                (None, root_ptr),
                (Some(a.id()), a_ptr),
                (Some(b.id()), b_ptr)
            ]
        );
        worker.abort();
        engine.shutdown();
    }

    #[test]
    fn old_root_worker_cannot_publish_after_pause_or_replacement() {
        let (engine, _) = running_engine("worker-revision");
        let shared = engine.inner.lock().unwrap().shared.clone();
        let revision = shared.worker_revision.load(Ordering::SeqCst);
        engine.pause();
        assert!(!shared.report_worker(revision, TorState::Running, 100, Some(9999), "stale"));
        shared.notify_worker_error(
            Some(revision),
            &ArtiError::Runtime {
                msg: "stale".into(),
            },
            100,
        );
        assert_eq!(shared.engine_state(), TorState::Paused);
        assert_eq!(engine.socks_port(), None);
        engine.shutdown();
    }

    #[test]
    fn native_tombstones_are_bounded_diagnostics() {
        let (engine, rec) = running_engine("tombstone-bounds");
        engine.pause();
        let first = engine.create_session(Box::new(rec.clone())).unwrap();
        first.close_session();
        for _ in 0..160 {
            let s = engine.create_session(Box::new(rec.clone())).unwrap();
            s.close_session();
        }
        let shared = engine.inner.lock().unwrap().shared.clone();
        assert!(shared.tombstones.lock().unwrap().len() <= MAX_SESSIONS * 4);
        assert_eq!(first.status_snapshot().state, SessionState::Invalidated);
        engine.shutdown();
    }
}
