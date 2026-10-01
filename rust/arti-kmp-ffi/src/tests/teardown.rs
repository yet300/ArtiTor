//! Real offline listeners with callback-held task destruction. No Tor network.
use super::*;
use std::net::{SocketAddr, TcpStream};
use std::sync::{mpsc, Condvar};

struct HeldRoot {
    published: mpsc::Sender<u16>,
    release: (StdMutex<bool>, Condvar),
    hold_log: bool,
}
impl HeldRoot {
    fn release(&self) {
        *self.release.0.lock().unwrap() = true;
        self.release.1.notify_all();
    }
    fn hold(&self, port: u16) {
        self.published.send(port).unwrap();
        let (released, _) = self
            .release
            .1
            .wait_timeout_while(
                self.release.0.lock().unwrap(),
                Duration::from_secs(10),
                |done| !*done,
            )
            .unwrap();
        assert!(*released, "callback watchdog expired");
    }
}
impl StatusListener for HeldRoot {
    fn on_status(&self, state: TorState, _: u32, port: Option<u16>, _: String) {
        if state == TorState::Running && !self.hold_log {
            self.hold(port.unwrap());
        }
    }
    fn on_log(&self, line: String) {
        if self.hold_log && line.starts_with("SOCKS listening on 127.0.0.1:") {
            self.hold(line.rsplit(':').next().unwrap().parse().unwrap());
        }
    }
    fn on_error(&self, _: ArtiErrorDetail) {}
}
impl StatusListener for Arc<HeldRoot> {
    fn on_status(&self, s: TorState, p: u32, port: Option<u16>, text: String) {
        self.as_ref().on_status(s, p, port, text);
    }
    fn on_log(&self, line: String) {
        self.as_ref().on_log(line);
    }
    fn on_error(&self, error: ArtiErrorDetail) {
        self.as_ref().on_error(error);
    }
}
struct ReleaseHeldRoot(Arc<HeldRoot>);
impl Drop for ReleaseHeldRoot {
    fn drop(&mut self) {
        self.0.release();
    }
}
struct Fixture {
    engine: Arc<ArtiTor>,
    shared: Arc<Shared>,
    a: Arc<SocksSession>,
    b: Arc<SocksSession>,
    root_port: u16,
    a_port: u16,
    b_port: u16,
    held: Arc<HeldRoot>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.held.release();
        self.engine.shutdown();
    }
}
fn held_fixture(name: &str) -> Fixture {
    let engine = ArtiTor::new();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    {
        let mut inner = engine.inner.lock().unwrap();
        inner.runtime = Some(rt);
        let _context = inner.runtime.as_ref().unwrap().enter();
        inner.shared.set_client(Some(test_client(name)));
        inner.shared.bootstrap_done.store(true, Ordering::SeqCst);
        *inner.shared.engine_state.lock().unwrap() = TorState::Running;
    }
    let a = engine
        .create_session(Box::new(SessionRecorder::new()))
        .unwrap();
    let b = engine
        .create_session(Box::new(SessionRecorder::new()))
        .unwrap();
    let a_port = a.status_snapshot().port.unwrap();
    let b_port = b.status_snapshot().port.unwrap();
    let (tx, rx) = mpsc::channel();
    let held = Arc::new(HeldRoot {
        published: tx,
        release: (StdMutex::new(false), Condvar::new()),
        hold_log: false,
    });
    let shared = {
        let mut inner = engine.inner.lock().unwrap();
        let shared = inner.shared.clone();
        shared.set_listener(held.clone());
        let client = shared.client().unwrap();
        let revision = shared.worker_revision.load(Ordering::SeqCst);
        let task_shared = shared.clone();
        inner.worker = Some(inner.runtime.as_ref().unwrap().spawn(async move {
            root_socks::run_socks_worker(client, 0, task_shared, None, revision)
                .await
                .unwrap();
        }));
        shared
    };
    let root_port = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    Fixture {
        engine,
        shared,
        a,
        b,
        root_port,
        a_port,
        b_port,
        held,
    }
}
fn refused(port: u16) {
    let result = TcpStream::connect_timeout(
        &SocketAddr::from(([127, 0, 0, 1], port)),
        Duration::from_secs(1),
    );
    assert!(matches!(&result, Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused),
        "old TCP listener must refuse immediately after teardown returns: port={port}, result={result:?}");
}
fn accepts(port: u16) {
    TcpStream::connect_timeout(
        &SocketAddr::from(([127, 0, 0, 1], port)),
        Duration::from_secs(1),
    )
    .unwrap();
}
fn bounded_call(engine: &Arc<ArtiTor>, action: fn(&ArtiTor)) {
    let engine = engine.clone();
    let (tx, rx) = mpsc::channel();
    let thread = std::thread::spawn(move || {
        action(&engine);
        tx.send(()).unwrap();
    });
    rx.recv_timeout(Duration::from_secs(5))
        .expect("lifecycle call must not join callback-held worker");
    thread.join().unwrap();
}

