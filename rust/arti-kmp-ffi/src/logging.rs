//! Process-global tracing and panic forwarding; callbacks occur after sink unlock.
use crate::StatusListener;
use std::sync::{Arc, Mutex, Once};

pub(super) static LOG_SINK: Mutex<Option<Arc<dyn StatusListener>>> = Mutex::new(None);
static TRACING_INIT: Once = Once::new();

struct ForwardLayer;

/// Only compiler-defined module metadata crosses FFI. Upstream event payloads
/// and custom targets can contain bridge identities, paths or target names.
fn forward_event(event: &tracing::Event<'_>, listener: &dyn StatusListener) {
    let metadata = event.metadata();
    listener.on_log(format!(
        "{} [{}]",
        metadata.level(),
        metadata.module_path().unwrap_or("upstream")
    ));
}

fn panic_diagnostic(_payload: &(dyn std::any::Any + Send)) -> &'static str {
    "PANIC: native operation failed"
}

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for ForwardLayer {
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let listener = LOG_SINK.lock().unwrap().clone();
        if let Some(listener) = listener {
            forward_event(event, listener.as_ref());
        }
    }
}

pub(super) fn init_tracing() {
    use tracing_subscriber::filter::LevelFilter;
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    use tracing_subscriber::Layer;
    TRACING_INIT.call_once(|| {
        let _ = tracing_subscriber::registry()
            .with(ForwardLayer.with_filter(LevelFilter::INFO))
            .try_init();
        std::panic::set_hook(Box::new(|info| {
            let listener = LOG_SINK.lock().unwrap().clone();
            if let Some(listener) = listener {
                listener.on_log(panic_diagnostic(info.payload()).into());
            }
        }));
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ArtiErrorDetail, TorState};
    use tracing_subscriber::{layer::SubscriberExt, Layer};

    #[derive(Clone)]
    struct Recorder(Arc<Mutex<Vec<String>>>);
    impl StatusListener for Recorder {
        fn on_status(&self, _: TorState, _: u32, _: Option<u16>, _: String) {}
        fn on_error(&self, _: ArtiErrorDetail) {}
        fn on_log(&self, line: String) {
            self.0.lock().unwrap().push(line);
        }
    }

    // The isolated subscriber calls the exact production forwarding function
    // without modifying the process-global sink or global tracing subscriber.
    struct IsolatedForwardLayer(Recorder);
    impl<S: tracing::Subscriber> Layer<S> for IsolatedForwardLayer {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _: tracing_subscriber::layer::Context<'_, S>,
        ) {
            forward_event(event, &self.0);
        }
    }

    #[test]
    fn production_forwarding_omits_adversarial_event_payloads_and_custom_target() {
        let recorder = Recorder(Arc::new(Mutex::new(vec![])));
        let subscriber = tracing_subscriber::registry().with(
            IsolatedForwardLayer(recorder.clone())
                .with_filter(tracing_subscriber::filter::LevelFilter::INFO),
        );
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(target: "PRIVATE_CUSTOM_TARGET.example", bridge="203.0.113.42:443", fingerprint="SECRET_FINGERPRINT", host="SECRET_HOST.example", key="SECRET_AUTH_KEY", path="/private/SECRET_STATE", isolation_token="SECRET_ISOLATION_TOKEN", "SECRET_MESSAGE");
            tracing::warn!(guard=?"SECRET_GUARD_FINGERPRINT", "SECRET_WARN_MESSAGE");
            tracing::debug!(key = "SECRET_DEBUG_KEY", "SECRET_DEBUG_MESSAGE");
        });
        let lines = recorder.0.lock().unwrap();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], format!("INFO [{}]", module_path!()));
        assert_eq!(lines[1], format!("WARN [{}]", module_path!()));
        for secret in [
            "203.0.113.42",
            "SECRET_",
            "PRIVATE_CUSTOM_TARGET",
            "/private/",
        ] {
            assert!(lines.iter().all(|line| !line.contains(secret)));
        }
    }

    #[test]
    fn panic_payload_is_never_forwarded() {
        let secret =
            String::from("bridge SECRET_FINGERPRINT target.example /private/state SECRET_KEY");
        assert_eq!(panic_diagnostic(&secret), "PANIC: native operation failed");
        assert_eq!(panic_diagnostic(&42_u64), "PANIC: native operation failed");
    }
}
