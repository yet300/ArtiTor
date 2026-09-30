# ArtiTor Rust modularization report

Date: 2026-09-30 (Asia/Tbilisi). Confidence: **high** in behavior/API
preservation for the inspected bodies and executed regression schedules;
Android hardware runtime remains **unknown / NOT VERIFIED**.

## Baseline and evidence boundaries

Accepted production baseline: `86da3c8dc71025278df66d588126cc0bef475b49`.
Initial HEAD matched exactly. Initial working tree contained only:

- Modified `docs/audit/ARTITOR_0_3_PHASE1_INDEPENDENT_AUDIT.md`: final independent
  acceptance/re-audit history, classified as Phase-1 acceptance evidence.
- Untracked `docs/audit/ARTITOR_0_3_CAPABILITY_AUDIT.md`: historical capability
  research with freeze correction addendum, classified separately.

No pending production changes existed. Neither document was mixed with
the structural commit. Earlier REMEDIATION REQUIRED verdicts were preserved.
Closure was appended: Phase 1 CLOSED, accepted production SHA above,
independent result PASS WITH FOLLOW-UPS. Capability proposals were retained
as historical research and explicitly subordinated to the current freeze.

Closure commits:

- `5c506b93a5381bb570c39373118d63e7733204db` — docs(audit): record final Phase 1 acceptance
- `ad83254cf6bbaae502c2a9ceb17caca58c8f2f17` — docs(audit): add ArtiTor 0.3 capability audit

The working tree was clean after those commits. Local logs, generated bindings,
XML and comparison artifacts were kept under `/private/tmp/artitor-modularization`
and build directories; none are committed or promised as permanent evidence.
The durable facts, commands, counts and hashes are recorded here.

## Responsibility map and final structure

Before: `src/lib.rs` (4,615 lines: production, test hooks and 2,269-line
internal test block) plus `src/socks.rs` (277 lines, including its own 7 tests).
The complete files and Cargo manifest were read before moving implementation.
The pre-move responsibility map was recorded in the architecture note.

After, as documented in `docs/design/RUST_MODULE_STRUCTURE.md`:

```text
src/
  lib.rs, ffi.rs, config.rs, error.rs, logging.rs, socks.rs
  engine/
    mod.rs, lifecycle.rs, bootstrap.rs, root_socks.rs, dispatch.rs, test_hooks.rs
    session/mod.rs, session/admission.rs, session/listener.rs
  tests/
    mod.rs, support.rs, config.rs, lifecycle.rs, sessions.rs, races.rs,
    publication.rs, errors.rs, ownership.rs, socks.rs
```

- Root: stable public types/callbacks/object definitions and lightweight
  Weak session accessors. `ffi.rs` is the obvious engine export facade.
- Engine state: Shared/Inner own runtime, worker, root client, registry and
  tasks. Generation, client_epoch, worker revision, engine publication
  revision and per-session status revision remain independent fences.
- Engine lifecycle: start/cold replacement/pause/resume/shutdown and
  ERROR callback freshness. Bootstrap progress and root accept workers
  have coherent separate boundaries.
- Session: isolated-client ownership, demotion/invalidation/close/rebind;
  admission performs generation/client_epoch/cap/lifecycle transaction
  checks. Listener validates generation/state/revision before dispatch.
- Protocol remains untouched; shared dispatch selects no identity itself
  and receives the client selected by the root or session listener.
- Test fixtures are reusable offline harnesses; race, publication, error
  and ownership regressions have separate searchable files. All test-only
  scheduling seams are retained, behind cfg(test).

Sessions are a child of engine because engine strongly owns them. Workers
consult parent-private state; orchestrators call registry transitions.
No generic models/utils module, abstract framework, dependency additions,
Kotlin refactor or Phase-2 implementation was introduced.

`ffi.rs` is included by lib.rs at crate-root macro scope. Engine includes
`lifecycle.rs` and `session/admission.rs` at engine scope so narrow facade
entry points can use authoritative private fields. Source boundaries remain
coherent even where lexical module scope is intentionally preserved.

## LOC

Before LOC by Rust source file: lib.rs 4,615; socks.rs 277 (total 4,892).
After LOC by Rust source file:

| File beneath src | Lines |
|---|---:|
| `config.rs` | 28 |
| `engine/bootstrap.rs` | 144 |
| `engine/dispatch.rs` | 34 |
| `engine/lifecycle.rs` | 455 |
| `engine/mod.rs` | 324 |
| `engine/root_socks.rs` | 152 |
| `engine/session/admission.rs` | 316 |
| `engine/session/listener.rs` | 167 |
| `engine/session/mod.rs` | 351 |
| `engine/test_hooks.rs` | 96 |
| `error.rs` | 39 |
| `ffi.rs` | 135 |
| `lib.rs` | 239 |
| `logging.rs` | 68 |
| `socks.rs` | 277 |
| `tests/config.rs` | 82 |
| `tests/errors.rs` | 335 |
| `tests/lifecycle.rs` | 456 |
| `tests/mod.rs` | 14 |
| `tests/ownership.rs` | 186 |
| `tests/publication.rs` | 212 |
| `tests/races.rs` | 222 |
| `tests/sessions.rs` | 296 |
| `tests/socks.rs` | 258 |
| `tests/support.rs` | 243 |

Largest production file: `engine/lifecycle.rs`, 455 lines. Largest native
test file: `tests/lifecycle.rs`, 456 lines. No source module exceeds 1,000
lines. `lib.rs` is 239 lines; exported facade is 135 lines. The previously
mixed test block is now organized by behavior.

The example `examples/host_poc.rs` also has three rustfmt-only layout changes
(println wrapping, Printer literal, resume call indentation). A check of
its exact accepted-baseline contents independently confirmed existing
formatting drift. This is necessary for the requested crate-wide fmt check;
only layout and formatter trailing commas changed; behavior is unchanged.

## Visibility audit

No new unrestricted `pub` implementation surface or explicit `pub(crate)`
items were added. Existing protocol pub(crate) items remain byte-identical.
`pub(super)` at a direct crate child is effectively crate-visible; these
intentional additions are explicitly included in this audit:

- Config helpers `resolve_dirs` / `tor_client_config_changed`, and logging
  `init_tracing` / LOG_SINK: needed by engine workers/lifecycle.
- Shared / Inner type names: needed by private root object fields. Their
  authoritative fields remain private to engine and its descendants.
- Shared::session_snapshot_for and the engine close_session_handle adapter:
  needed by root Weak session accessors.
- Fourteen `ArtiTor::ffi_*` entry points: callable by the root export facade;
  they retain original implementation bodies. They are pub(super) at engine
  scope, never exported through UniFFI and never public outside the crate.

Engine-only pub(super) additions: spawn_cold, root spawn/run worker/ready
helpers and RootBindReady, common handle_socks, session cap/id helpers,
SessionRuntime plus its fields/snapshot/abort_all, pending notifications
plus payload fields, demote/invalidate/close/rebind helpers. They support
existing ownership across engine descendants, without crate-visible state.
Session listener module and run_session_listener/session_listener_failed
are bounded to crate::engine (pub(super)/pub(in crate::engine)).

Test-only additions are engine-scoped probe/gate types, their externally
used fields and scheduling methods, plus tests-scoped fixture types, fields,
constructors/accessors and gate installers. TestSignal's internal value/
condvar and fixture-only fields/methods remain private. No hook is public.

## Public API / FFI comparison

Ubique uses `generateFromLibrary()`, extracting proc-macro metadata from
the compiled library; scaffolding is at crate root. Baseline bindings were
captured after the normal Gradle build, then regenerated after extraction.

An intermediate extraction moved exported impls into child modules. It
preserved signatures and native function names, but changed 14 UniFFI
constructor/method checksums and declaration ordering. This was rejected
before committing. Keeping the original macro expansion at root with direct
facade delegates restored **byte-identical** generated files on all four
targets (including ordering, documentation, FFI signatures and checksums).

Final baseline/after binding SHA-256 values:

| Generated file | SHA-256 (before = after) |
|---|---|
| commonMain/arti_kmp_ffi.common.kt | `5a1d2fbde0d9cc709a3e144733f5c7df14deb85c62a494ec15d52ed68a7db39f` |
| androidMain/arti_kmp_ffi.android.kt | `6d1a18427ffca91126680e70941ef55446a82b0c7b74b99f28df32186b10f5b0` |
| jvmMain/arti_kmp_ffi.jvm.kt | `6d1a18427ffca91126680e70941ef55446a82b0c7b74b99f28df32186b10f5b0` |
| nativeMain/arti_kmp_ffi.native.kt | `3e0d9fa22ce04af5efd2e89b9680c84f0f36004ad4dbffc67a091bb507c521d2` |