#[derive(Clone, Copy)]
enum Reentry {
    Pause,
    Shutdown,
    Close,
}
struct ReentrantRoot {
    engine: Weak<ArtiTor>,
    session: Arc<SocksSession>,
    action: Reentry,
    done: mpsc::Sender<u16>,
}
impl StatusListener for ReentrantRoot {
    fn on_status(&self, state: TorState, _: u32, port: Option<u16>, _: String) {
        let engine = self.engine.upgrade().unwrap();
        let _ = engine.has_client();
        let _ = engine.socks_port();
        let _ = engine.list_sessions();
        if state == TorState::Running {
            match self.action {
                Reentry::Pause => engine.pause(),
                Reentry::Shutdown => engine.shutdown(),
                Reentry::Close => self.session.close_session(),
            }
            self.done.send(port.unwrap()).unwrap();
        }
    }
    fn on_log(&self, _: String) {}
    fn on_error(&self, _: ArtiErrorDetail) {}
}
struct ReentrantSession {
    engine: Weak<ArtiTor>,
    paused: Arc<AtomicBool>,
}
impl SessionStatusListener for ReentrantSession {
    fn on_session_status(&self, _: String, state: SessionState, _: Option<u16>, _: u64) {
        let engine = self.engine.upgrade().unwrap();
        let _ = engine.has_client();
        let _ = engine.socks_port();
        let _ = engine.list_sessions();
        if state == SessionState::Paused {
            self.paused.store(true, Ordering::SeqCst);
        }
        if state == SessionState::Closed {
            engine.shutdown();
        }
    }
}
#[test]
fn callback_reentry_pause_shutdown_and_close_do_not_join_their_worker() {
    for (i, action) in [Reentry::Pause, Reentry::Shutdown, Reentry::Close]
        .into_iter()
        .enumerate()
    {
        let (engine, _) = running_engine(&format!("teardown-reentry-{i}"));
        let paused = Arc::new(AtomicBool::new(false));
        let session = engine
            .create_session(Box::new(ReentrantSession {
                engine: Arc::downgrade(&engine),
                paused: paused.clone(),
            }))
            .unwrap();
        let session_port = session.status_snapshot().port.unwrap();
        // Preserve the native CLOSED tombstone while its callback shuts down
        // the engine; public Kotlin terminal latching is covered separately.
        let retained_shared = engine.inner.lock().unwrap().shared.clone();
        let (tx, rx) = mpsc::channel();
        {
            let mut inner = engine.inner.lock().unwrap();
            let shared = inner.shared.clone();
            shared.set_listener(Arc::new(ReentrantRoot {
                engine: Arc::downgrade(&engine),
                session: session.clone(),
                action,
                done: tx,
            }));
            let client = shared.client().unwrap();
            let revision = shared.worker_revision.load(Ordering::SeqCst);
            inner.worker = Some(inner.runtime.as_ref().unwrap().spawn(async move {
                root_socks::run_socks_worker(client, 0, shared, None, revision)
                    .await
                    .unwrap();
            }));
        }
        let root_port = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("callback reentry deadlocked");
        refused(root_port);
        refused(session_port);
        match action {
            Reentry::Pause => {
                assert!(paused.load(Ordering::SeqCst));
                assert_eq!(session.status_snapshot().state, SessionState::Paused);
            }
            Reentry::Shutdown => {
                assert_eq!(session.status_snapshot().state, SessionState::Invalidated)
            }
            Reentry::Close => assert_eq!(session.status_snapshot().state, SessionState::Closed),
        }
        engine.shutdown();
        drop(retained_shared);
    }
}

