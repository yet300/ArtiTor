# Synchronous listener teardown evidence

Baseline e75336411a7eaadda4a2c828d668cd373dc6b0e1. See the parent remediation
report for verdict, path inspection, ownership/lock proof and commit identities.

`native-red.txt` is the five real desired-invariant failures before production
changes. `native-green-corrected-fixture.txt` contains ten callback-held root
TCP refusals plus the other four initial regressions. `root-exit-red.txt` proves
the additional normal task-exit cleanup regression was observed before its guard.
Final reviewed native corpora: `rust-parallel-reviewed-complete.txt`, `rust-serial-reviewed.txt`.
`root-log-held-red.txt` preserves the independent review's prepublication
fixed-port counterexample before its correction. Both held-log and direct
log-callback reentry pause/shutdown are now permanent regressions.

Permanent tests live in `rust/arti-kmp-ffi/src/tests/teardown.rs`. Run:

```sh
rtk proxy cargo test --manifest-path rust/arti-kmp-ffi/Cargo.toml engine::tests::teardown -- --nocapture --test-threads=1
rtk proxy cargo fmt --check --manifest-path rust/arti-kmp-ffi/Cargo.toml
rtk proxy cargo test --manifest-path rust/arti-kmp-ffi/Cargo.toml
rtk proxy cargo test --manifest-path rust/arti-kmp-ffi/Cargo.toml -- --test-threads=1
```

Real host loopback socket permission is required. The tests use offline retained
unbootstrapped clients, not remote Tor targets. The callback latch has explicit
release, a 10-second callback watchdog and 5-second method/test bounds. Bounds
protect tests from deadlock; none is a production teardown mechanism.

Mutation: copy the crate's Cargo.toml, Cargo.lock and src into a disposable
directory; replace only `self.0.lock().unwrap().take();` in ListeningSocket.close
with an empty body. Retain all shutdown/abort signalling. Run the same teardown
tests with an **independent target directory**. `mutation-reviewed-isolated.txt`
records the final reviewed-source mutation: eight physical teardown/normal-exit
and log-schedule regressions fail, one reentry control passes, 97 unrelated tests
filtered. Earlier six-failure and shared-target
mutation and resulting cache collision are retained separately; authoritative
source was never mutated. A fresh authoritative rebuild then passed.

Cross-platform first and final commands/outputs are preserved, with separate
iOS XML directories (103/103 each). Android initializationError is separate from
the corrected 100-cycle local test and its actual diagnostic-tag Logcat. All
local probes require TCP ECONNREFUSED, including after pause and session close.

Maven-only consumer source/config was copied into a fresh temporary directory,
then built against the newly published io.github.yet300:tor:0.3.0. Build output,
FieldOrder verifier JSON, artifact hash chain, and finite hardware evidence are
preserved. APKs, full R8 output and whole-device logs stay outside the checkout.
The short smoke retains 60 process/tag-scoped first/last lines; full observed
marker scan counts are in JSON. Original first-process privacy closure stands.

Checksums for durable files are listed in SHA256SUMS.json. Reports preserve
preparation failures and first live outcomes instead of replacing them.

## Final reviewed-source hardware/artifact evidence

`cross-platform-reviewed.txt` and `ios-reviewed/`: required release bindings,
iOS arm64 compile, Android assembly and live iOS suite; 103/103 PASS.
`android-local-reviewed.txt`, `android-local-reviewed/` and
`android-local-reviewed-logcat.txt`: 100 Pixel cycles, 200 paused old TCP
refusals, 100 closed-session refusals, two shutdown refusals, zero accepts.

`maven-local-publish-reviewed.txt`, `minified-consumer-reviewed-build.txt` and
`jna-metadata-reviewed.json`: final publication, fresh Maven-only R8 consumer,
17 surviving FieldOrder structures verified. `artifact-provenance.json` is
final; `artifact-provenance-intermediate.json` preserves the earlier build.
`minified-local-reviewed.txt` and its summary: first final-artifact local run,
100 cycles / 200 TCP refusals / zero failures. `minified-integrated-reviewed-first.txt`
and its summary: first final integrated run, 86 assertions and 36 immediate
ECONNREFUSED probes, PASS. `redaction-smoke-reviewed.json` and the 60-line
excerpt preserve the short final local process smoke; all synthetic marker
counts zero. Earlier artifact evidence is historical, not substituted for these
final-source gates. `connected-reviewed-first.txt` and `connected-reviewed-first/`
preserve the single final-source connected suite outcome.
