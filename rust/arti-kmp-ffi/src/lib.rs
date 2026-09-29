//! Thin UniFFI surface over `arti-client` (Tor in Rust) for Kotlin Multiplatform.
//!
//! Design goals (see repo docs/adr and docs/api-lifecycle-bitchat.md):
//! - The async tokio runtime is owned *inside* this crate; callers never block their main thread.
//! - Bootstrap progress is a first-class signal sourced from `TorClient::bootstrap_events()`
//!   (`BootstrapStatus::as_frac()`), NOT scraped from log lines.
//! - Lifecycle splits bootstrap from SOCKS: [pause] keeps the client; [resume] rebinds SOCKS;
//!   [shutdown] tears everything down.
//! - rustls only, no OpenSSL.
//! - No platform-specific code (no JNI, no android_logger): portable to Android, iOS, desktop.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::{Arc, Mutex, Once};

use arti_client::config::TorClientConfigBuilder;
use arti_client::TorClient;
use futures::StreamExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::runtime::Runtime;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tor_rtcompat::PreferredRuntime;

uniffi::setup_scaffolding!();

// ============================================================================
// tracing -> StatusListener.on_log forwarding
// ============================================================================

static LOG_SINK: Mutex<Option<Arc<dyn StatusListener>>> = Mutex::new(None);
static TRACING_INIT: Once = Once::new();

struct ForwardLayer;

struct MsgVisitor {
    msg: String,
    extra: String,
}

impl tracing::field::Visit for MsgVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.msg = format!("{value:?}");
        } else {
            self.extra
                .push_str(&format!(" {}={:?}", field.name(), value));
        }
    }
}

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for ForwardLayer {
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let guard = LOG_SINK.lock().unwrap();
        let Some(listener) = guard.as_ref() else {
            return;
        };
        let mut visitor = MsgVisitor {
            msg: String::new(),
            extra: String::new(),
        };
        event.record(&mut visitor);
        let meta = event.metadata();
        listener.on_log(format!(
            "{} [{}]{} {}",
            meta.level(),
            meta.target(),
            visitor.extra,
            visitor.msg
        ));
    }
}

fn init_tracing() {
    use tracing_subscriber::filter::LevelFilter;
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    use tracing_subscriber::Layer;
    TRACING_INIT.call_once(|| {
        let _ = tracing_subscriber::registry()
            .with(ForwardLayer.with_filter(LevelFilter::INFO))
            .try_init();
        std::panic::set_hook(Box::new(|info| {
            if let Some(listener) = LOG_SINK.lock().unwrap().as_ref() {
                listener.on_log(format!("PANIC: {info}"));
            }
        }));
    });
}

// ============================================================================
// Public FFI types
// ============================================================================

/// High-level lifecycle state, mirrored 1:1 into Kotlin `TorState`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum TorState {
    Off,
    Starting,
    Bootstrapping,
    Running,
    /// Bootstrapped client kept; SOCKS listener is down.
    Paused,
    Stopping,
    Error,
}

/// Caller-supplied configuration. Paths are provided by the caller.
///
/// `socks_port == 0` binds an ephemeral port; the actual port is reported via
/// [StatusListener::on_status].
#[derive(Debug, Clone, uniffi::Record)]
pub struct ArtiConfig {
    pub data_dir: String,
    pub socks_port: u16,
    #[uniffi(default = [])]
    pub bridges: Vec<String>,
    #[uniffi(default = None)]
    pub state_dir: Option<String>,
    #[uniffi(default = None)]
    pub cache_dir: Option<String>,
}

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum ArtiError {
    #[error("already running")]
    AlreadyRunning,
    #[error("not running")]
    NotRunning,
    #[error("configuration error: {msg}")]
    Config { msg: String },
    #[error("failed to bind SOCKS on port {port}: {msg}")]
    Bind { port: u16, msg: String },
    #[error("bootstrap failed: {msg}")]
    Bootstrap { msg: String },
    #[error("runtime error: {msg}")]
    Runtime { msg: String },
}