#[test]
fn root_pause_closes_tcp_while_callback_held() {
    for iteration in 0..10 {
        let f = held_fixture(&format!("root-teardown-{iteration}"));
        bounded_call(&f.engine, ArtiTor::pause);
        assert_eq!(f.shared.engine_state(), TorState::Paused);
        assert_eq!(f.shared.bound_port.load(Ordering::SeqCst), 0);
        assert!(f.engine.has_client());
        refused(f.root_port);
        println!("ROOT_TEARDOWN_PASS,iteration={iteration},old_tcp_refused=true,callback_still_held=true");
    }
}
#[test]
fn session_pause_closes_tcp_while_runtime_callback_held() {
    let f = held_fixture("session-teardown");
    let isolated = f
        .shared
        .sessions
        .lock()
        .unwrap()
        .get(&f.a.id())
        .unwrap()
        .isolated
        .clone();
    bounded_call(&f.engine, ArtiTor::pause);
    assert_eq!(f.a.status_snapshot().state, SessionState::Paused);
    assert_eq!(f.a.status_snapshot().port, None);
    assert!(Arc::ptr_eq(
        &isolated,
        &f.shared
            .sessions
            .lock()
            .unwrap()
            .get(&f.a.id())
            .unwrap()
            .isolated
    ));
    refused(f.a_port);
    refused(f.b_port);
}
#[test]
fn close_closes_tcp_without_affecting_root_or_sibling() {
    let f = held_fixture("close-teardown");
    f.a.close_session();
    assert_eq!(f.a.status_snapshot().state, SessionState::Closed);
    assert_eq!(f.a.status_snapshot().port, None);
    refused(f.a_port);
    accepts(f.root_port);
    accepts(f.b_port);
    f.a.close_session();
    assert_eq!(f.a.status_snapshot().state, SessionState::Closed);
    assert_eq!(f.b.status_snapshot().state, SessionState::Active);
    assert_eq!(f.shared.engine_state(), TorState::Running);
}
#[test]
fn shutdown_closes_root_and_session_tcp_while_callback_held() {
    let f = held_fixture("shutdown-teardown");
    bounded_call(&f.engine, ArtiTor::shutdown);
    refused(f.root_port);
    refused(f.a_port);
    refused(f.b_port);
    assert_eq!(f.a.status_snapshot().state, SessionState::Invalidated);
    assert_eq!(
        f.engine.inner.lock().unwrap().shared.engine_state(),
        TorState::Off
    );
    assert!(!f.engine.has_client());
}
#[tokio::test]
async fn restart_keeps_stale_tcp_closed_and_handles_invalid() {
    let f = held_fixture("restart-teardown");
    bounded_call(&f.engine, ArtiTor::shutdown);
    refused(f.root_port);
    refused(f.a_port);
    f.held.release();
    revive_engine(&f.engine, "restart-teardown-new");
    let c = f
        .engine
        .create_session(Box::new(SessionRecorder::new()))
        .unwrap();
    assert_ne!(c.generation(), f.a.generation());
    assert_ne!(c.id(), f.a.id());
    f.a.close_session();
    assert_eq!(f.a.status_snapshot().state, SessionState::Invalidated);
    assert_eq!(c.status_snapshot().state, SessionState::Active);
    accepts(c.status_snapshot().port.unwrap());
    c.close_session();
}

