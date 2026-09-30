//! Convert existing errors into typed callback payloads without parsing messages.
use crate::{ArtiError, ArtiErrorDetail, ErrorKind, TorErrorKind};

impl From<&ArtiError> for ArtiErrorDetail {
    fn from(e: &ArtiError) -> Self {
        match e {
            ArtiError::AlreadyRunning => Self {
                kind: ErrorKind::AlreadyRunning,
                error_kind: TorErrorKind::AlreadyRunning,
                port: None,
                msg: "already running".into(),
            },
            ArtiError::NotRunning => Self {
                kind: ErrorKind::NotRunning,
                error_kind: TorErrorKind::NotRunning,
                port: None,
                msg: "not running".into(),
            },
            ArtiError::Config { msg } => Self {
                kind: ErrorKind::Config,
                error_kind: TorErrorKind::Config,
                port: None,
                msg: msg.clone(),
            },
            ArtiError::Bind { port, msg } => Self {
                kind: ErrorKind::Bind,
                error_kind: TorErrorKind::Bind,
                port: Some(*port),
                msg: msg.clone(),
            },
            ArtiError::Bootstrap { msg, error_kind } => Self {
                kind: ErrorKind::Bootstrap,
                error_kind: *error_kind,
                port: None,
                msg: msg.clone(),
            },
            ArtiError::Runtime { msg, error_kind } => Self {
                kind: ErrorKind::Runtime,
                error_kind: *error_kind,
                port: None,
                msg: msg.clone(),
            },
        }
    }
}

/// Pinned to all 59 variants of tor-error 0.46.0/src/lib.rs:133–751.
/// Classification uses the upstream stable category, never its message.
pub(crate) fn classify_upstream(error: &impl arti_client::HasKind) -> TorErrorKind {
    classify_upstream_kind(error.kind())
}

pub(crate) fn classify_upstream_kind(kind: arti_client::ErrorKind) -> TorErrorKind {
    finish_classification(known_upstream_kind(kind))
}

fn finish_classification(known: Option<TorErrorKind>) -> TorErrorKind {
    known.unwrap_or(TorErrorKind::Unknown)
}

fn known_upstream_kind(kind: arti_client::ErrorKind) -> Option<TorErrorKind> {
    use arti_client::ErrorKind::*;
    match kind {
        TorAccessFailed | DirectoryExpired | TorProtocolViolation | LocalNetworkError
        | RelayIdMismatch | CircuitCollapse | TorNetworkTimeout | TorDirectoryError
        | RelayTooBusy | CircuitRefused | NoPath | TorDirectoryUnusable | ClockSkew
        | TorDocumentRejected => Some(TorErrorKind::Network),
        PersistentStateAccessFailed
        | LocalResourceAlreadyInUse
        | FsPermissions
        | PersistentStateCorrupted
        | CacheCorrupted
        | CacheAccessFailed
        | KeystoreCorrupted
        | KeystoreAccessFailed => Some(TorErrorKind::Storage),
        RemoteNetworkTimeout
        | RemoteStreamClosed
        | RemoteStreamReset
        | RemoteStreamError
        | RemoteConnectionRefused
        | ExitPolicyRejected
        | ExitTimeout
        | RemoteNetworkFailed
        | RemoteHostNotFound
        | OnionServiceNotFound
        | OnionServiceNotRunning
        | OnionServiceProtocolViolation
        | OnionServiceConnectionFailed
        | RemoteHostResolutionFailed
        | RemoteProtocolViolation
        | NoExit => Some(TorErrorKind::ExitFailed),
        OnionServiceMissingClientAuth
        | OnionServiceWrongClientAuth
        | OnionServiceAddressInvalid
        | InvalidStreamTarget
        | ForbiddenStreamTarget => Some(TorErrorKind::TargetRejected),
        InvalidConfig | InvalidConfigTransition | NoHomeDirectory => Some(TorErrorKind::Config),
        BootstrapRequired => Some(TorErrorKind::BootstrapRequired),
        ReactorShuttingDown
        | ArtiShuttingDown
        | SoftwareDeprecated
        | NotImplemented
        | FeatureDisabled
        | LocalProtocolViolation
        | LocalResourceExhausted
        | ExternalToolFailed
        | TransientFailure
        | BadApiUsage
        | Internal => Some(TorErrorKind::Runtime),
        Other => Some(TorErrorKind::Unknown),
        _ => None,
    }
}

