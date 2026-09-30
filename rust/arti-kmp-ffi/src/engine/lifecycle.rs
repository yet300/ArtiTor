// Engine state machine and publication freshness, including typed ERROR reentry.
// Pending session notifications are dispatched outside lifecycle/registry locks.

/// Result of authoritative pause mutation, dispatched after releasing locks.
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

impl ArtiTor {
    pub(super) fn ffi_new() -> Arc<Self> {
        // Process-global note: `LOG_SINK` routes tracing output to the most
        // recently installed listener. Only ONE ArtiTor instance per process is
        // supported; a second instance steals log routing (documented, not
        // silently multi-instance). Status/error callbacks remain per-instance.
        Arc::new(Self {
            inner: Mutex::new(Inner::default()),
        })
    }

    pub(super) fn ffi_version(&self) -> String {
        format!(
            "arti-kmp-ffi {} (arti-client 0.46, rustls)",
            env!("CARGO_PKG_VERSION")
        )
    }

    /// True when a bootstrapped TorClient is held (RUNNING or PAUSED).
    pub(super) fn ffi_has_client(&self) -> bool {
        let inner = self.inner.lock().unwrap();
        inner.shared.bootstrap_done.load(Ordering::SeqCst) && inner.shared.client().is_some()
    }

    /// True when SOCKS is bound and bootstrap is complete.
    pub(super) fn ffi_is_ready(&self) -> bool {
        let inner = self.inner.lock().unwrap();
        inner.shared.bootstrap_done.load(Ordering::SeqCst)
            && inner.shared.bound_port.load(Ordering::SeqCst) != 0
    }

    /// Currently bound SOCKS port, if listening.
    pub(super) fn ffi_socks_port(&self) -> Option<u16> {
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
    pub(super) fn ffi_start(
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
    pub(super) fn ffi_resume(&self, listener: Box<dyn StatusListener>) -> Result<(), ArtiError> {
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
    pub(super) fn ffi_pause(&self) {
        let (outcome, shared, publication) = {
            let mut inner = self.inner.lock().unwrap();
            let shared = inner.shared.clone();
            let _transition = shared.transition_gate.lock().unwrap();
            shared.advance_engine_publication_under_gate();
            shared.worker_revision.fetch_add(1, Ordering::SeqCst);
            inner.shared.signal_socks_shutdown();
            inner.shared.bound_port.store(0, Ordering::SeqCst);
            inner.shared.abort_connections();
            if let Some(w) = inner.worker.take() {
                w.abort();
            }
            let outcome = if inner.shared.bootstrap_done.load(Ordering::SeqCst)
                && inner.shared.client().is_some()
            {
                let superseding_error = shared.engine_state() == TorState::Error;
                *inner.shared.engine_state.lock().unwrap() = TorState::Paused;
                let sessions = inner.shared.demote_sessions_to_paused(superseding_error);
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
            };
            let publication = shared.engine_publication_revision.load(Ordering::SeqCst);
            (outcome, shared.clone(), publication)
        };
        match outcome {
            PauseOutcome::Paused { sessions, listener } => {
                for n in sessions {
                    // A newer engine publication does not supersede a still-
                    // current session revision. Finish its committed delivery.
                    if shared
                        .session_snapshot_for(&n.id, shared.generation)
                        .revision
                        != n.revision
                    {
                        continue;
                    }
                    n.listener
                        .on_session_status(n.id, n.state, n.port, n.revision);
                }
                if !shared.publication_is_current(publication) {
                    return;
                }
                if let Some(l) = listener {
                    l.on_status(TorState::Paused, 100, None, "paused".into());
                    if !shared.publication_is_current(publication) {
                        return;
                    }
                    l.on_log("SOCKS paused; TorClient retained".into());
                }
            }
            PauseOutcome::CancelledBootstrap { sessions, listener } => {
                for n in sessions {
                    // A newer engine publication does not supersede a still-
                    // current session revision. Finish its committed delivery.
                    if shared
                        .session_snapshot_for(&n.id, shared.generation)
                        .revision
                        != n.revision
                    {
                        continue;
                    }
                    n.listener
                        .on_session_status(n.id, n.state, n.port, n.revision);
                }
                if !shared.publication_is_current(publication) {
                    return;
                }
                if let Some(l) = listener {
                    l.on_status(TorState::Off, 0, None, "bootstrap cancelled".into());
                    if !shared.publication_is_current(publication) {
                        return;
                    }
                    l.on_log("bootstrap cancelled by pause; client discarded".into());
                }
            }
        }
    }

    /// Full teardown: SOCKS, client, and tokio runtime.
    ///
    /// Postcondition: OFF, no client, no SOCKS listener, no worker, runtime
    /// released. All live SOCKS streams are terminated. Idempotent.
    pub(super) fn ffi_shutdown(&self) {
        let (session_pending, old_listener, current_shared) = {
            let mut inner = self.inner.lock().unwrap();
            let shared = inner.shared.clone();
            let _transition = shared.transition_gate.lock().unwrap();
            shared.advance_engine_publication_under_gate();
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
            (session_pending, old_listener, inner.shared.clone())
        };
        // The replacement Shared is OFF at revision 1. Any reentrant start,
        // pause or shutdown advances this fence before the old OFF is sent.
        for n in session_pending {
            // Terminal invalidations stay authoritative across a new lifecycle;
            // don't strand sibling observers when a callback restarts the engine.
            n.listener
                .on_session_status(n.id, n.state, n.port, n.revision);
        }
        if !current_shared.publication_is_current(1) {
            return;
        }
        if let Some(l) = old_listener {
            l.on_status(TorState::Off, 0, None, String::new());
        }
    }

    /// Deprecated alias for [Self::shutdown] (0.1.x compatibility).
    pub(super) fn ffi_stop(&self) {
        self.shutdown();
    }
}

impl Shared {
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
        let (pending, listener, publication) = {
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
                self.demote_sessions_to_paused(false),
                self.listener(),
                self.advance_engine_publication_under_gate(),
            )
        };
        for n in pending {
            if !self.publication_is_current(publication) {
                return;
            }
            n.listener
                .on_session_status(n.id, n.state, n.port, n.revision);
            if !self.publication_is_current(publication) {
                return;
            }
        }
        if !self.publication_is_current(publication) {
            return;
        }
        if let Some(l) = listener {
            l.on_error(ArtiErrorDetail::from(error));
            // Foreign callbacks may complete a newer lifecycle transaction.
            if !self.publication_is_current(publication) {
                return;
            }
            l.on_status(
                TorState::Error,
                bootstrap_pct,
                None,
                format!("error: {error}"),
            );
            if !self.publication_is_current(publication) {
                return;
            }
            l.on_log(format!("ERROR: {error}"));
        }
    }
}
