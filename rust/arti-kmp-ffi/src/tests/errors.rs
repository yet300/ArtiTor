//! Typed errors, demotion ordering and engine publication freshness.
use super::*;

#[test]
fn error_detail_preserves_declared_types() {
    // No string parsing: kind discriminant + typed port carry the type.
    let cases: Vec<(ArtiError, ErrorKind, Option<u16>)> = vec![
        (ArtiError::AlreadyRunning, ErrorKind::AlreadyRunning, None),
        (ArtiError::NotRunning, ErrorKind::NotRunning, None),
        (
            ArtiError::Config { msg: "bad".into() },
            ErrorKind::Config,
            None,
        ),
        (
            ArtiError::Bind {
                port: 9050,
                msg: "taken".into(),
            },
            ErrorKind::Bind,
            Some(9050),
        ),
        (
            ArtiError::Bootstrap {
                error_kind: crate::TorErrorKind::Bootstrap,
                msg: "no consensus".into(),
            },
            ErrorKind::Bootstrap,
            None,
        ),
        (
            ArtiError::Runtime {
                error_kind: crate::TorErrorKind::Runtime,
                msg: "boom".into(),
            },
            ErrorKind::Runtime,
            None,
        ),
    ];
    for (err, kind, port) in cases {
        let d = ArtiErrorDetail::from(&err);
        assert_eq!(d.kind, kind, "kind for {err}");
        assert_eq!(
            d.error_kind,
            match kind {
                ErrorKind::AlreadyRunning => crate::TorErrorKind::AlreadyRunning,
                ErrorKind::NotRunning => crate::TorErrorKind::NotRunning,
                ErrorKind::Config => crate::TorErrorKind::Config,
                ErrorKind::Bind => crate::TorErrorKind::Bind,
                ErrorKind::Bootstrap => crate::TorErrorKind::Bootstrap,
                ErrorKind::Runtime => crate::TorErrorKind::Runtime,
            },
            "legacy classification default for {err}"
        );
        assert_eq!(d.port, port, "port for {err}");
        assert!(!d.msg.is_empty(), "msg for {err}");
    }
}

#[test]
fn notify_error_preserves_upstream_category_and_operation_class() {
    let recorder = Arc::new(Recorder {
        statuses: StdMutex::new(vec![]),
        errors: StdMutex::new(vec![]),
    });
    let shared = test_shared(recorder.clone());
    shared.notify_error(
        &ArtiError::Bootstrap {
            msg: "Tor bootstrap failed".into(),
            error_kind: crate::TorErrorKind::Network,
        },
        0,
    );
    let errors = recorder.errors.lock().unwrap();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].kind, ErrorKind::Bootstrap);
    assert_eq!(errors[0].error_kind, crate::TorErrorKind::Network);
}

#[test]
fn notify_error_delivers_typed_detail_before_status() {
    #[derive(Debug, PartialEq)]
    enum RecordedEvent {
        Error(ErrorKind),
        Status(TorState),
    }
    struct OrderedRecorder(StdMutex<Vec<RecordedEvent>>);
    impl StatusListener for OrderedRecorder {
        fn on_status(&self, state: TorState, _: u32, _: Option<u16>, _: String) {
            self.0.lock().unwrap().push(RecordedEvent::Status(state));
        }
        fn on_error(&self, error: ArtiErrorDetail) {
            self.0
                .lock()
                .unwrap()
                .push(RecordedEvent::Error(error.kind));
        }
        fn on_log(&self, _: String) {}
    }
    let rec = Arc::new(OrderedRecorder(StdMutex::new(vec![])));
    let shared = Shared::new(1);
    shared.set_listener(rec.clone());
    shared.notify_error(
        &ArtiError::Bootstrap {
            error_kind: crate::TorErrorKind::Bootstrap,
            msg: "no consensus".into(),
        },
        42,
    );
    assert_eq!(
        *rec.0.lock().unwrap(),
        vec![
            RecordedEvent::Error(ErrorKind::Bootstrap),
            RecordedEvent::Status(TorState::Error)
        ]
    );
}

