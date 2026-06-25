//! Thin UniFFI surface over `arti-client` (Tor in Rust) for Kotlin Multiplatform.
//!
//! Design goals (see repo docs/adr):
//! - The async tokio runtime is owned *inside* this crate; callers never block their main thread.
//! - Bootstrap progress is a first-class signal sourced from `TorClient::bootstrap_events()`
//!   (`BootstrapStatus::as_frac()`), NOT scraped from log lines.
//! - rustls only, no OpenSSL.
//! - No platform-specific code (no JNI, no android_logger): portable to Android, iOS, desktop.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, Once};

use arti_client::config::TorClientConfigBuilder;
use arti_client::TorClient;
use futures::StreamExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::runtime::Runtime;
use tokio::task::JoinHandle;
use tor_rtcompat::PreferredRuntime;

uniffi::setup_scaffolding!();

// ============================================================================
// tracing -> StatusListener.on_log forwarding
// ============================================================================
// arti-client emits its diagnostics (incl. bootstrap progress) via the `tracing`
// crate. We install a process-wide subscriber once that forwards events to the
// currently-active listener, realizing the public `logs` Flow and making
// bootstrap behaviour observable.

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
        // Surface panics (otherwise they vanish to stderr, invisible on Android).
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
    Stopping,
    Error,
}

/// Caller-supplied configuration. `data_dir` is provided by the caller; the
/// library never reaches into platform-specific paths on its own.
#[derive(Debug, Clone, uniffi::Record)]
pub struct ArtiConfig {
    pub data_dir: String,
    pub socks_port: u16,
    #[uniffi(default = [])]
    pub bridges: Vec<String>,
}

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum ArtiError {
    #[error("already running")]
    AlreadyRunning,
    #[error("configuration error: {msg}")]
    Config { msg: String },
    #[error("runtime error: {msg}")]
    Runtime { msg: String },
}

/// Status sink implemented on the Kotlin/Swift side. Invoked from worker
/// threads inside the owned runtime; implementations must be thread-safe.
#[uniffi::export(callback_interface)]
pub trait StatusListener: Send + Sync {
    /// `bootstrap_percent` is 0..=100. `socks_port` is `Some` only once the
    /// local SOCKS listener is actually bound and accepting.
    fn on_status(&self, state: TorState, bootstrap_percent: u32, socks_port: Option<u16>);
    /// Optional debug log line.
    fn on_log(&self, line: String);
}

// ============================================================================
// ArtiTor object
// ============================================================================

#[derive(Default)]
struct Inner {
    runtime: Option<Runtime>,
    main_task: Option<JoinHandle<()>>,
    client: Option<Arc<TorClient<PreferredRuntime>>>,
}

#[derive(uniffi::Object)]
pub struct ArtiTor {
    inner: Mutex<Inner>,
}

#[uniffi::export]
impl ArtiTor {
    #[uniffi::constructor]
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(Inner::default()),
        })
    }

    /// Version string of this wrapper and the embedded Arti client.
    pub fn version(&self) -> String {
        format!(
            "arti-kmp-ffi {} (arti-client 0.43, rustls)",
            env!("CARGO_PKG_VERSION")
        )
    }

    /// Start bootstrapping and bring up the local SOCKS proxy. Returns
    /// immediately; progress and readiness are reported via `listener`.
    /// Readiness == `on_status(Running, 100, Some(port))`.
    pub fn start(
        &self,
        config: ArtiConfig,
        listener: Box<dyn StatusListener>,
    ) -> Result<(), ArtiError> {
        let mut inner = self.inner.lock().unwrap();
        if inner.main_task.is_some() {
            return Err(ArtiError::AlreadyRunning);
        }

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(|e| ArtiError::Runtime { msg: e.to_string() })?;

        let listener: Arc<dyn StatusListener> = Arc::from(listener);
        init_tracing();
        // rustls 0.23 requires a process-wide CryptoProvider; install ring once.
        let _ = rustls::crypto::ring::default_provider().install_default();
        *LOG_SINK.lock().unwrap() = Some(listener.clone());
        let shared = SharedClient::default();
        let shared_for_task = shared.clone();

        let task = runtime.spawn(async move {
            if let Err(e) = run(config, listener.clone(), shared_for_task).await {
                listener.on_log(format!("ERROR: {e}"));
                listener.on_status(TorState::Error, 0, None);
            }
        });

        inner.runtime = Some(runtime);
        inner.main_task = Some(task);
        inner.client = None; // populated lazily by the task via SharedClient
                             // Keep a handle so stop() can drop the client too.
        inner.client = shared.0.lock().unwrap().clone();
        Ok(())
    }

    /// Stop the proxy and tear down the runtime.
    pub fn stop(&self) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(task) = inner.main_task.take() {
            task.abort();
        }
        inner.client = None;
        if let Some(rt) = inner.runtime.take() {
            // Drop the runtime off-thread to avoid "drop runtime in async context" panics.
            rt.shutdown_background();
        }
    }
}

#[derive(Default, Clone)]
struct SharedClient(Arc<Mutex<Option<Arc<TorClient<PreferredRuntime>>>>>);

// ============================================================================
// Core async flow
// ============================================================================

async fn run(
    config: ArtiConfig,
    listener: Arc<dyn StatusListener>,
    shared: SharedClient,
) -> Result<(), ArtiError> {
    listener.on_status(TorState::Starting, 0, None);
    listener.on_log(format!(
        "starting arti: data_dir={}, socks_port={}",
        config.data_dir, config.socks_port
    ));

    let data_path = PathBuf::from(&config.data_dir);
    let state_dir = data_path.join("state");
    let cache_dir = data_path.join("cache");
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
    listener.on_log("config built; creating unbootstrapped client".to_string());

    // Unbootstrapped client so we can stream bootstrap progress ourselves.
    let client = TorClient::builder()
        .config(tor_config)
        .create_unbootstrapped()
        .map_err(|e| ArtiError::Runtime { msg: e.to_string() })?;
    *shared.0.lock().unwrap() = Some(client.clone());
    listener.on_log("client created; starting bootstrap".to_string());

    // Forward bootstrap progress (0.0..=1.0 -> 0..=100) until bootstrap finishes.
    let mut events = client.bootstrap_events();
    let progress_listener = listener.clone();
    let progress_task = tokio::spawn(async move {
        while let Some(status) = events.next().await {
            let pct = (status.as_frac() * 100.0).round() as u32;
            progress_listener.on_status(TorState::Bootstrapping, pct.min(99), None);
        }
    });

    let boot = client.bootstrap().await;
    progress_task.abort();
    listener.on_log(format!("bootstrap() returned: ok={}", boot.is_ok()));
    boot.map_err(|e| ArtiError::Runtime { msg: e.to_string() })?;
    listener.on_log("bootstrap complete".to_string());

    // Bring up the local SOCKS proxy and report readiness only once bound.
    let addr = format!("127.0.0.1:{}", config.socks_port);
    let socks = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|e| ArtiError::Runtime {
            msg: format!("failed to bind {addr}: {e}"),
        })?;
    listener.on_log(format!("SOCKS listening on {addr}"));
    listener.on_status(TorState::Running, 100, Some(config.socks_port));

    loop {
        match socks.accept().await {
            Ok((stream, _peer)) => {
                let client = client.clone();
                let listener = listener.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_socks(stream, client).await {
                        listener.on_log(format!("socks conn error: {e}"));
                    }
                });
            }
            Err(e) => {
                listener.on_log(format!("socks accept error: {e}"));
                break;
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
    // No-auth.
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
