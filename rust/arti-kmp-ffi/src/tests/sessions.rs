//! Isolation primitive, admission, cap and terminal-handle regressions.
use super::*;

// --- Isolation primitive ------------------------------------------------

#[test]
fn isolated_client_handles_are_distinct() {
    // Each session derives exactly one `root.isolated_client()`; the
    // handles must be pairwise distinct objects (distinct owner tokens by
    // upstream construction, client.rs:1466).
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("test runtime");
    rt.block_on(async {
        let root = test_client("iso-distinct");
        let a = root.isolated_client();
        let b = root.isolated_client();
        assert!(!Arc::ptr_eq(&root, &a), "session A != root");
        assert!(!Arc::ptr_eq(&root, &b), "session B != root");
        assert!(!Arc::ptr_eq(&a, &b), "session A != session B");
    });
}

#[test]
fn fresh_isolation_tokens_are_mutually_incompatible() {
    // Lowest-level accessible primitive: distinct owner tokens never
    // share circuits; a token is always compatible with itself.
    use arti_client::isolation::Isolation;
    use arti_client::IsolationToken;
    let t1 = IsolationToken::new();
    let t2 = IsolationToken::new();
    assert!(t1.compatible(&t1), "reflexive");
    assert!(!t1.compatible(&t2), "distinct owners incompatible");
    assert!(!t2.compatible(&t1), "symmetric");
    assert_ne!(t1, IsolationToken::no_isolation());
    assert_ne!(t2, IsolationToken::no_isolation());
}

#[test]
fn stream_isolation_requires_matching_owner_and_stream() {
    // Models the frozen conjunction rule (isolation.rs: two streams share
    // a circuit iff owner token AND stream prefs both match):
    // - same owner + same stream -> MAY share (within one session);
    // - different owners + SAME stream token -> MUST NOT share
    //   (root vs session, session A vs session B).
    use arti_client::isolation::{Isolation, StreamIsolation};
    use arti_client::IsolationToken;
    let owner_root = IsolationToken::new();
    let owner_a = IsolationToken::new();
    let owner_b = IsolationToken::new();
    let stream_token = IsolationToken::new();
    let mk = |owner: IsolationToken| {
        StreamIsolation::builder()
            .owner_token(owner)
            .stream_isolation(Box::new(stream_token) as Box<dyn Isolation>)
            .build()
            .expect("StreamIsolation builds")
    };
    let root_iso = mk(owner_root);
    let a_iso = mk(owner_a);
    let b_iso = mk(owner_b);
    let a_iso_again = mk(owner_a);
    assert!(
        a_iso.compatible(&a_iso_again),
        "streams inside one session MAY share circuits"
    );
    assert!(
        !a_iso.compatible(&b_iso),
        "session A vs session B MUST NOT share circuits"
    );
    assert!(
        !root_iso.compatible(&a_iso),
        "root vs session MUST NOT share circuits"
    );
    assert!(
        !b_iso.compatible(&root_iso),
        "session vs root MUST NOT share circuits (symmetric)"
    );
}

// --- Session id ----------------------------------------------------------

#[test]
fn session_ids_are_unique_and_opaque() {
    let a = next_session_id();
    let b = next_session_id();
    assert_ne!(a, b, "unique within process generation");
    assert!(!a.is_empty() && !b.is_empty());
}

// --- Registry / lifecycle -------------------------------------------------

#[tokio::test]
async fn create_while_running_binds_and_registers_active() {
    let (engine, rec) = running_engine("create-active");
    let session = engine
        .create_session(Box::new(rec.clone()))
        .expect("create while RUNNING");
    let snap = engine.session_status(&session);
    assert_eq!(snap.state, SessionState::Active);
    let port = snap.port.expect("ACTIVE carries an endpoint");
    assert_ne!(port, 0);
    // Loopback listener really accepts.
    let probe = tokio::net::TcpStream::connect(("127.0.0.1", port)).await;
    assert!(probe.is_ok(), "session listener must accept");
    drop(probe);
    // Live snapshot lists exactly this session.
    let list = engine.list_sessions();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].id, session.id());
    assert_eq!(list[0].state, SessionState::Active);
    assert_eq!(list[0].port, Some(port));
    // Registry holds a distinct isolated client (dispatch proof by
    // construction: the accept loop clones the entry's client).
    let (root, isolated) = {
        let inner = engine.inner.lock().unwrap();
        let sessions = inner.shared.sessions.lock().unwrap();
        let entry = sessions.get(&session.id()).expect("registered");
        assert_eq!(entry.generation, session.generation());
        (inner.shared.client().unwrap(), entry.isolated.clone())
    };
    assert!(!Arc::ptr_eq(&root, &isolated));
    engine.close_session(&session);
    engine.shutdown();
}

#[tokio::test]
async fn create_while_paused_registers_paused_without_listener() {
    let (engine, rec) = running_engine("create-paused");
    engine.pause();
    assert_eq!(
        engine.inner.lock().unwrap().shared.engine_state(),
        TorState::Paused
    );
    let session = engine
        .create_session(Box::new(rec.clone()))
        .expect("create while PAUSED");
    let snap = engine.session_status(&session);
    assert_eq!(snap.state, SessionState::Paused);
    assert_eq!(snap.port, None, "PAUSED carries no endpoint");
    engine.close_session(&session);
    engine.shutdown();
}