#[derive(Clone, Copy)]
enum ErrorReentry {
    None,
    ErrorShutdown,
    SessionShutdown,
    SessionPause,
    ErrorPause,
}

struct ErrorFreshnessListener {
    engine: Weak<ArtiTor>,
    action: ErrorReentry,
    events: Arc<StdMutex<Vec<String>>>,
}

impl StatusListener for ErrorFreshnessListener {
    fn on_status(&self, state: TorState, _: u32, _: Option<u16>, _: String) {
        self.events.lock().unwrap().push(format!("{state:?}"));
    }
    fn on_log(&self, _: String) {}
    fn on_error(&self, _: ArtiErrorDetail) {
        self.events.lock().unwrap().push("typed error".into());
        let engine = self.engine.upgrade().unwrap();
        match self.action {
            ErrorReentry::ErrorShutdown => engine.shutdown(),
            ErrorReentry::ErrorPause => engine.pause(),
            _ => {}
        }
    }
}

impl SessionStatusListener for ErrorFreshnessListener {
    fn on_session_status(&self, _: String, state: SessionState, _: Option<u16>, _: u64) {
        self.events
            .lock()
            .unwrap()
            .push(format!("session {state:?}"));
        if state == SessionState::Paused {
            let engine = self.engine.upgrade().unwrap();
            match self.action {
                ErrorReentry::SessionShutdown => engine.shutdown(),
                ErrorReentry::SessionPause
                    if engine.inner.lock().unwrap().shared.engine_state() == TorState::Error =>
                {
                    engine.pause()
                }
                _ => {}
            }
        }
    }
}

fn assert_error_publication_freshness(
    action: ErrorReentry,
    expected: Vec<&str>,
    final_state: TorState,
) {
    let (engine, _) = running_engine("error-publication-freshness");
    let events = Arc::new(StdMutex::new(vec![]));
    let make_listener = || ErrorFreshnessListener {
        engine: Arc::downgrade(&engine),
        action,
        events: events.clone(),
    };
    let session = engine.create_session(Box::new(make_listener())).unwrap();
    let shared = engine.inner.lock().unwrap().shared.clone();
    shared.set_listener(Arc::new(make_listener()));
    events.lock().unwrap().clear();
    let worker = shared.worker_revision.load(Ordering::SeqCst);
    shared.notify_worker_error(
        Some(worker),
        &ArtiError::Runtime {
            error_kind: crate::TorErrorKind::Runtime,
            msg: "injected".into(),
        },
        100,
    );
    let got = events.lock().unwrap().clone();
    assert_eq!(
        got, expected,
        "no stale ERROR after a newer callback lifecycle: {got:?}"
    );
    assert_eq!(
        engine.inner.lock().unwrap().shared.engine_state(),
        final_state
    );
    if final_state == TorState::Off {
        assert!(!engine.has_client());
        assert_eq!(session.status_snapshot().state, SessionState::Invalidated);
        assert_eq!(got.last().unwrap(), "Off");
        let off = got.iter().position(|e| e == "Off").unwrap();
        assert!(!got[off + 1..].iter().any(|e| e == "Error"));
    } else {
        assert_eq!(session.status_snapshot().state, SessionState::Paused);
        assert_eq!(session.status_snapshot().port, None);
        engine.shutdown();
    }
}

