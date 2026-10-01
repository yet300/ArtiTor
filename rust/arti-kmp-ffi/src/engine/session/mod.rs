//! Engine-owned session resources, admission, rebind and terminal registry transitions.
//! Callbacks are dispatched after transaction guards are released.
use super::*;
pub(super) mod listener;
use listener::run_session_listener;
#[cfg(test)]
pub(super) use listener::session_listener_failed;

/// Private conservative safety cap based on listener/task/client resource accounting.
/// Non-contractual; Phase 5 must measure incremental FD, memory and task costs
/// on Android devices and iOS devices/simulators and empirically revisit 32.
pub(super) const MAX_SESSIONS: usize = 32;

/// Process-local opaque session-id counter.
///
/// Session ids are diagnostic only: unique within the process (hence within
/// any engine generation), never derived from `IsolationToken`, never
/// persisted, never reused. Uniqueness is the soft barrier; the
/// `(id, generation)` check against the live registry is the hard barrier
/// against stale-handle resurrection.
static SESSION_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

pub(super) fn next_session_id() -> String {
    let n = SESSION_ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("sess-{n:016x}")
}

/// Typed pending session notification returned by lifecycle mutation
/// helpers. The caller dispatches these after releasing all locks.
#[derive(Clone)]
pub(super) struct PendingSessionNotification {
    pub(super) id: String,
    pub(super) listener: Arc<dyn SessionStatusListener>,
    pub(super) state: SessionState,
    pub(super) port: Option<u16>,
    pub(super) revision: u64,
}

/// Engine-owned session resources.
///
/// Lives in the [`Shared`] registry; NEVER in the exported [`SocksSession`]
/// handle. Dropping a `SessionRuntime` (close / pause-teardown / shutdown)
/// releases the strong isolated `TorClient`, the listener task, and all
/// tracked connection tasks for that session only.
pub(super) struct SessionRuntime {
    /// Engine generation that created this session. Must match both the
    /// owning [`Shared::generation`] and the handle's generation for any
    /// operation; id equality alone never validates.
    pub(super) generation: u64,
    /// The session's sole isolation mechanism: `root.isolated_client()`.
    /// Retained across pause (identity preserved), dropped at close/shutdown.
    pub(super) isolated: Arc<TorClient<PreferredRuntime>>,
    /// Live state: only `Active` or `Paused` is stored. Terminal states are
    /// published then recorded in [`Shared::tombstones`], never stored here.
    pub(super) state: SessionState,
    /// Internal u64 monotonic ordering token; exhaustion is outside a realistic
    /// process lifetime, not API. N03 overflow policy remains a LOW follow-up.
    pub(super) status_revision: u64,
    /// Actual bound loopback port when `Active`; `None` when `Paused`.
    pub(super) port: Option<u16>,
    /// Signals the session accept loop to exit (close / pause / shutdown).
    pub(super) shutdown_tx: Option<oneshot::Sender<()>>,
    /// Session accept-loop task (or in-flight rebind task on resume).
    pub(super) listener_task: Option<JoinHandle<()>>,
    /// Sole physical socket; the task polls through this controller and cannot
    /// retain the listener across Pending or a callback.
    pub(super) socket: Option<Arc<ListeningSocket>>,
    /// Per-connection handlers accepted on this session's listener.
    pub(super) connections: Vec<JoinHandle<()>>,
    /// Kotlin status sink for atomic `(state, endpoint)` publications.
    pub(super) status_listener: Option<Arc<dyn SessionStatusListener>>,
}

impl SessionRuntime {
    pub(super) fn snapshot(&self) -> SessionStatusFfi {
        SessionStatusFfi {
            state: self.state,
            port: self.port,
            revision: self.status_revision,
        }
    }

    /// Physically close the listener, then request task/connection cancellation
    /// (fail-closed). No task join, await or callback. Safe to call redundantly.
    pub(super) fn abort_all(&mut self) {
        if let Some(socket) = self.socket.take() {
            socket.close();
        }
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

impl Shared {
    /// Freeze every live session to `Paused` (endpoint null), aborting its
    /// listener and tracked connections but retaining its isolated client
    /// (isolation identity preserved). Publishes only genuine `Active → Paused`
    /// transitions normally. A pause superseding ERROR replays current PAUSED
    /// snapshots: ERROR may have committed them without completing callbacks.
    /// Duplicate revisions are idempotent at the Kotlin acceptance fence.
    /// No-op when the registry is empty.
    ///
    /// Returns typed pending notifications for the caller to dispatch after
    /// releasing all locks (callback re-entry safety). Does NOT invoke any
    /// FFI callbacks itself.
    pub(super) fn demote_sessions_to_paused(
        &self,
        replay_paused: bool,
    ) -> Vec<PendingSessionNotification> {
        let mut sessions = self.sessions.lock().unwrap();
        let mut out = Vec::new();
        for (id, entry) in sessions.iter_mut() {
            entry.abort_all();
            let changed = entry.state == SessionState::Active;
            entry.state = SessionState::Paused;
            entry.port = None;
            if changed {
                entry.status_revision += 1;
            }
            if changed || replay_paused {
                if let Some(l) = entry.status_listener.clone() {
                    out.push(PendingSessionNotification {
                        id: id.clone(),
                        listener: l,
                        state: SessionState::Paused,
                        port: None,
                        revision: entry.status_revision,
                    });
                }
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
    pub(super) fn invalidate_all_sessions(&self) -> Vec<PendingSessionNotification> {
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
pub(super) fn close_session_handle(weak: &Weak<Shared>, sid: &str, generation: u64) {
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
pub(super) fn rebind_paused_sessions(shared: &Arc<Shared>, runtime: &tokio::runtime::Handle) {
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
        let sock = {
            let _context = runtime.enter();
            match tokio::net::TcpListener::from_std(sock) {
                Ok(socket) => Arc::new(ListeningSocket::new(socket)),
                Err(e) => {
                    if let Some(l) = shared.listener() {
                        l.on_log(format!(
                            "session={sid} listener convert failed: {}",
                            e.kind()
                        ));
                    }
                    continue;
                }
            }
        };
        let task_socket = sock.clone();
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
                Some(task_socket),
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
                    entry.socket = Some(sock.clone());
                    Some(entry.status_revision)
                }
                _ => None,
            }
        };
        if let Some(revision) = commit {
            listener.on_session_status(sid, SessionState::Active, Some(port), revision);
            let _ = barrier_tx.send(());
        } else {
            sock.close();
            if let Some(h) = listener_task {
                h.abort();
            }
            if let Some(tx) = shutdown_tx {
                let _ = tx.send(());
            }
        }
    }
}