impl ArtiError {
    pub(crate) fn client_creation(error: &impl arti_client::HasKind) -> Self {
        Self::Runtime {
            msg: "failed to create Tor client".into(),
            error_kind: classify_upstream(error),
        }
    }

    pub(crate) fn bootstrap_failure(error: &impl arti_client::HasKind) -> Self {
        Self::Bootstrap {
            msg: "Tor bootstrap failed".into(),
            error_kind: classify_upstream(error),
        }
    }
}

#[cfg(test)]
mod classification_tests {
    use super::*;

    struct FakeError(arti_client::ErrorKind, String);
    impl arti_client::HasKind for FakeError {
        fn kind(&self) -> arti_client::ErrorKind {
            self.0
        }
    }
    impl std::fmt::Display for FakeError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(&self.1)
        }
    }

    #[test]
    fn synthetic_future_kind_uses_same_unknown_fallback() {
        // Rust cannot construct a future non-exhaustive enum variant. The
        // wildcard yields None, then follows this exact fallback path.
        assert_eq!(finish_classification(None), TorErrorKind::Unknown);
    }

    #[test]
    fn upstream_classification_ignores_diagnostic_and_redacts_transport() {
        let secret = "bridge=203.0.113.42 key=PRIVATE_KEY target=secret.example path=/private/state BOOTSTRAP TIMEOUT NETWORK";
        for kind in [
            arti_client::ErrorKind::FsPermissions,
            arti_client::ErrorKind::InvalidStreamTarget,
        ] {
            let error = FakeError(kind, secret.into());
            let expected = classify_upstream_kind(kind);
            assert_eq!(classify_upstream(&error), expected);
            for native in [
                ArtiError::client_creation(&error),
                ArtiError::bootstrap_failure(&error),
            ] {
                let detail = ArtiErrorDetail::from(&native);
                assert_eq!(detail.error_kind, expected);
                for diagnostic in [native.to_string(), format!("{native:?}"), detail.msg] {
                    for sensitive in [
                        "203.0.113.42",
                        "PRIVATE_KEY",
                        "secret.example",
                        "/private/state",
                    ] {
                        assert!(!diagnostic.contains(sensitive));
                    }
                }
            }
        }
    }

    #[test]
    fn session_taxonomy_transports_without_new_native_error_classes() {
        // Public close/snapshot are infallible. There is no fallible stale
        // session operation in 0.3; verify the internal transport only.
        for error_kind in [
            TorErrorKind::SessionClosed,
            TorErrorKind::SessionInvalidated,
        ] {
            let native = ArtiError::Runtime {
                msg: "session unavailable".into(),
                error_kind,
            };
            let detail = ArtiErrorDetail::from(&native);
            assert_eq!(detail.kind, ErrorKind::Runtime);
            assert_eq!(detail.error_kind, error_kind);
        }
    }
}

