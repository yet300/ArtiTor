//! Strong resource release and per-surface task ownership.
use super::*;

#[tokio::test]
async fn shutdown_invalidates_all_and_releases_strong_refs() {
    let (engine, rec) = running_engine("shutdown-inv");
    let a = engine.create_session(Box::new(rec.clone())).unwrap();
    let b = engine.create_session(Box::new(rec.clone())).unwrap();
    let (gen, old_shared, weak_iso_a, weak_root) = {
        let inner = engine.inner.lock().unwrap();
        let sessions = inner.shared.sessions.lock().unwrap();
        let iso: Arc<TorClient<PreferredRuntime>> = sessions.get(&a.id()).unwrap().isolated.clone();
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
