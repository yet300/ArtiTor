// Host smoke test for the FFI lifecycle: bootstrap, SOCKS, pause, resume.
// Run with:
//   cargo run --example host_poc
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::Arc;
use std::time::Duration;

use arti_kmp_ffi::{ArtiConfig, ArtiErrorDetail, ArtiTor, StatusListener, TorState};

struct Printer {
    port: Arc<AtomicU16>,
}

impl StatusListener for Printer {
    fn on_status(
        &self,
        state: TorState,
        bootstrap_percent: u32,
        socks_port: Option<u16>,
        summary: String,
    ) {
        if let Some(p) = socks_port {
            self.port.store(p, Ordering::SeqCst);
        }
        println!("STATUS {state:?} {bootstrap_percent}% port={socks_port:?} ({summary})");
    }
    fn on_log(&self, line: String) {
        println!("LOG {line}");
    }
    fn on_error(&self, error: ArtiErrorDetail) {
        println!(
            "ERROR {:?} port={:?} ({})",
            error.kind, error.port, error.msg
        );
    }
}

fn main() {
    let dir = std::env::temp_dir().join("arti-host-poc");
    let tor = ArtiTor::new();
    let port = Arc::new(AtomicU16::new(0));
    println!("version: {}", tor.version());

    tor.start(
        ArtiConfig {
            data_dir: dir.to_string_lossy().into_owned(),
            socks_port: 0, // ephemeral
            bridges: vec![],
            bridges_enabled: arti_kmp_ffi::BridgesEnabled::Auto,
            state_dir: None,
            cache_dir: None,
            allow_onion_addrs: true,
            connect_timeout_nanos: 10_000_000_000,
            resolve_timeout_nanos: 10_000_000_000,
        },
        Box::new(Printer { port: port.clone() }),
    )
    .expect("start failed");

    // Wait until ready (or timeout).
    let deadline = std::time::Instant::now() + Duration::from_secs(150);
    while std::time::Instant::now() < deadline {
        if tor.is_ready() {
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    println!(
        "ready={} has_client={} socks={:?}",
        tor.is_ready(),
        tor.has_client(),
        tor.socks_port()
    );

    if tor.is_ready() {
        println!("pause…");
        tor.pause();
        std::thread::sleep(Duration::from_secs(1));
        println!(
            "after pause: ready={} has_client={} socks={:?}",
            tor.is_ready(),
            tor.has_client(),
            tor.socks_port()
        );

        println!("resume…");
        tor.resume(Box::new(Printer { port: port.clone() }))
            .expect("resume failed");
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        while std::time::Instant::now() < deadline {
            if tor.is_ready() {
                break;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        println!(
            "after resume: ready={} socks={:?}",
            tor.is_ready(),
            tor.socks_port()
        );
    }

    tor.shutdown();
    println!("shutdown complete");
}