Final textual binding differences: **NONE**. All commonMain Kotlin sources
are byte-identical. Generated package remains `com.yet.tor.ffi`; public
package remains `com.yet.tor`. Object/function/record/enum/callback sets and
UniFFI checksum contract are unchanged; generated types do not enter KMP
public API. `:tor:tasks --all` was inspected: no configured standalone
ABI/API validation task exists; none was invented.

## Mechanical-move and dependency audit

A token-level comparison of all function bodies from original lib.rs against
the extracted tree (ignoring comments/whitespace, preserving literals and
punctuation, accounting for ffi_* internal definition names) found all
**171/171 original function-body instances unchanged**. Added bodies are
14 direct facade calls plus one close-handle adapter. All 75 original native
test names/assertions remain, with only module-qualified names changing.
This comparison complements review and executed tests; it is not a claim
of exhaustive concurrency correctness beyond Phase-1 acceptance.

Reviewed staged stat/summary/color-moved diff and whitespace check. No
intentional algorithm, atomic ordering, guard scope/drop, callback clone/
delivery, channel/barrier, abort timing, cap, ID, revision arithmetic, error
message or configuration comparison change. No behavior bug was fixed.

Cargo.toml, Cargo.lock and `cargo tree -e features` output are byte-identical
before/after. No feature/dependency/build/toolchain/version/ABI/target or
release change. `rust/hide-sqlite3-symbols.sh` and `tor/build.gradle.kts`
(including Apple mitigation hook) are byte-identical. All platform builds
continue executing the existing symbol-localization hook.

## Verification and all attempts

All shell commands used the instructed `rtk` prefix. Log redirections below
are omitted from command spelling; temporary raw outputs were retained.

| Command | Result |
|---|---|
| git rev-parse HEAD; git status --short; git log --oneline -15 | Exact baseline; only two classified docs |
| cargo test --manifest-path rust/arti-kmp-ffi/Cargo.toml (initial sandbox) | FAILED: 33 pass / 42 fail; loopback bind Operation not permitted |
| same baseline command outside sandbox | PASS: 75/0; doc tests 0/0 |
| ./gradlew :tor:tasks --all (initial sandbox) | FAILED: Gradle cache .lck Operation not permitted |
| same task discovery outside sandbox | PASS; binding tasks present, no API/ABI validation task configured |
| cargo check / check --tests --manifest-path rust/arti-kmp-ffi/Cargo.toml | Passed at pure-types, state, test extraction, session, worker, grouped-test and facade checkpoints after repairs noted below; final production check PASS without warnings |
| cargo fmt --check --manifest-path rust/arti-kmp-ffi/Cargo.toml | Final PASS |
| rustfmt --edition 2021 --check rust/arti-kmp-ffi/src/ffi.rs rust/arti-kmp-ffi/src/engine/lifecycle.rs rust/arti-kmp-ffi/src/engine/session/admission.rs | Final PASS; additionally checks included source units |
| cargo test --manifest-path rust/arti-kmp-ffi/Cargo.toml | Final PASS: 75/0; doc tests 0/0 |
| cargo test --manifest-path rust/arti-kmp-ffi/Cargo.toml -- --test-threads=1 | Final PASS: 75/0; doc tests 0/0 |
| cargo tree --manifest-path rust/arti-kmp-ffi/Cargo.toml -e features | PASS; before/after exact match |
| ./gradlew :tor:buildBindings | PASS; final bindings exact match |
| ./gradlew :tor:iosSimulatorArm64Test :tor:compileKotlinIosArm64 :tor:assembleAndroidDeviceTest | Baseline PASS (2m18s), intermediate PASS (2m48s), final PASS (2m30s) |
| adb devices -l (initial sandbox) | FAILED: daemon socket Operation not permitted |
| adb devices -l outside sandbox | PASS: empty attached-device list |
| git diff --check / staged diff check | PASS |

