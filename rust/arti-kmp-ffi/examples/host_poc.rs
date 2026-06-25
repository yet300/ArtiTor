// Host smoke test for the FFI flow: bootstrap over the real Tor network and
// bring up the local SOCKS proxy. Run with:
//   cargo run --example host_poc
use arti_kmp_ffi::{ArtiConfig, ArtiTor, StatusListener, TorState};

struct Printer;

impl StatusListener for Printer {
    fn on_status(&self, state: TorState, bootstrap_percent: u32, socks_port: Option<u16>) {
        println!("STATUS {state:?} {bootstrap_percent}% port={socks_port:?}");
    }
    fn on_log(&self, line: String) {
        println!("LOG {line}");
    }
}

fn main() {
    let dir = std::env::temp_dir().join("arti-host-poc");
    let tor = ArtiTor::new();
    println!("version: {}", tor.version());
    tor.start(
        ArtiConfig {
            data_dir: dir.to_string_lossy().into_owned(),
            socks_port: 19050,
            bridges: vec![],
        },
        Box::new(Printer),
    )
    .expect("start failed");

    // Wait up to ~150s for bootstrap.
    std::thread::sleep(std::time::Duration::from_secs(150));
    println!("done waiting");
}
