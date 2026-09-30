//! Root listener lifecycle and ready barrier preceding optional session rebinds.
use super::*;

/// Internal signal used by `run_socks` to notify that root bind succeeded
/// and session rebinds may begin.
pub(super) struct RootBindReady {
    rx: oneshot::Receiver<()>,
}

pub(super) fn spawn_socks(inner: &mut Inner, socks_port: u16) -> Result<RootBindReady, ArtiError> {
    let runtime = inner.runtime.as_ref().ok_or_else(|| ArtiError::Runtime {
        error_kind: crate::TorErrorKind::Runtime,
        msg: "no runtime".into(),
    })?;
    let client = inner.shared.client().ok_or(ArtiError::NotRunning)?;
    let shared = inner.shared.clone();
    let (ready_tx, ready_rx) = oneshot::channel();
    shared.advance_engine_publication_under_gate();
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
pub(super) async fn wait_root_then_rebind_sessions(
    ready: RootBindReady,
    shared: Arc<Shared>,
    handle: tokio::runtime::Handle,
) {
    // Wait for root bind to succeed (signaled by run_socks after reporting RUNNING).
    if ready.rx.await.is_ok() {
        rebind_paused_sessions(&shared, &handle);
    }
}

#[cfg(test)]
pub(super) async fn run_socks(
    client: Arc<TorClient<PreferredRuntime>>,
    socks_port: u16,
    shared: Arc<Shared>,
    ready_tx: Option<oneshot::Sender<()>>,
) -> Result<(), ArtiError> {
    let revision = shared.worker_revision.load(Ordering::SeqCst);
    run_socks_worker(client, socks_port, shared, ready_tx, revision).await
}

pub(super) async fn run_socks_worker(
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
            error_kind: crate::TorErrorKind::Runtime,
            msg: format!("local_addr: {e}"),
        })?
        .port();

    if let Some(l) = shared.listener() {
        l.on_log(format!("SOCKS listening on 127.0.0.1:{actual_port}"));
    }
    let (mut shutdown_rx, publication) = {
        let _transition = shared.transition_gate.lock().unwrap();
        if shared.worker_revision.load(Ordering::SeqCst) != revision {
            return Ok(());
        }
        let rx = shared.install_socks_shutdown();
        *shared.engine_state.lock().unwrap() = TorState::Running;
        shared.bound_port.store(actual_port, Ordering::SeqCst);
        (rx, shared.advance_engine_publication_under_gate())
    };
    if !shared.publication_is_current(publication) {
        return Ok(());
    }
    if let Some(l) = shared.listener() {
        l.on_status(
            TorState::Running,
            100,
            Some(actual_port),
            "proxy ready".into(),
        );
    }

    if !shared.publication_is_current(publication) {
        return Ok(());
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
