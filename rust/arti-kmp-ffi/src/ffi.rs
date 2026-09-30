// Stable crate-root UniFFI facade (included by lib.rs to preserve metadata checksums).
#[uniffi::export]
impl ArtiTor {
    #[uniffi::constructor]
    pub fn new() -> Arc<Self> {
        Self::ffi_new()
    }

    pub fn version(&self) -> String {
        self.ffi_version()
    }

    /// True when a bootstrapped TorClient is held (RUNNING or PAUSED).
    pub fn has_client(&self) -> bool {
        self.ffi_has_client()
    }

    /// True when SOCKS is bound and bootstrap is complete.
    pub fn is_ready(&self) -> bool {
        self.ffi_is_ready()
    }

    /// Currently bound SOCKS port, if listening.
    pub fn socks_port(&self) -> Option<u16> {
        self.ffi_socks_port()
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
        self.ffi_start(config, listener)
    }

    /// Re-bind SOCKS using the last configuration. Requires a bootstrapped client.
    ///
    /// Root is rebound first; then every live non-closed session is rebound
    /// on a fresh ephemeral port (`Paused → Active(new endpoint)`, published
    /// atomically). Ports MAY change across pause/resume — never assert
    /// equality. A session that fails to rebind stays `Paused` (null
    /// endpoint) with a diagnostic log while the engine stays `Running` if
    /// root bound; a root bind failure drives the engine to `Error` and
    /// freezes all sessions as `Paused`/null (never `Active` on an errored
    /// engine).
    pub fn resume(&self, listener: Box<dyn StatusListener>) -> Result<(), ArtiError> {
        self.ffi_resume(listener)
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
        self.ffi_pause()
    }

    /// Full teardown: SOCKS, client, and tokio runtime.
    ///
    /// Postcondition: OFF, no client, no SOCKS listener, no worker, runtime
    /// released. All live SOCKS streams are terminated. Idempotent.
    pub fn shutdown(&self) {
        self.ffi_shutdown()
    }

    /// Deprecated alias for [Self::shutdown] (0.1.x compatibility).
    pub fn stop(&self) {
        self.ffi_stop()
    }

    /// Create an additional circuit-isolated SOCKS session.
    ///
    /// - While `Running`: derives `root.isolated_client()`, binds an
    ///   ephemeral loopback listener synchronously, registers atomically,
    ///   publishes `Active(endpoint)`, and returns the non-owning handle.
    ///   Bind failure returns [`ArtiError::Bind`] with no leaked registry
    ///   entry, no retained isolated client, and the engine untouched.
    /// - While `Paused`: derives the isolated client and registers as
    ///   `Paused(endpoint=null)` without binding; the listener binds on the
    ///   next successful engine `resume()`.
    /// - While `Starting`/`Bootstrapping`/`Stopping`/`Error`/`Off`: rejected
    ///   with [`ArtiError::NotRunning`] (never queued).
    /// - Over the internal safety cap: [`ArtiError::Runtime`] with
    ///   `"session limit reached"`.
    ///
    /// Locking: engine → session ordering; no await/block while holding
    /// either lock (the loopback bind uses blocking `std::net`, the accept
    /// loop is spawned onto the runtime handle). A pause/shutdown racing the
    /// bind is detected at registration: the just-bound socket is dropped and
    /// the session registers as `Paused`, or everything is dropped with
    /// `NotRunning` after a generation change.
    pub fn create_session(
        &self,
        listener: Box<dyn SessionStatusListener>,
    ) -> Result<Arc<SocksSession>, ArtiError> {
        self.ffi_create_session(listener)
    }

    /// Snapshot of live (non-`Closed`, non-`Invalidated`) additional sessions.
    /// The root endpoint is never included.
    pub fn list_sessions(&self) -> Vec<SessionInfo> {
        self.ffi_list_sessions()
    }

    /// Close one session (`== SocksSession.close_session()`). No-op on
    /// unknown/closed/stale handles; never throws.
    pub fn close_session(&self, session: &SocksSession) {
        self.ffi_close_session(session)
    }

    /// Current snapshot for one handle (live, terminal-tombstone, or
    /// `Invalidated` for stale/unknown handles). Never throws.
    pub fn session_status(&self, session: &SocksSession) -> SessionStatusFfi {
        self.ffi_session_status(session)
    }
}
