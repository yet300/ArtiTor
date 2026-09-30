//! Convert existing errors into typed callback payloads without parsing messages.
use crate::{ArtiError, ArtiErrorDetail, ErrorKind};

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
