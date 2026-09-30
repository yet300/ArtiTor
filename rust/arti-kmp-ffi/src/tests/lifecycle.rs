//! Root lifecycle, rebuild, bind failure and callback reentry regressions.
use super::*;

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
        matches!(result, Err(ArtiError::Runtime { ref msg, .. }) if msg == "injected spawn_cold failure")
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