/// Typed error discriminant for asynchronous failure notification.
///
/// [`ArtiError`] itself is a UniFFI `Error` type and therefore cannot travel as a
/// callback argument; this mirror record carries the same information across the
/// [`StatusListener::on_error`] callback so the Kotlin layer can reconstruct the
/// declared public type *without* parsing status strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ErrorKind {
    AlreadyRunning,
    NotRunning,
    Config,
    Bind,
    Bootstrap,
    Runtime,
}

/// Typed asynchronous failure payload (see [`ErrorKind`]).
#[derive(Debug, Clone, uniffi::Record)]
pub struct ArtiErrorDetail {
    pub kind: ErrorKind,
    /// Only meaningful when `kind == Bind`; `None` otherwise.
    pub port: Option<u16>,
    pub msg: String,
}

impl From<&ArtiError> for ArtiErrorDetail {
    fn from(e: &ArtiError) -> Self {
        match e {
            ArtiError::AlreadyRunning => Self {
                kind: ErrorKind::AlreadyRunning,
                port: None,
                msg: "already running".into(),
            },
            ArtiError::NotRunning => Self {
                kind: ErrorKind::NotRunning,
                port: None,
                msg: "not running".into(),
            },
            ArtiError::Config { msg } => Self {
                kind: ErrorKind::Config,
                port: None,
                msg: msg.clone(),
            },
            ArtiError::Bind { port, msg } => Self {
                kind: ErrorKind::Bind,
                port: Some(*port),
                msg: msg.clone(),
            },
            ArtiError::Bootstrap { msg } => Self {
                kind: ErrorKind::Bootstrap,
                port: None,
                msg: msg.clone(),
            },
            ArtiError::Runtime { msg } => Self {
                kind: ErrorKind::Runtime,
                port: None,
                msg: msg.clone(),
            },
        }
    }
}

/// Status sink implemented on the Kotlin/Swift side. Invoked from worker
/// threads inside the owned runtime; implementations must be thread-safe.
///
/// Ordering guarantee: when an asynchronous operation fails, `on_error` is
/// invoked with the typed failure *before* the corresponding
/// `on_status(Error, ..)` report, on the same worker thread.
#[uniffi::export(callback_interface)]
pub trait StatusListener: Send + Sync {
    /// `bootstrap_percent` is 0..=100. `socks_port` is `Some` only once the
    /// local SOCKS listener is actually bound and accepting.
    /// `summary` is a short UI-oriented string (may be empty).
    fn on_status(
        &self,
        state: TorState,
        bootstrap_percent: u32,
        socks_port: Option<u16>,
        summary: String,
    );
    fn on_log(&self, line: String);
    /// Typed asynchronous failure. Always precedes the `Error` status report
    /// for the same failure; never parse `summary` strings to recover types.
    fn on_error(&self, error: ArtiErrorDetail);
}

// ============================================================================
// Shared handles between lifecycle methods and async tasks
// ============================================================================

struct Shared {
    client: Mutex<Option<Arc<TorClient<PreferredRuntime>>>>,
    listener: Mutex<Option<Arc<dyn StatusListener>>>,
    /// Actual bound SOCKS port (0 = not listening).
    bound_port: AtomicU16,
    bootstrap_done: AtomicBool,
    /// Signals the active SOCKS loop to exit (pause / shutdown).
    socks_shutdown: Mutex<Option<oneshot::Sender<()>>>,
    /// Per-connection SOCKS handlers. Aborted on pause/shutdown (fail-closed:
    /// Tor OFF means no traffic is retained; see `pause` docs).
    connections: Mutex<Vec<JoinHandle<()>>>,
}

