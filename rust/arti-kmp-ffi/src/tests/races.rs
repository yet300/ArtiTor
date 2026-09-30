//! Deterministic admission and lifecycle transaction races.
use super::*;

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
    assert_eq!(
        results
            .iter()
            .filter(
                |r| matches!(r, Err(ArtiError::Runtime { msg, .. }) if msg == "session limit reached")
            )
            .count(),
        8
    );
    assert_eq!(engine.list_sessions().len(), MAX_SESSIONS);
    assert!(engine.list_sessions().iter().all(|s| s.state
        == if paused {
            SessionState::Paused
        } else {
            SessionState::Active
        }));
    engine.shutdown();
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
            error_kind: crate::TorErrorKind::Runtime,
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
fn old_root_worker_cannot_publish_after_pause_or_replacement() {
    let (engine, _) = running_engine("worker-revision");
    let shared = engine.inner.lock().unwrap().shared.clone();
    let revision = shared.worker_revision.load(Ordering::SeqCst);
    engine.pause();
    assert!(!shared.report_worker(revision, TorState::Running, 100, Some(9999), "stale"));
    shared.notify_worker_error(
        Some(revision),
        &ArtiError::Runtime {
            error_kind: crate::TorErrorKind::Runtime,
            msg: "stale".into(),
        },
        100,
    );
    assert_eq!(shared.engine_state(), TorState::Paused);
    assert_eq!(engine.socks_port(), None);
    engine.shutdown();
}
