//! ACTIVE publication barriers, callback reentry and revision ordering.
use super::*;

#[test]
fn session_listener_does_not_dispatch_before_active_commit() {
    let (engine, rec) = running_engine("dispatch-precommit");
    let gate = install_commit_gate(&engine);
    let probe = install_listener_probe(&engine);
    let create_engine = engine.clone();
    let create_rec = rec.clone();
    let create = std::thread::spawn(move || create_engine.create_session(Box::new(create_rec)));
    gate.arrived.wait();
    let port = gate.bound_port.lock().unwrap().unwrap();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let mut stream = rt
        .block_on(tokio::net::TcpStream::connect(("127.0.0.1", port)))
        .unwrap();
    // Queue enough SOCKS bytes to exercise the real handler after release.
    rt.block_on(stream.write_all(&[0x05, 0x01, 0x00])).unwrap();
    probe.assert_barrier_pending();
    assert!(engine.list_sessions().is_empty());
    assert!(rec.events().is_empty());
    gate.release.signal();
    let session = create.join().unwrap().unwrap();
    let mut greeting = [0u8; 2];
    rt.block_on(stream.read_exact(&mut greeting)).unwrap();
    assert_eq!(greeting, [0x05, 0x00]);
    assert_eq!(probe.dispatch_count.load(Ordering::SeqCst), 1);
    assert_eq!(
        rec.last_for(&session.id()),
        Some((SessionState::Active, Some(port)))
    );
    engine.shutdown();
}

#[derive(Clone, Copy)]
enum PublicationAction {
    Close,
    Pause,
}

struct PublicationReentryListener {
    engine: Weak<ArtiTor>,
    probe: Arc<TestListenerProbe>,
    events: SessionRecorder,
    action: PublicationAction,
    // Keep the listener runnable after the callback mutates the registry,
    // so this test observes the dispatch gate rather than task abortion.
    retained_task: Mutex<Option<(JoinHandle<()>, Option<oneshot::Sender<()>>)>>,
    traffic: Mutex<Option<std::net::TcpStream>>,
}

impl SessionStatusListener for Arc<PublicationReentryListener> {
    fn on_session_status(
        &self,
        sid: String,
        state: SessionState,
        port: Option<u16>,
        revision: u64,
    ) {
        self.events
            .on_session_status(sid.clone(), state, port, revision);
        let engine = self.engine.upgrade().unwrap();
        let _ = engine.has_client();
        let _ = engine.socks_port();
        if state != SessionState::Active {
            return;
        }
        let shared = engine.inner.lock().unwrap().shared.clone();
        let snapshot = shared.session_snapshot_for(&sid, shared.generation);
        assert_eq!(
            snapshot.state,
            SessionState::Active,
            "commit must precede callback"
        );
        assert_eq!(snapshot.port, port);
        self.probe.assert_barrier_pending();
        let mut traffic = std::net::TcpStream::connect(("127.0.0.1", port.unwrap())).unwrap();
        std::io::Write::write_all(&mut traffic, &[0x05, 0x01, 0x00]).unwrap();
        *self.traffic.lock().unwrap() = Some(traffic);
        {
            let mut sessions = shared.sessions.lock().unwrap();
            let entry = sessions.get_mut(&sid).unwrap();
            *self.retained_task.lock().unwrap() = Some((
                entry.listener_task.take().unwrap(),
                entry.shutdown_tx.take(),
            ));
        }
        match self.action {
            PublicationAction::Close => {
                close_session_handle(&Arc::downgrade(&shared), &sid, shared.generation)
            }
            PublicationAction::Pause => engine.pause(),
        }
    }
}

fn assert_publication_reentry(rebind: bool, action: PublicationAction) {
    let (engine, events) = running_engine("publication-reentry");
    if rebind {
        engine.pause();
    }
    let probe = install_listener_probe(&engine);
    let listener = Arc::new(PublicationReentryListener {
        engine: Arc::downgrade(&engine),
        probe: probe.clone(),
        events: events.clone(),
        action,
        retained_task: Mutex::new(None),
        traffic: Mutex::new(None),
    });
    let session = engine.create_session(Box::new(listener.clone())).unwrap();
    if rebind {
        let inner = engine.inner.lock().unwrap();
        let shared = inner.shared.clone();
        let handle = inner.runtime.as_ref().unwrap().handle().clone();
        *shared.engine_state.lock().unwrap() = TorState::Running;
        drop(inner);
        rebind_paused_sessions(&shared, &handle);
    }
    // The queued SOCKS greeting is accepted only after ACTIVE publication
    // returns; its dispatch is rejected because that callback closed/paused.
    probe.dispatch_checked.wait();
    assert_eq!(probe.dispatch_count.load(Ordering::SeqCst), 0);
    let expected = match action {
        PublicationAction::Close => SessionState::Closed,
        PublicationAction::Pause => SessionState::Paused,
    };
    assert_eq!(session.status_snapshot().state, expected);
    assert_eq!(session.status_snapshot().port, None);
    assert_eq!(events.last_for(&session.id()), Some((expected, None)));
    let history: Vec<_> = events.events().into_iter().map(|(_, s, _)| s).collect();
    assert_eq!(
        history,
        if rebind {
            vec![SessionState::Paused, SessionState::Active, expected]
        } else {
            vec![SessionState::Active, expected]
        }
    );
    let (task, shutdown) = listener.retained_task.lock().unwrap().take().unwrap();
    task.abort();
    drop(shutdown);
    engine.shutdown();
}

#[test]
fn create_active_callback_close_precedes_barrier_and_rejects_dispatch() {
    assert_publication_reentry(false, PublicationAction::Close);
}

#[test]
fn create_active_callback_pause_precedes_barrier_and_rejects_dispatch() {
    assert_publication_reentry(false, PublicationAction::Pause);
}

#[test]
fn rebind_active_callback_close_precedes_barrier_and_rejects_dispatch() {
    assert_publication_reentry(true, PublicationAction::Close);
}

#[test]
fn rebind_active_callback_pause_precedes_barrier_and_rejects_dispatch() {
    assert_publication_reentry(true, PublicationAction::Pause);
}

#[test]
fn delayed_active_publication_carries_older_revision_than_pause() {
    struct DelayedListener {
        gate: Arc<TestCommitGate>,
        events: Arc<StdMutex<Vec<(SessionState, u64)>>>,
    }
    impl SessionStatusListener for DelayedListener {
        fn on_session_status(
            &self,
            _: String,
            state: SessionState,
            port: Option<u16>,
            revision: u64,
        ) {
            if state == SessionState::Active {
                self.gate.arrive_and_wait(port);
            }
            self.events.lock().unwrap().push((state, revision));
        }
    }
    let (engine, _) = running_engine("delayed-publication-revision");
    let gate = Arc::new(TestCommitGate::default());
    let events = Arc::new(StdMutex::new(vec![]));
    let e = engine.clone();
    let g = gate.clone();
    let ev = events.clone();
    let worker = std::thread::spawn(move || {
        e.create_session(Box::new(DelayedListener {
            gate: g,
            events: ev,
        }))
    });
    gate.arrived.wait();
    engine.pause();
    gate.release.signal();
    let session = worker.join().unwrap().unwrap();
    assert_eq!(
        *events.lock().unwrap(),
        vec![(SessionState::Paused, 2), (SessionState::Active, 1)]
    );
    assert_eq!(session.status_snapshot().revision, 2);
    engine.shutdown();
}