impl Shared {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            client: Mutex::new(None),
            listener: Mutex::new(None),
            bound_port: AtomicU16::new(0),
            bootstrap_done: AtomicBool::new(false),
            socks_shutdown: Mutex::new(None),
            connections: Mutex::new(Vec::new()),
        })
    }

    fn set_listener(&self, listener: Arc<dyn StatusListener>) {
        *self.listener.lock().unwrap() = Some(listener.clone());
        *LOG_SINK.lock().unwrap() = Some(listener);
    }

    fn listener(&self) -> Option<Arc<dyn StatusListener>> {
        self.listener.lock().unwrap().clone()
    }

    fn report(&self, state: TorState, pct: u32, port: Option<u16>, summary: impl Into<String>) {
        match port {
            Some(p) => self.bound_port.store(p, Ordering::SeqCst),
            None if matches!(
                state,
                TorState::Paused | TorState::Off | TorState::Stopping | TorState::Error
            ) =>
            {
                self.bound_port.store(0, Ordering::SeqCst);
            }
            None => {}
        }
        if let Some(l) = self.listener() {
            l.on_status(state, pct, port, summary.into());
        }
    }

    fn client(&self) -> Option<Arc<TorClient<PreferredRuntime>>> {
        self.client.lock().unwrap().clone()
    }

    fn set_client(&self, c: Option<Arc<TorClient<PreferredRuntime>>>) {
        *self.client.lock().unwrap() = c;
    }

    fn install_socks_shutdown(&self) -> oneshot::Receiver<()> {
        let (tx, rx) = oneshot::channel();
        *self.socks_shutdown.lock().unwrap() = Some(tx);
        rx
    }

    fn signal_socks_shutdown(&self) {
        if let Some(tx) = self.socks_shutdown.lock().unwrap().take() {
            let _ = tx.send(());
        }
    }

    /// Track a per-connection SOCKS handler so pause/shutdown can terminate it.
    fn track_connection(&self, handle: JoinHandle<()>) {
        let mut conns = self.connections.lock().unwrap();
        // Opportunistic prune so the vec cannot grow without bound.
        conns.retain(|h| !h.is_finished());
        conns.push(handle);
    }

    /// Abort all live SOCKS connection handlers (fail-closed Tor OFF).
    fn abort_connections(&self) {
        let mut conns = self.connections.lock().unwrap();
        for h in conns.drain(..) {
            h.abort();
        }
    }

    /// Typed async failure notification: `on_error` (typed) first, then the
    /// `Error` status report and a log line. Callers must not encode the error
    /// type only into the summary string.
    fn notify_error(&self, error: &ArtiError, bootstrap_pct: u32) {
        if let Some(l) = self.listener() {
            l.on_error(ArtiErrorDetail::from(error));
        }
        self.report(TorState::Error, bootstrap_pct, None, format!("error: {error}"));
        if let Some(l) = self.listener() {
            l.on_log(format!("ERROR: {error}"));
        }
    }
}

// ============================================================================
// ArtiTor object
// ============================================================================

struct Inner {
    runtime: Option<Runtime>,
    /// In-flight cold start (bootstrap + SOCKS) or a resume SOCKS task.
    worker: Option<JoinHandle<()>>,
    last_config: Option<ArtiConfig>,
    shared: Arc<Shared>,
}

impl Default for Inner {
    fn default() -> Self {
        Self {
            runtime: None,
            worker: None,
            last_config: None,
            shared: Shared::new(),
        }
    }
}

#[derive(uniffi::Object)]
pub struct ArtiTor {
    inner: Mutex<Inner>,
}

#[uniffi::export]
impl ArtiTor {
    #[uniffi::constructor]
    pub fn new() -> Arc<Self> {
        // Process-global note: `LOG_SINK` routes tracing output to the most
        // recently installed listener. Only ONE ArtiTor instance per process is
        // supported; a second instance steals log routing (documented, not
        // silently multi-instance). Status/error callbacks remain per-instance.
        Arc::new(Self {
            inner: Mutex::new(Inner::default()),
        })
    }

    pub fn version(&self) -> String {
        format!(
            "arti-kmp-ffi {} (arti-client 0.46, rustls)",
            env!("CARGO_PKG_VERSION")
        )
    }