#[test]
fn error_session_callback_pause_notifies_all_demoted_siblings() {
    let (engine, _) = running_engine("error-session-pause-siblings");
    let events = Arc::new(StdMutex::new(vec![]));
    let listener = || ErrorFreshnessListener {
        engine: Arc::downgrade(&engine),
        action: ErrorReentry::SessionPause,
        events: events.clone(),
    };
    let recorder = SessionRecorder::new();
    let first = engine.create_session(Box::new(listener())).unwrap();
    let second = engine.create_session(Box::new(recorder.clone())).unwrap();
    let shared = engine.inner.lock().unwrap().shared.clone();
    // Choose whichever hash-map entry publishes first, deterministically.
    let first_id = shared
        .sessions
        .lock()
        .unwrap()
        .keys()
        .next()
        .unwrap()
        .clone();
    {
        let mut sessions = shared.sessions.lock().unwrap();
        for (id, entry) in sessions.iter_mut() {
            entry.status_listener = if id == &first_id {
                Some(Arc::new(listener()))
            } else {
                Some(Arc::new(recorder.clone()))
            };
        }
    }
    shared.set_listener(Arc::new(listener()));
    shared.notify_worker_error(
        Some(shared.worker_revision.load(Ordering::SeqCst)),
        &ArtiError::Runtime {
            error_kind: crate::TorErrorKind::Runtime,
            msg: "injected".into(),
        },
        100,
    );
    let sibling = if first.id() == first_id {
        second.id()
    } else {
        first.id()
    };
    assert_eq!(
        recorder.last_for(&sibling),
        Some((SessionState::Paused, None)),
        "new pause must deliver demotions cancelled by the ERROR transaction"
    );
    assert_eq!(shared.engine_state(), TorState::Paused);
    assert!(!events
        .lock()
        .unwrap()
        .iter()
        .any(|e| e == "typed error" || e == "Error"));
    engine.shutdown();
}

#[test]
fn error_callback_shutdown_publication_finishes_off() {
    assert_error_publication_freshness(
        ErrorReentry::ErrorShutdown,
        vec![
            "session Paused",
            "typed error",
            "session Invalidated",
            "Off",
        ],
        TorState::Off,
    );
}

#[test]
fn error_session_callback_shutdown_suppresses_remaining_error_publications() {
    assert_error_publication_freshness(
        ErrorReentry::SessionShutdown,
        vec!["session Paused", "session Invalidated", "Off"],
        TorState::Off,
    );
}

#[test]
fn error_callback_pause_publication_finishes_paused() {
    assert_error_publication_freshness(
        ErrorReentry::ErrorPause,
        vec!["session Paused", "typed error", "session Paused", "Paused"],
        TorState::Paused,
    );
}

#[test]
fn error_normal_publication_keeps_demotion_typed_error_status_order() {
    assert_error_publication_freshness(
        ErrorReentry::None,
        vec!["session Paused", "typed error", "Error"],
        TorState::Error,
    );
}

struct ErrorSnapshotListener {
    shared: Weak<Shared>,
    sid: String,
    generation: u64,
    observations: Arc<StdMutex<Vec<SessionStatusFfi>>>,
}

impl StatusListener for ErrorSnapshotListener {
    fn on_status(&self, state: TorState, _: u32, _: Option<u16>, _: String) {
        if state == TorState::Error {
            self.observations.lock().unwrap().push(
                self.shared
                    .upgrade()
                    .unwrap()
                    .session_snapshot_for(&self.sid, self.generation),
            );
        }
    }
    fn on_error(&self, _: ArtiErrorDetail) {
        self.observations.lock().unwrap().push(
            self.shared
                .upgrade()
                .unwrap()
                .session_snapshot_for(&self.sid, self.generation),
        );
    }
    fn on_log(&self, _: String) {}
}

#[test]
fn audit_error_callbacks_must_observe_demoted_sessions() {
    let (engine, rec) = running_engine("audit-error-order");
    let session = engine.create_session(Box::new(rec)).unwrap();
    let shared = engine.inner.lock().unwrap().shared.clone();
    let observations = Arc::new(StdMutex::new(vec![]));
    shared.set_listener(Arc::new(ErrorSnapshotListener {
        shared: Arc::downgrade(&shared),
        sid: session.id(),
        generation: session.generation(),
        observations: observations.clone(),
    }));
    shared.notify_error(
        &ArtiError::Runtime {
            error_kind: crate::TorErrorKind::Runtime,
            msg: "audit".into(),
        },
        100,
    );
    let got = observations.lock().unwrap().clone();
    eprintln!("AUDIT session snapshots during on_error/on_status(ERROR): {got:?}");
    engine.shutdown();
    assert!(
        got.iter()
            .all(|s| s.state == SessionState::Paused && s.port.is_none()),
        "ERROR observers must see PAUSED/null"
    );
}
