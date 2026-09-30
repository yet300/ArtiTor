//! Session accept/dispatch identity and revision-checked listener failure handling.
use super::*;

/// A dead listener may demote only the exact ACTIVE instance that owns it.
pub(in crate::engine) fn session_listener_failed(
    weak: &Weak<Shared>,
    sid: &str,
    generation: u64,
    revision: u64,
) {
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
pub(in crate::engine) async fn run_session_listener(
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
