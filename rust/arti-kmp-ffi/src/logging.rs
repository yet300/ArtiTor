//! Process-global tracing and panic forwarding; callbacks occur after sink unlock.
use crate::StatusListener;
use std::sync::{Arc, Mutex, Once};

pub(super) static LOG_SINK: Mutex<Option<Arc<dyn StatusListener>>> = Mutex::new(None);
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
        let listener = LOG_SINK.lock().unwrap().clone();
        let Some(listener) = listener else {
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
                listener.on_log(format!("PANIC: {info}"));
            }
        }));
    });
}
