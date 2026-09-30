//! Authoritative engine ownership and lifecycle state. Children share parent-private state.
use crate::config::{resolve_dirs, tor_client_config_changed};
use crate::logging::{init_tracing, LOG_SINK};
#[cfg(test)]
use crate::socks::{parse_socks_request, SocksRequestError};
use crate::*;
mod bootstrap;
mod dispatch;
include!("lifecycle.rs");
include!("session/admission.rs");
mod root_socks;
use bootstrap::spawn_cold;
use dispatch::handle_socks;
#[cfg(test)]
use root_socks::run_socks;
use root_socks::{run_socks_worker, spawn_socks, wait_root_then_rebind_sessions};
use session::listener::run_session_listener;
mod session;
#[cfg(test)]
use session::session_listener_failed;
use session::{
    next_session_id, rebind_paused_sessions, PendingSessionNotification, SessionRuntime,
    MAX_SESSIONS,
};
#[cfg(test)]
mod test_hooks;
#[cfg(test)]
use arti_client::config::TorClientConfigBuilder;
use arti_client::TorClient;
use futures::StreamExt;
use std::collections::HashMap;
use std::net::SocketAddr;
#[cfg(test)]
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
#[cfg(test)]
use test_hooks::*;
#[cfg(test)]
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::runtime::Runtime;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tor_rtcompat::PreferredRuntime;

// ============================================================================
// Shared handles between lifecycle methods and async tasks
// ============================================================================

pub(super) struct Shared {
    /// Engine generation this `Shared` belongs to. Bumped on every shutdown;
    /// sessions (and handles) carrying an older generation are stale.
    generation: u64,
    /// Lock order: inner -> transition_gate -> state/client -> sessions -> tombstones.
    /// No callbacks or await while held.
    transition_gate: Mutex<()>,
    client_epoch: AtomicU64,
    /// Cancels stale bootstrap/root-listener workers; distinct from publications.
    worker_revision: AtomicU64,
    /// Lifecycle publication freshness across callback re-entry. Advanced only
    /// under transition_gate, including replacement/cancellation transactions.
    engine_publication_revision: AtomicU64,
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
            engine_publication_revision: AtomicU64::new(1),
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

    /// Caller holds transition_gate. Checked arithmetic never wraps into an
    /// older publication token (exhaustion is outside a realistic lifetime).
    fn advance_engine_publication_under_gate(&self) -> u64 {
        self.engine_publication_revision
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |r| r.checked_add(1))
            .expect("engine publication revision exhausted")
            + 1
    }

    fn publication_is_current(&self, revision: u64) -> bool {
        self.engine_publication_revision.load(Ordering::SeqCst) == revision
    }

    fn report_worker(
        &self,
        revision: u64,
        state: TorState,
        pct: u32,
        port: Option<u16>,
        summary: impl Into<String>,
    ) -> bool {
        let publication = {
            let _transition = self.transition_gate.lock().unwrap();
            if self.worker_revision.load(Ordering::SeqCst) != revision {
                return false;
            }
            *self.engine_state.lock().unwrap() = state;
            if let Some(p) = port {
                self.bound_port.store(p, Ordering::SeqCst);
            }
            self.advance_engine_publication_under_gate()
        };
        if !self.publication_is_current(publication) {
            return false;
        }
        if let Some(l) = self.listener() {
            l.on_status(state, pct, port, summary.into());
        }
        self.publication_is_current(publication)
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
        self.advance_engine_publication_under_gate();
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

    /// Snapshot lookup for one handle: `(id, generation)` must match a live
    /// registry entry. Returns the entry snapshot, the recorded terminal state
    /// for a removed session, or `Invalidated` for unknown/stale handles.
    pub(super) fn session_snapshot_for(&self, sid: &str, generation: u64) -> SessionStatusFfi {
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

// ============================================================================
// ArtiTor object
// ============================================================================

pub(super) struct Inner {
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

#[cfg(test)]
#[path = "../tests/mod.rs"]
mod tests;

pub(super) fn close_session_handle(weak: &Weak<Shared>, sid: &str, generation: u64) {
    session::close_session_handle(weak, sid, generation);
}