#[cfg(test)]
mod pinned_table_tests {
    use super::*;
    #[test]
    fn all_59_pinned_046_upstream_categories() {
        use arti_client::ErrorKind as Upstream;
        let cases = [
            (Upstream::TorAccessFailed, TorErrorKind::Network),
            (Upstream::DirectoryExpired, TorErrorKind::Network),
            (Upstream::TorProtocolViolation, TorErrorKind::Network),
            (Upstream::LocalNetworkError, TorErrorKind::Network),
            (Upstream::RelayIdMismatch, TorErrorKind::Network),
            (Upstream::CircuitCollapse, TorErrorKind::Network),
            (Upstream::TorNetworkTimeout, TorErrorKind::Network),
            (Upstream::TorDirectoryError, TorErrorKind::Network),
            (Upstream::RelayTooBusy, TorErrorKind::Network),
            (Upstream::CircuitRefused, TorErrorKind::Network),
            (Upstream::NoPath, TorErrorKind::Network),
            (Upstream::TorDirectoryUnusable, TorErrorKind::Network),
            (Upstream::ClockSkew, TorErrorKind::Network),
            (Upstream::TorDocumentRejected, TorErrorKind::Network),
            (Upstream::PersistentStateAccessFailed, TorErrorKind::Storage),
            (Upstream::LocalResourceAlreadyInUse, TorErrorKind::Storage),
            (Upstream::FsPermissions, TorErrorKind::Storage),
            (Upstream::PersistentStateCorrupted, TorErrorKind::Storage),
            (Upstream::CacheCorrupted, TorErrorKind::Storage),
            (Upstream::CacheAccessFailed, TorErrorKind::Storage),
            (Upstream::KeystoreCorrupted, TorErrorKind::Storage),
            (Upstream::KeystoreAccessFailed, TorErrorKind::Storage),
            (Upstream::RemoteNetworkTimeout, TorErrorKind::ExitFailed),
            (Upstream::RemoteStreamClosed, TorErrorKind::ExitFailed),
            (Upstream::RemoteStreamReset, TorErrorKind::ExitFailed),
            (Upstream::RemoteStreamError, TorErrorKind::ExitFailed),
            (Upstream::RemoteConnectionRefused, TorErrorKind::ExitFailed),
            (Upstream::ExitPolicyRejected, TorErrorKind::ExitFailed),
            (Upstream::ExitTimeout, TorErrorKind::ExitFailed),
            (Upstream::RemoteNetworkFailed, TorErrorKind::ExitFailed),
            (Upstream::RemoteHostNotFound, TorErrorKind::ExitFailed),
            (Upstream::OnionServiceNotFound, TorErrorKind::ExitFailed),
            (Upstream::OnionServiceNotRunning, TorErrorKind::ExitFailed),
            (
                Upstream::OnionServiceProtocolViolation,
                TorErrorKind::ExitFailed,
            ),
            (
                Upstream::OnionServiceConnectionFailed,
                TorErrorKind::ExitFailed,
            ),
            (
                Upstream::RemoteHostResolutionFailed,
                TorErrorKind::ExitFailed,
            ),
            (Upstream::RemoteProtocolViolation, TorErrorKind::ExitFailed),
            (Upstream::NoExit, TorErrorKind::ExitFailed),
            (
                Upstream::OnionServiceMissingClientAuth,
                TorErrorKind::TargetRejected,
            ),
            (
                Upstream::OnionServiceWrongClientAuth,
                TorErrorKind::TargetRejected,
            ),
            (
                Upstream::OnionServiceAddressInvalid,
                TorErrorKind::TargetRejected,
            ),
            (Upstream::InvalidStreamTarget, TorErrorKind::TargetRejected),
            (
                Upstream::ForbiddenStreamTarget,
                TorErrorKind::TargetRejected,
            ),
            (Upstream::InvalidConfig, TorErrorKind::Config),
            (Upstream::InvalidConfigTransition, TorErrorKind::Config),
            (Upstream::NoHomeDirectory, TorErrorKind::Config),
            (Upstream::BootstrapRequired, TorErrorKind::BootstrapRequired),
            (Upstream::ReactorShuttingDown, TorErrorKind::Runtime),
            (Upstream::ArtiShuttingDown, TorErrorKind::Runtime),
            (Upstream::SoftwareDeprecated, TorErrorKind::Runtime),
            (Upstream::NotImplemented, TorErrorKind::Runtime),
            (Upstream::FeatureDisabled, TorErrorKind::Runtime),
            (Upstream::LocalProtocolViolation, TorErrorKind::Runtime),
            (Upstream::LocalResourceExhausted, TorErrorKind::Runtime),
            (Upstream::ExternalToolFailed, TorErrorKind::Runtime),
            (Upstream::TransientFailure, TorErrorKind::Runtime),
            (Upstream::BadApiUsage, TorErrorKind::Runtime),
            (Upstream::Internal, TorErrorKind::Runtime),
            (Upstream::Other, TorErrorKind::Unknown),
        ];
        assert_eq!(cases.len(), 59);
        for (upstream, expected) in cases {
            assert_eq!(classify_upstream_kind(upstream), expected, "{upstream:?}");
        }
    }
}