#[tokio::test]
async fn create_rejected_without_live_client() {
    let (engine, rec) = running_engine("create-rejected");
    for state in [
        TorState::Off,
        TorState::Starting,
        TorState::Bootstrapping,
        TorState::Stopping,
        TorState::Error,
    ] {
        {
            let inner = engine.inner.lock().unwrap();
            *inner.shared.engine_state.lock().unwrap() = state;
        }
        let err = engine
            .create_session(Box::new(rec.clone()))
            .expect_err("must reject");
        assert!(
            matches!(err, ArtiError::NotRunning),
            "state {state:?} must reject with NotRunning, got {err}"
        );
    }
    // Rejected creations leak nothing.
    assert!(engine.list_sessions().is_empty());
    engine.shutdown();
}

#[tokio::test]
async fn close_is_idempotent_and_never_throws() {
    let (engine, rec) = running_engine("close-idem");
    let session = engine.create_session(Box::new(rec.clone())).unwrap();
    let sid = session.id();
    session.close_session();
    assert_eq!(engine.session_status(&session).state, SessionState::Closed);
    // Second close: no-op, no second publication.
    session.close_session();
    engine.close_session(&session);
    let closed_events = rec
        .events()
        .into_iter()
        .filter(|(id, s, _)| id == &sid && *s == SessionState::Closed)
        .count();
    assert_eq!(closed_events, 1, "CLOSED published exactly once");
    assert!(engine.list_sessions().is_empty(), "closed leaves the list");
    engine.shutdown();
}

#[tokio::test]
async fn unknown_and_stale_handles_are_noop_invalidated() {
    let (engine, rec) = running_engine("stale-noop");
    let rt_handle = {
        let inner = engine.inner.lock().unwrap();
        inner.runtime.as_ref().unwrap().handle().clone()
    };
    let _ = rt_handle;
    let live = engine
        .create_session(Box::new(rec.clone()))
        .expect("live session");
    let live_id = live.id();
    let weak = {
        let inner = engine.inner.lock().unwrap();
        Arc::downgrade(&inner.shared)
    };

    // Unknown id: no-op close, INVALIDATED status.
    let ghost = SocksSession {
        id: "sess-deadbeefdeadbeef".into(),
        generation: live.generation(),
        weak: weak.clone(),
    };
    ghost.close_session();
    assert_eq!(ghost.status_snapshot().state, SessionState::Invalidated);

    // Right id, wrong generation (id collision across restart): never
    // attaches, never disturbs the live entry.
    let impostor = SocksSession {
        id: live_id.clone(),
        generation: live.generation().wrapping_add(1000),
        weak: weak.clone(),
    };
    assert_eq!(impostor.status_snapshot().state, SessionState::Invalidated);
    impostor.close_session();
    assert_eq!(
        engine.session_status(&live).state,
        SessionState::Active,
        "impostor close must not disturb the live entry"
    );

    // Dead engine (Weak upgrade failure): no-op, INVALIDATED.
    let orphan = SocksSession {
        id: live_id.clone(),
        generation: live.generation(),
        weak: Weak::new(),
    };
    orphan.close_session();
    assert_eq!(orphan.status_snapshot().state, SessionState::Invalidated);

    engine.close_session(&live);
    engine.shutdown();
}

#[tokio::test]
async fn session_limit_is_enforced_without_engine_damage() {
    let (engine, rec) = running_engine("sess-limit");
    let mut handles = Vec::new();
    for _ in 0..MAX_SESSIONS {
        handles.push(engine.create_session(Box::new(rec.clone())).unwrap());
    }
    assert_eq!(engine.list_sessions().len(), MAX_SESSIONS);
    let err = engine
        .create_session(Box::new(rec.clone()))
        .expect_err("over the cap");
    match err {
        ArtiError::Runtime { msg } => assert!(
            msg.contains("session limit reached"),
            "unexpected message: {msg}"
        ),
        other => panic!("expected Runtime limit, got {other}"),
    }
    // Engine untouched: still RUNNING, existing sessions live.
    assert_eq!(
        engine.inner.lock().unwrap().shared.engine_state(),
        TorState::Running
    );
    assert_eq!(engine.list_sessions().len(), MAX_SESSIONS);
    // Closing one frees a slot.
    handles.pop().unwrap().close_session();
    let _replacement = engine
        .create_session(Box::new(rec.clone()))
        .expect("slot freed");
    for h in handles {
        h.close_session();
    }
    engine.shutdown();
}

#[test]
fn native_tombstones_are_bounded_diagnostics() {
    let (engine, rec) = running_engine("tombstone-bounds");
    engine.pause();
    let first = engine.create_session(Box::new(rec.clone())).unwrap();
    first.close_session();
    for _ in 0..160 {
        let s = engine.create_session(Box::new(rec.clone())).unwrap();
        s.close_session();
    }
    let shared = engine.inner.lock().unwrap().shared.clone();
    assert!(shared.tombstones.lock().unwrap().len() <= MAX_SESSIONS * 4);
    assert_eq!(first.status_snapshot().state, SessionState::Invalidated);
    engine.shutdown();
}
