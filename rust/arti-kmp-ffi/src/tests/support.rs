//! Reusable offline engine fixtures, recorders and deterministic gate installers.
use super::*;

pub(super) fn test_config() -> ArtiConfig {
    ArtiConfig {
        data_dir: "/tmp/artitor-test".into(),
        socks_port: 0,
        bridges: vec![],
        bridges_enabled: BridgesEnabled::Auto,
        state_dir: None,
        cache_dir: None,
        allow_onion_addrs: true,
        connect_timeout_nanos: 10_000_000_000,
        resolve_timeout_nanos: 10_000_000_000,
    }
}

pub(super) struct Recorder {
    pub(super) statuses: StdMutex<Vec<(TorState, u32, Option<u16>, String)>>,
    pub(super) errors: StdMutex<Vec<ArtiErrorDetail>>,
}

impl StatusListener for Recorder {
    fn on_status(
        &self,
        state: TorState,
        bootstrap_percent: u32,
        socks_port: Option<u16>,
        summary: String,
    ) {
        self.statuses
            .lock()
            .unwrap()
            .push((state, bootstrap_percent, socks_port, summary));
    }
    fn on_log(&self, _line: String) {}
    fn on_error(&self, error: ArtiErrorDetail) {
        self.errors.lock().unwrap().push(error);
    }
}

pub(super) fn test_client(name: &str) -> Arc<TorClient<PreferredRuntime>> {
    // Unique dirs per test: parallel tests must not share dir.sqlite3.
    let dir = std::env::temp_dir().join(format!(
        "artitor-unit-{}-{}-{:?}",
        std::process::id(),
        name,
        std::thread::current().id()
    ));
    let cfg = TorClientConfigBuilder::from_directories(dir.join("state"), dir.join("cache"))
        .build()
        .expect("test config builds offline");
    TorClient::builder()
        .config(cfg)
        .create_unbootstrapped()
        .expect("unbootstrapped client needs no network")
}

pub(super) fn test_shared(rec: Arc<Recorder>) -> Arc<Shared> {
    let shared = Shared::new(1);
    shared.set_listener(rec);
    shared
}

// ========================================================================
// 0.3 Phase 1 — isolation session foundation tests
// ========================================================================

#[derive(Clone)]
pub(super) struct SessionRecorder {
    events: Arc<StdMutex<Vec<(String, SessionState, Option<u16>)>>>,
}

impl SessionRecorder {
    pub(super) fn new() -> Self {
        Self {
            events: Arc::new(StdMutex::new(Vec::new())),
        }
    }

    pub(super) fn events(&self) -> Vec<(String, SessionState, Option<u16>)> {
        self.events.lock().unwrap().clone()
    }

    pub(super) fn last_for(&self, sid: &str) -> Option<(SessionState, Option<u16>)> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|(id, _, _)| id == sid)
            .map(|(_, s, p)| (*s, *p))
    }
}

impl SessionStatusListener for SessionRecorder {
    fn on_session_status(
        &self,
        session_id: String,
        state: SessionState,
        port: Option<u16>,
        _revision: u64,
    ) {
        self.events.lock().unwrap().push((session_id, state, port));
    }
}

/// Engine harness with an owned runtime, a bootstrapped (unbootstrapped
/// in reality — no Tor network) root client, and the engine reporting
/// RUNNING. No network involved; SOCKS binds are loopback-local.
pub(super) fn running_engine(name: &str) -> (Arc<ArtiTor>, SessionRecorder) {
    let engine = ArtiTor::new();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("test runtime");
    {
        let mut inner = engine.inner.lock().unwrap();
        inner.runtime = Some(rt);
        let shared = inner.shared.clone();
        let _runtime_context = inner.runtime.as_ref().unwrap().enter();
        shared.set_client(Some(test_client(name)));
        shared.bootstrap_done.store(true, Ordering::SeqCst);
        *shared.engine_state.lock().unwrap() = TorState::Running;
    }
    (engine, SessionRecorder::new())
}

/// Re-arm an engine whose runtime was consumed by `shutdown()` (models
/// restart into a new generation): fresh runtime + fresh root client on
/// the CURRENT (post-shutdown) generation.
pub(super) fn revive_engine(engine: &ArtiTor, name: &str) {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("test runtime");
    let mut inner = engine.inner.lock().unwrap();
    inner.runtime = Some(rt);
    let shared = inner.shared.clone();
    shared.set_client(Some(test_client(name)));
    shared.bootstrap_done.store(true, Ordering::SeqCst);
    *shared.engine_state.lock().unwrap() = TorState::Running;
}

pub(super) async fn wait_for_session_state(
    rec: &SessionRecorder,
    sid: &str,
    want: SessionState,
    timeout: Duration,
) -> Option<u16> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if let Some((state, port)) = rec.last_for(sid) {
            if state == want {
                return port;
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

// ========================================================================
// Concurrency / race remediation tests
// ========================================================================

#[derive(Clone)]
pub(super) struct ReentrantSessionListener {
    pub(super) engine: Arc<ArtiTor>,
    pub(super) events: Arc<StdMutex<Vec<(String, SessionState, Option<u16>)>>>,
}

impl SessionStatusListener for ReentrantSessionListener {
    fn on_session_status(
        &self,
        session_id: String,
        state: SessionState,
        port: Option<u16>,
        _revision: u64,
    ) {
        self.events.lock().unwrap().push((session_id, state, port));
        let _ = self.engine.has_client();
        let _ = self.engine.socks_port();
    }
}

#[derive(Clone)]
pub(super) struct ReentrantEngineListener {
    pub(super) engine: Weak<ArtiTor>,
    pub(super) statuses: Arc<StdMutex<Vec<TorState>>>,
    pub(super) logs: Arc<StdMutex<Vec<String>>>,
    pub(super) errors: Arc<StdMutex<Vec<ArtiErrorDetail>>>,
}

impl ReentrantEngineListener {
    pub(super) fn new(engine: &Arc<ArtiTor>) -> Self {
        Self {
            engine: Arc::downgrade(engine),
            statuses: Arc::new(StdMutex::new(vec![])),
            logs: Arc::new(StdMutex::new(vec![])),
            errors: Arc::new(StdMutex::new(vec![])),
        }
    }

    fn reenter(&self) {
        if let Some(engine) = self.engine.upgrade() {
            let _ = engine.has_client();
            let _ = engine.socks_port();
        }
    }
}

impl StatusListener for ReentrantEngineListener {
    fn on_status(
        &self,
        state: TorState,
        _bootstrap_percent: u32,
        _socks_port: Option<u16>,
        _summary: String,
    ) {
        self.statuses.lock().unwrap().push(state);
        self.reenter();
    }
    fn on_log(&self, line: String) {
        self.logs.lock().unwrap().push(line);
        self.reenter();
    }
    fn on_error(&self, error: ArtiErrorDetail) {
        self.errors.lock().unwrap().push(error);
    }
}

pub(super) fn install_commit_gate(engine: &ArtiTor) -> Arc<TestCommitGate> {
    let gate = Arc::new(TestCommitGate::default());
    engine.inner.lock().unwrap().test_commit_gate = Some(gate.clone());
    gate
}

pub(super) fn install_listener_probe(engine: &ArtiTor) -> Arc<TestListenerProbe> {
    let probe = Arc::new(TestListenerProbe::default());
    let mut inner = engine.inner.lock().unwrap();
    inner.test_dispatch_count = Some(probe.dispatch_count.clone());
    *inner.shared.test_listener_probe.lock().unwrap() = Some(probe.clone());
    probe
}