    /// True when a bootstrapped TorClient is held (RUNNING or PAUSED).
    pub fn has_client(&self) -> bool {
        let inner = self.inner.lock().unwrap();
        inner.shared.bootstrap_done.load(Ordering::SeqCst) && inner.shared.client().is_some()
    }

    /// True when SOCKS is bound and bootstrap is complete.
    pub fn is_ready(&self) -> bool {
        let inner = self.inner.lock().unwrap();
        inner.shared.bootstrap_done.load(Ordering::SeqCst)
            && inner.shared.bound_port.load(Ordering::SeqCst) != 0
    }

    /// Currently bound SOCKS port, if listening.
    pub fn socks_port(&self) -> Option<u16> {
        let p = self
            .inner
            .lock()
            .unwrap()
            .shared
            .bound_port
            .load(Ordering::SeqCst);
        if p == 0 {
            None
        } else {
            Some(p)
        }
    }

    /// Bootstrap (if needed) and bring up the local SOCKS proxy.
    ///
    /// Returns immediately; progress via `listener`.
    /// Readiness == `on_status(Running, 100, Some(port), _)`.
    /// Asynchronous failures are reported via `on_error` (typed) followed by
    /// `on_status(Error, ..)`.
    ///
    /// If a client is already bootstrapped and SOCKS is down (paused):
    /// - only `socks_port` changed → rebind SOCKS without re-bootstrapping;
    /// - TorClient-defining config (`data_dir`/`state_dir`/`cache_dir`/`bridges`,
    ///   including non-empty → empty) changed → the old client is torn down and
    ///   a new client is bootstrapped.
    /// If SOCKS is already up → [ArtiError::AlreadyRunning].
    pub fn start(
        &self,
        config: ArtiConfig,
        listener: Box<dyn StatusListener>,
    ) -> Result<(), ArtiError> {
        let mut inner = self.inner.lock().unwrap();
        let listener: Arc<dyn StatusListener> = Arc::from(listener);
        init_tracing();
        let _ = rustls::crypto::ring::default_provider().install_default();
        inner.shared.set_listener(listener);

        // SOCKS already up.
        if inner.shared.bound_port.load(Ordering::SeqCst) != 0 {
            return Err(ArtiError::AlreadyRunning);
        }
        // Worker still running (bootstrapping or SOCKS without port yet).
        if let Some(ref w) = inner.worker {
            if !w.is_finished() {
                return Err(ArtiError::AlreadyRunning);
            }
            inner.worker = None;
        }

        // Resume path: a bootstrapped client is held, only SOCKS is down.
        if inner.shared.bootstrap_done.load(Ordering::SeqCst) && inner.shared.client().is_some() {
            let needs_new_client = inner
                .last_config
                .as_ref()
                .is_some_and(|old| tor_client_config_changed(old, &config));
            if needs_new_client {
                // TorClient-defining config changed: discard the old client and
                // fall through to a cold bootstrap. Never keep a client built
                // with different bridges/dirs.
                inner.shared.set_client(None);
                inner.shared.bootstrap_done.store(false, Ordering::SeqCst);
                inner.shared.bound_port.store(0, Ordering::SeqCst);
                inner.shared.abort_connections();
            } else {
                // SOCKS-only change (at most the port): rebind, no bootstrap.
                inner.last_config = Some(config.clone());
                let port = config.socks_port;
                return spawn_socks(&mut inner, port);
            }
        }

        // Cold start (also used when the client-defining config changed).
        inner.last_config = Some(config.clone());
        spawn_cold(&mut inner, config)
    }

