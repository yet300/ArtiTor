//! Owned cold bootstrap worker, progress and root-client installation fences.
use super::*;

/// Spawn a cold bootstrap + SOCKS task on the owned runtime, creating the
/// runtime on first use. The task reports typed failures via `on_error`.
pub(super) fn spawn_cold(inner: &mut Inner, config: ArtiConfig) -> Result<(), ArtiError> {
    inner.shared.advance_engine_publication_under_gate();
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

    shared.advance_engine_publication_under_gate();
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