Extraction failures retained, then repaired mechanically: listener failure
re-export was initially too private; test fixture visibility script initially
prefixed parameter/trait-method lines; root facade initially lacked Arc
import. Formatter reported that parse error, and later three included-file
formatting differences; all corrected. Intermediate unused Arc/MAX_SESSIONS
imports were removed/scoped. A first mechanical audit script accidentally
excluded tests/socks.rs as well as production socks.rs, reporting nine
missing functions; correcting its path filter yielded 171/171 equality.
An initial example token audit flagged rustfmt removal of trailing commas;
normalizing trailing commas before closing delimiters confirmed equality.
No failing regression was discarded, weakened, or retried into an unreported
PASS. The baseline host-example fmt probe deliberately failed on existing
drift, confirming the reason for its formatting-only change.

### Live simulator attempts

| Attempt | All simulator tests | Live tests | Live elapsed | Result |
|---|---:|---:|---:|---|
| Baseline | 76 | 2 | 96.055s | 0 failures/errors/skips |
| After extraction, before facade checksum repair | 76 | 2 | 109.087s | 0 failures/errors/skips |
| Final facade-preserving tree | 76 | 2 | 87.604s | 0 failures/errors/skips |

Each was a single normal build/test attempt; the second after-refactor run
verified the necessary checksum repair, not a retry of a failing live test.
Each total comprises 30 session, 24 lifecycle, 8 config, 6 invariants,
6 error mapping and 2 live tests. No CONNECT code5 failure was observed
in these attempts. Its historical intermittent cause remains unknown.
Existing Gradle deprecation/commonTest-host warnings and iOS test CStructVar
deprecation remain unchanged, intentionally unfixed.

**Android hardware runtime: NOT VERIFIED.** No device attached. Android test
assembly passed but is not runtime evidence.

## Before/after evidence

| Area | Before | After |
|---|---|---|
| Native tests | 75/0 outside sandbox | 75/0 parallel; 75/0 serial |
| iOS simulator tests | 76/0, including 2 live | 76/0, including 2 live |
| Generated/public API | Accepted baseline bindings/Kotlin sources | Byte-identical |
| Cargo features | Captured accepted feature graph | Byte-identical |
| lib.rs LOC | 4,615 | 239 |
| Largest Rust production file | lib.rs 4,615 (mixed; pre-test portion 2,346) | engine/lifecycle.rs 455 |
| Largest Rust test container | 2,269-line block in lib.rs | tests/lifecycle.rs 456 |

## Intentionally-unfixed follow-ups and final self-audit

No new behavior defect was discovered. The checksum effect was an extraction
incompatibility and was repaired structurally before commit. Preserved
non-blocking acceptance follow-ups: Android hardware verification; Phase-5
FD/memory/task measurement and cap reassessment; N03 session revision
exhaustion horizon; intermittent SOCKS CONNECT code5 reliability investigation.
No bridge implementation or Phase-2 feature entered production.

| Question | Answer |
|---|---|
| Any public Kotlin API change? | NO |
| Any UniFFI exported API / checksum contract change? | NO |
| Any Cargo feature/dependency change? | NO |
| Any lifecycle transition change? | NO |
| Any intentional lock-order change? | NO |
| Any intentional callback-order change? | NO |
| Any TorClient/session ownership-rule change? | NO |
| Any Phase-2 feature in production? | NO |
| Any Apple SQLite mitigation change? | NO |

## Final commit identities

- Phase-1 closure: `5c506b93a5381bb570c39373118d63e7733204db`.
- Historical capability audit: `ad83254cf6bbaae502c2a9ceb17caca58c8f2f17`.
- Verified structural refactor: `54eeeca15a8eaebe16c3966c1126924360645ed5`
  (`refactor(rust): modularize arti-kmp-ffi`).

This report is committed separately after the structural commit so it can
record that immutable SHA. The report-only commit's own identity is
discoverable with:

```text
git log -1 --format=%H -- docs/audit/ARTITOR_RUST_MODULARIZATION_REPORT.md
```

The structural commit contains only Rust extraction/test organization,
minimal formatting/module documentation and the architecture note. No
generated bindings, XML, logs, caches, IDE files or temporary probes were
staged. The only remaining file after the structural commit was this
report; after its documentation commit the working tree is clean.

PHASE 1 CLOSURE COMMITTED

RUST MODULARIZATION COMPLETE

Phase 2 started: NO

Behavioral changes: NONE

Public API changes: NONE