    /// Re-bind SOCKS using the last configuration. Requires a bootstrapped client.
    pub fn resume(&self, listener: Box<dyn StatusListener>) -> Result<(), ArtiError> {
        let mut inner = self.inner.lock().unwrap();
        let listener: Arc<dyn StatusListener> = Arc::from(listener);
        inner.shared.set_listener(listener);

        if inner.shared.bound_port.load(Ordering::SeqCst) != 0 {
            return Err(ArtiError::AlreadyRunning);
        }
        if let Some(ref w) = inner.worker {
            if !w.is_finished() {
                return Err(ArtiError::AlreadyRunning);
            }
            inner.worker = None;
        }
        if !inner.shared.bootstrap_done.load(Ordering::SeqCst) || inner.shared.client().is_none() {
            return Err(ArtiError::NotRunning);
        }
        let port = inner
            .last_config
            .as_ref()
            .map(|c| c.socks_port)
            .unwrap_or(0);
        spawn_socks(&mut inner, port)
    }

    /// Stop the SOCKS listener but keep the bootstrapped client and runtime.
    ///
    /// Deterministic pause-during-bootstrap semantics (see
    /// docs/api-lifecycle-bitchat.md §2.1): if no bootstrapped client is held
    /// yet (STARTING/BOOTSTRAPPING), the bootstrap worker is aborted, the
    /// partial/unbootstrapped client is discarded, all associated work
    /// (including SOCKS connection handlers) is stopped, and the state
    /// transitions to OFF — never left in BOOTSTRAPPING with no worker.
    /// All live SOCKS streams are terminated (fail-closed Tor OFF); new
    /// connections are no longer accepted.
    pub fn pause(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.shared.signal_socks_shutdown();
        inner.shared.abort_connections();
        if let Some(w) = inner.worker.take() {
            // If still bootstrapping, abort; if SOCKS was running, shutdown signal
            // should finish the task — abort as fallback.
            w.abort();
        }
        if inner.shared.bootstrap_done.load(Ordering::SeqCst) && inner.shared.client().is_some() {
            inner
                .shared
                .report(TorState::Paused, 100, None, "paused");
            if let Some(l) = inner.shared.listener() {
                l.on_log("SOCKS paused; TorClient retained".into());
            }
        } else {
            // No bootstrapped client: cancel the in-flight bootstrap (if any),
            // discard any partial client, and settle to OFF so no caller is left
            // observing STARTING/BOOTSTRAPPING after the worker was aborted.
            inner.shared.set_client(None);
            inner.shared.bootstrap_done.store(false, Ordering::SeqCst);
            inner.shared.bound_port.store(0, Ordering::SeqCst);
            inner
                .shared
                .report(TorState::Off, 0, None, "bootstrap cancelled");
            if let Some(l) = inner.shared.listener() {
                l.on_log("bootstrap cancelled by pause; client discarded".into());
            }
        }
    }

    /// Full teardown: SOCKS, client, and tokio runtime.
    ///
    /// Postcondition: OFF, no client, no SOCKS listener, no worker, runtime
    /// released. All live SOCKS streams are terminated. Idempotent.
    pub fn shutdown(&self) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(l) = inner.shared.listener() {
            l.on_status(TorState::Stopping, 0, None, "stopping".into());
        }
        inner.shared.signal_socks_shutdown();
        inner.shared.abort_connections();
        if let Some(w) = inner.worker.take() {
            w.abort();
        }
        inner.shared.set_client(None);
        inner.shared.bootstrap_done.store(false, Ordering::SeqCst);
        inner.shared.bound_port.store(0, Ordering::SeqCst);
        inner.last_config = None;
        if let Some(rt) = inner.runtime.take() {
            rt.shutdown_background();
        }
        let old_listener = inner.shared.listener();
        inner.shared = Shared::new();
        // Only release the process-global log sink if it still points at our
        // listener; a newer instance may have installed its own (single-
        // instance is supported, last-writer-wins for logs).
        if let Some(ours) = old_listener.clone() {
            let mut sink = LOG_SINK.lock().unwrap();
            if sink.as_ref().is_some_and(|s| Arc::ptr_eq(s, &ours)) {
                *sink = None;
            }
        }
        if let Some(l) = old_listener {
            l.on_status(TorState::Off, 0, None, String::new());
        }
    }

    /// Deprecated alias for [Self::shutdown] (0.1.x compatibility).
    pub fn stop(&self) {
        self.shutdown();
    }
}

