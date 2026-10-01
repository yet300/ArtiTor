# Narrow reliability evidence

`reconnected-first.txt` is the first completed hardware diagnostic: 100 session-create greetings, 80 root greetings and 30 resumed-session greetings succeed, then the old root port accepts TCP after pause cycle 30. It stops at that failure. `confirmation.txt` is a separate run with richer failure capture; all its local probes and four fixed HTTPS requests pass. Neither replaces the other.

`*-process-logcat.txt` files retain only each consumer PID, beginning with the harmless pre-construction control marker. The synthetic bridge markers are absent from all three verified nonempty captures and from public exception/log-flow records. `evidence-summary.json` contains counts and redaction checks. Whole-device captures/APKs remain under `/tmp/artitor-android-reliability`, outside the repository.

`offline_pause_reproducer.rs` is a **diagnostic that passes when it observes the defect**. It must not become a passing product regression asserting the wrong contract. It was injected into a disposable source copy and exercises the existing private offline test fixture plus actual root SOCKS worker and native pause, without external Tor traffic. It holds the worker's synchronous RUNNING callback to force the cancellation/destruction interval. Ten iterations verify PAUSED/null native state while the old TCP port connects, then refusal after callback release. No CONNECT is sent.

Reproduce from the repository root, keeping authoritative Rust files untouched:

```sh
rtk proxy python3 - <<'PY'
from pathlib import Path
import shutil, tempfile
source = Path('rust/arti-kmp-ffi')
copy = Path(tempfile.mkdtemp(prefix='artitor-offline-pause-'))
for name in ('Cargo.toml', 'Cargo.lock'):
    shutil.copy2(source / name, copy / name)
shutil.copytree(source / 'src', copy / 'src')
evidence = Path('docs/audit/evidence/0.3/android-request-reliability-2026-10-01')
shutil.copy2(evidence / 'offline_pause_reproducer.rs', copy / 'src/tests/offline_pause_reproducer.rs')
module = copy / 'src/tests/mod.rs'
module.write_text(module.read_text() + '\nmod offline_pause_reproducer;\n')
print(copy / 'Cargo.toml')
PY
```

Use the manifest path printed above (loopback sockets require host permission in this environment):

```sh
rtk proxy cargo test --manifest-path /tmp/artitor-offline-pause-REPLACE/Cargo.toml \
  --target-dir /Users/yet/development/Multiplatform/ArtiTor/rust/arti-kmp-ffi/target \
  deterministically_reproduces_pause_tcp_teardown_gap -- --nocapture --test-threads=1
```

For the on-device diagnostic, build/install the existing minified Maven consumer as described in `tests/android-minified-consumer/README.md`, then capture Logcat before launch:

```sh
rtk proxy /Users/yet/Library/Android/sdk/platform-tools/adb shell am force-stop consumer.artitor03.hardwaregate
rtk proxy /Users/yet/Library/Android/sdk/platform-tools/adb logcat -c
rtk proxy /Users/yet/Library/Android/sdk/platform-tools/adb logcat -v threadtime > /tmp/artitor-local-diagnostic-logcat.txt
```

From another terminal, with a unique `run` value:

```sh
rtk proxy /Users/yet/Library/Android/sdk/platform-tools/adb shell am start \
  -n consumer.artitor03.hardwaregate/consumer.artitor03.app.MainActivity \
  --es run local-diagnostic-UNIQUE --es mode reliability
```

The program stops at its first local assertion failure. If its entire local phase passes, it makes exactly two HTTPS requests each to the existing check-service and ipify targets on one root endpoint, without retries. The output is `gate-local-diagnostic-UNIQUE.txt` in the app's external files directory. Extra failure instrumentation records PAUSED snapshots, recent timestamped public history and a greeting attempt on an unexpectedly accepted old TCP socket. It never turns a failed negative probe into a pass.

The installed/built APK and Maven AAR hashes/native-byte provenance are in `artifact-provenance.json`. The first incomplete old-consumer launch and interrupted diagnostic are preserved separately; no completed acceptance result is inferred from them. `tooling-first-failures.txt` records the preserved installer/sandbox failures, and the recovered instrumentation XML is kept separately when available.
