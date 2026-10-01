// Session admission implementation, included at engine scope for facade-only access.
impl ArtiTor {
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
    pub(super) fn ffi_create_session(
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
                    error_kind: crate::TorErrorKind::Runtime, msg: "session limit reached".into(),
                });
            }
            let root = shared.client().ok_or(ArtiError::NotRunning)?;
            let runtime = inner
                .runtime
                .as_ref()
                .ok_or_else(|| ArtiError::Runtime {
                    error_kind: crate::TorErrorKind::Runtime, msg: "no runtime".into(),
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
        // A controller owns the socket before publication; the task only polls it.
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

        let socket = match bound_sock {
            Some(sock) => {
                let _context = runtime.enter();
                let sock = tokio::net::TcpListener::from_std(sock).map_err(|e| ArtiError::Bind {
                    port: 0, msg: e.to_string(),
                })?;
                Some(Arc::new(ListeningSocket::new(sock)))
            }
            None => None,
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
            let task_sock = socket.clone().unwrap();
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
                                error_kind: crate::TorErrorKind::Runtime, msg: "session limit reached".into(),
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
                                socket: socket.clone(),
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
                                error_kind: crate::TorErrorKind::Runtime, msg: "session limit reached".into(),
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
                                socket: None,
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
                if let Some(socket) = socket {
                    socket.close();
                }
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
                if let Some(socket) = socket {
                    socket.close();
                }
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
    pub(super) fn ffi_list_sessions(&self) -> Vec<SessionInfo> {
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
    pub(super) fn ffi_close_session(&self, session: &SocksSession) {
        close_session_handle(&session.weak, &session.id, session.generation);
    }

    /// Current snapshot for one handle (live, terminal-tombstone, or
    /// `Invalidated` for stale/unknown handles). Never throws.
    pub(super) fn ffi_session_status(&self, session: &SocksSession) -> SessionStatusFfi {
        session.status_snapshot()
    }
}