fn spawn_socks(inner: &mut Inner, socks_port: u16) -> Result<(), ArtiError> {
    let runtime = inner.runtime.as_ref().ok_or_else(|| ArtiError::Runtime {
        msg: "no runtime".into(),
    })?;
    let client = inner.shared.client().ok_or(ArtiError::NotRunning)?;
    let shared = inner.shared.clone();
    let task = runtime.spawn(async move {
        if let Err(e) = run_socks(client, socks_port, shared.clone()).await {
            let pct = if shared.bootstrap_done.load(Ordering::SeqCst) {
                100
            } else {
                0
            };
            // Typed notification first; never rely on parsing the summary.
            shared.notify_error(&e, pct);
        }
    });
    inner.worker = Some(task);
    Ok(())
}

/// Spawn a cold bootstrap + SOCKS task on the owned runtime, creating the
/// runtime on first use. The task reports typed failures via `on_error`.
fn spawn_cold(inner: &mut Inner, config: ArtiConfig) -> Result<(), ArtiError> {
    let runtime = match inner.runtime.take() {
        Some(rt) => rt,
        None => tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(|e| ArtiError::Runtime { msg: e.to_string() })?,
    };

    let shared = inner.shared.clone();
    shared.bootstrap_done.store(false, Ordering::SeqCst);
    shared.set_client(None);
    shared.bound_port.store(0, Ordering::SeqCst);

    let task = runtime.spawn(async move {
        if let Err(e) = cold_start(config, shared.clone()).await {
            shared.notify_error(&e, 0);
        }
    });

    inner.runtime = Some(runtime);
    inner.worker = Some(task);
    Ok(())
}

/// TorClient-defining configuration: changing any of these requires a new
/// TorClient/bootstrap. `socks_port` is deliberately excluded (rebind only).
/// Bridge comparison is exact: non-empty → empty counts as a change.
fn tor_client_config_changed(old: &ArtiConfig, new: &ArtiConfig) -> bool {
    old.data_dir != new.data_dir
        || old.state_dir != new.state_dir
        || old.cache_dir != new.cache_dir
        || old.bridges != new.bridges
}

// ============================================================================
// Cold start + SOCKS
// ============================================================================

fn resolve_dirs(config: &ArtiConfig) -> (PathBuf, PathBuf) {
    let data = PathBuf::from(&config.data_dir);
    let state = config
        .state_dir
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or_else(|| data.join("state"));
    let cache = config
        .cache_dir
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or_else(|| data.join("cache"));
    (state, cache)
}

async fn cold_start(config: ArtiConfig, shared: Arc<Shared>) -> Result<(), ArtiError> {
    shared.report(TorState::Starting, 0, None, "starting");
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
    shared.set_client(Some(client.clone()));

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
                            shared.report(
                                TorState::Bootstrapping,
                                pct.min(99),
                                None,
                                format!("bootstrapping {pct}%"),
                            );
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
    shared.bootstrap_done.store(true, Ordering::SeqCst);
    if let Some(l) = shared.listener() {
        l.on_log("bootstrap complete".into());
    }

    run_socks(client, config.socks_port, shared).await
}