#[test]
fn root_worker_exit_closes_its_socket_even_when_controller_is_retained() {
    let f = held_fixture("root-worker-exit");
    // Exercise task completion itself, without the lifecycle's direct close.
    let tx = f.shared.socks_shutdown.lock().unwrap().take().unwrap();
    tx.send(()).unwrap();
    let (handle, task) = {
        let mut inner = f.engine.inner.lock().unwrap();
        (
            inner.runtime.as_ref().unwrap().handle().clone(),
            inner.worker.take().unwrap(),
        )
    };
    f.held.release();
    handle.block_on(async {
        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap();
    });
    refused(f.root_port);
}

#[test]
fn fixed_port_resume_log_held_pause_and_shutdown_close_tcp() {
    for (i, action) in [ArtiTor::pause as fn(&ArtiTor), ArtiTor::shutdown]
        .into_iter()
        .enumerate()
    {
        let f = held_fixture(&format!("log-held-teardown-{i}"));
        f.engine.pause();
        refused(f.root_port);
        f.held.release();
        let mut config = test_config();
        config.socks_port = f.root_port;
        f.engine.inner.lock().unwrap().last_config = Some(config);
        let (tx, rx) = mpsc::channel();
        let held = Arc::new(HeldRoot {
            published: tx,
            release: (StdMutex::new(false), Condvar::new()),
            hold_log: true,
        });
        let _release = ReleaseHeldRoot(held.clone());
        f.engine.resume(Box::new(held)).unwrap();
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            f.root_port
        );
        bounded_call(&f.engine, action);
        refused(f.root_port);
    }
}

#[test]
fn root_listening_log_reentry_pause_and_shutdown_close_before_callback_returns() {
    struct LogReentry {
        engine: Weak<ArtiTor>,
        action: fn(&ArtiTor),
        done: mpsc::Sender<u16>,
    }
    impl StatusListener for LogReentry {
        fn on_status(&self, state: TorState, _: u32, _: Option<u16>, _: String) {
            assert_ne!(
                state,
                TorState::Running,
                "cancelled prepublication worker published RUNNING"
            );
            let engine = self.engine.upgrade().unwrap();
            let _ = engine.has_client();
            let _ = engine.socks_port();
        }
        fn on_log(&self, line: String) {
            if line.starts_with("SOCKS listening on 127.0.0.1:") {
                let port = line.rsplit(':').next().unwrap().parse().unwrap();
                let engine = self.engine.upgrade().unwrap();
                (self.action)(&engine);
                refused(port);
                self.done.send(port).unwrap();
            }
        }
        fn on_error(&self, _: ArtiErrorDetail) {}
    }
    for (i, action) in [ArtiTor::pause as fn(&ArtiTor), ArtiTor::shutdown]
        .into_iter()
        .enumerate()
    {
        let (engine, _) = running_engine(&format!("log-reentry-{i}"));
        let (tx, rx) = mpsc::channel();
        {
            let mut inner = engine.inner.lock().unwrap();
            let shared = inner.shared.clone();
            shared.set_listener(Arc::new(LogReentry {
                engine: Arc::downgrade(&engine),
                action,
                done: tx,
            }));
            let client = shared.client().unwrap();
            let revision = shared.worker_revision.load(Ordering::SeqCst);
            inner.worker = Some(inner.runtime.as_ref().unwrap().spawn(async move {
                root_socks::run_socks_worker(client, 0, shared, None, revision)
                    .await
                    .unwrap();
            }));
        }
        refused(
            rx.recv_timeout(Duration::from_secs(5))
                .expect("log callback self-deadlocked"),
        );
        engine.shutdown();
    }
}