async fn run_socks(
    client: Arc<TorClient<PreferredRuntime>>,
    socks_port: u16,
    shared: Arc<Shared>,
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
            msg: format!("local_addr: {e}"),
        })?
        .port();

    if let Some(l) = shared.listener() {
        l.on_log(format!("SOCKS listening on 127.0.0.1:{actual_port}"));
    }
    shared.report(TorState::Running, 100, Some(actual_port), "proxy ready");

    let mut shutdown_rx = shared.install_socks_shutdown();

    loop {
        tokio::select! {
            _ = &mut shutdown_rx => {
                if let Some(l) = shared.listener() {
                    l.on_log("SOCKS shutdown signal".into());
                }
                shared.bound_port.store(0, Ordering::SeqCst);
                break;
            }
            accept = socks.accept() => {
                match accept {
                    Ok((stream, _peer)) => {
                        let client = client.clone();
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

/// Minimal SOCKS5 CONNECT handler tunnelling through the Tor client.
/// Ported from the proven bitchat android wrapper (CONNECT only).
async fn handle_socks(
    mut stream: tokio::net::TcpStream,
    client: Arc<TorClient<PreferredRuntime>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut buf = [0u8; 512];

    let n = stream.read(&mut buf).await?;
    if n < 2 {
        return Err("invalid SOCKS handshake".into());
    }
    stream.write_all(&[0x05, 0x00]).await?;

    let n = stream.read(&mut buf).await?;
    if n < 10 {
        return Err("invalid SOCKS request".into());
    }
    if buf[0] != 0x05 {
        return Err("unsupported SOCKS version".into());
    }
    if buf[1] != 0x01 {
        stream
            .write_all(&[0x05, 0x07, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
            .await?;
        return Err("unsupported SOCKS command".into());
    }

    let (host, port) = match buf[3] {
        0x01 => {
            let ip = format!("{}.{}.{}.{}", buf[4], buf[5], buf[6], buf[7]);
            (ip, u16::from_be_bytes([buf[8], buf[9]]))
        }
        0x03 => {
            let len = buf[4] as usize;
            if n < 5 + len + 2 {
                return Err("invalid domain length".into());
            }
            let domain = String::from_utf8_lossy(&buf[5..5 + len]).to_string();
            let port = u16::from_be_bytes([buf[5 + len], buf[5 + len + 1]]);
            (domain, port)
        }
        0x04 => {
            if n < 22 {
                stream
                    .write_all(&[0x05, 0x01, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
                    .await?;
                return Err("truncated IPv6".into());
            }
            let mut seg = [0u16; 8];
            for i in 0..8 {
                seg[i] = u16::from_be_bytes([buf[4 + i * 2], buf[5 + i * 2]]);
            }
            let ip = format!(
                "{:x}:{:x}:{:x}:{:x}:{:x}:{:x}:{:x}:{:x}",
                seg[0], seg[1], seg[2], seg[3], seg[4], seg[5], seg[6], seg[7]
            );
            (ip, u16::from_be_bytes([buf[20], buf[21]]))
        }
        _ => {
            stream
                .write_all(&[0x05, 0x08, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
                .await?;
            return Err("unsupported address type".into());
        }
    };

    let tor_stream = match client.connect((host.as_str(), port)).await {
        Ok(s) => s,
        Err(e) => {
            stream
                .write_all(&[0x05, 0x05, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
                .await?;
            return Err(e.into());
        }
    };
    stream
        .write_all(&[0x05, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
        .await?;

    let (mut cr, mut cw) = stream.split();
    let (mut tr, mut tw) = tor_stream.split();
    let c2t = async { tokio::io::copy(&mut cr, &mut tw).await };
    let t2c = async { tokio::io::copy(&mut tr, &mut cw).await };
    tokio::select! {
        _ = c2t => {}
        _ = t2c => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;
    use std::time::Duration;

    fn test_config() -> ArtiConfig {
        ArtiConfig {
            data_dir: "/tmp/artitor-test".into(),
            socks_port: 0,
            bridges: vec![],
            state_dir: None,
            cache_dir: None,
        }
    }

    #[test]
    fn identical_config_needs_no_new_client() {
        let a = test_config();
        assert!(!tor_client_config_changed(&a, &a));
    }

    #[test]
    fn socks_port_only_needs_no_new_client() {
        let a = test_config();
        let mut b = test_config();
        b.socks_port = 19050;
        assert!(!tor_client_config_changed(&a, &b));
    }

    #[test]
    fn data_dir_change_needs_new_client() {
        let a = test_config();
        let mut b = test_config();
        b.data_dir = "/tmp/other".into();
        assert!(tor_client_config_changed(&a, &b));
    }

    #[test]
    fn state_dir_override_change_needs_new_client() {
        let a = test_config();
        let mut b = test_config();
        b.state_dir = Some("/tmp/other-state".into());
        assert!(tor_client_config_changed(&a, &b));
    }

    #[test]
    fn cache_dir_override_change_needs_new_client() {
        let a = test_config();
        let mut b = test_config();
        b.cache_dir = Some("/tmp/other-cache".into());
        assert!(tor_client_config_changed(&a, &b));
    }

    #[test]
    fn bridges_empty_to_nonempty_needs_new_client() {
        let a = test_config();
        let mut b = test_config();
        b.bridges = vec!["obfs4 1.2.3.4:443 FINGERPRINT".into()];
        assert!(tor_client_config_changed(&a, &b));
    }

    #[test]
    fn bridges_nonempty_to_empty_needs_new_client() {
        // Regression: the old resume path silently kept the bootstrapped client
        // (built with bridges) when the new config cleared them.
        let mut a = test_config();
        a.bridges = vec!["obfs4 1.2.3.4:443 FINGERPRINT".into()];
        let b = test_config();
        assert!(tor_client_config_changed(&a, &b));
    }

    #[test]
    fn same_bridges_need_no_new_client() {
        let mut a = test_config();
        a.bridges = vec!["obfs4 1.2.3.4:443 FINGERPRINT".into()];
        let mut b = test_config();
        b.bridges = a.bridges.clone();
        assert!(!tor_client_config_changed(&a, &b));
    }

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
                ArtiError::Bootstrap { msg: "no consensus".into() },
                ErrorKind::Bootstrap,
                None,
            ),
            (
                ArtiError::Runtime { msg: "boom".into() },
                ErrorKind::Runtime,
                None,
            ),
        ];
        for (err, kind, port) in cases {
            let d = ArtiErrorDetail::from(&err);
            assert_eq!(d.kind, kind, "kind for {err}");
            assert_eq!(d.port, port, "port for {err}");
            assert!(!d.msg.is_empty(), "msg for {err}");
        }
    }

    #[test]
    fn resolve_dirs_defaults_and_overrides() {
        let c = test_config();
        let (s, ca) = resolve_dirs(&c);
        assert_eq!(s, PathBuf::from("/tmp/artitor-test/state"));
        assert_eq!(ca, PathBuf::from("/tmp/artitor-test/cache"));

        let mut c2 = test_config();
        c2.state_dir = Some("/s".into());
        c2.cache_dir = Some("/c".into());
        let (s2, c3) = resolve_dirs(&c2);
        assert_eq!(s2, PathBuf::from("/s"));
        assert_eq!(c3, PathBuf::from("/c"));
    }

    struct Recorder {
        statuses: StdMutex<Vec<(TorState, u32, Option<u16>, String)>>,
        errors: StdMutex<Vec<ArtiErrorDetail>>,
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

    fn test_client(name: &str) -> Arc<TorClient<PreferredRuntime>> {
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

    fn test_shared(rec: Arc<Recorder>) -> Arc<Shared> {
        let shared = Shared::new();
        shared.set_listener(rec);
        shared
    }

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
        let err = run_socks(test_client("bind-collision"), port, shared)
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
        let task = tokio::spawn(async move { run_socks(client, 0, shared).await });
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

    #[tokio::test]
    async fn notify_error_delivers_typed_detail_before_status() {
        let rec = Arc::new(Recorder {
            statuses: StdMutex::new(vec![]),
            errors: StdMutex::new(vec![]),
        });
        let shared = test_shared(rec.clone());
        shared.notify_error(
            &ArtiError::Bootstrap {
                msg: "no consensus".into(),
            },
            42,
        );
        let errors = rec.errors.lock().unwrap();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].kind, ErrorKind::Bootstrap);
        assert_eq!(errors[0].port, None);
        let statuses = rec.statuses.lock().unwrap();
        assert_eq!(statuses.len(), 1);
        assert_eq!(statuses[0].0, TorState::Error);
        assert_eq!(statuses[0].1, 42);
        assert_eq!(statuses[0].2, None);
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
}
