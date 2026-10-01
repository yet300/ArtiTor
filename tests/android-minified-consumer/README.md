# Minified Android Maven consumer release regression

This standalone app consumes only `io.github.yet300:tor:0.3.0` for ArtiTor.
It uses transitive coroutine dependencies, the optimized Android default R8
configuration and the published library consumer rules. It has no project
dependency, native override or application keep-rule file.

From the repository root (JDK 21, Android SDK, Python 3):

```sh
./gradlew :tor:publishToMavenLocal -PreleaseBuild=true --no-configuration-cache
./gradlew -p tests/android-minified-consumer :app:verifyJnaMetadata --refresh-dependencies --no-configuration-cache
```

Set `ANDROID_HOME`, or create an untracked `local.properties` in this fixture
containing `sdk.dir`. For a source/config-only copy in a fresh directory, pass
`-PjnaMetadataVerifier=/absolute/path/to/scripts/verify_android_jna_metadata.py`.
The task validates runtime FieldOrder annotations and reflected field names
against all surviving annotated structures discovered from original resolved
ArtiTor/UniFFI AARs. RustBufferStruct and UniffiRustCallStatusStruct are mandatory.
It examines all APK DEX files through SDK dexdump and R8 mapping. Unused structs
may shrink. Results are in `app/build/reports/jna-metadata.json`; CI runs this
before hardware testing.

On hardware install `app/build/outputs/apk/release/app-release.apk`, then launch
`consumer.artitor03.hardwaregate/consumer.artitor03.app.MainActivity` with a
unique string extra `run`. Force-stop an old instance before an intentional new
run; package replacement can automatically recreate the foreground activity.
The app writes `gate-<run>.txt` in its external files directory and logs with tag
`ArtiGate`. Preserve the first runtime result before any diagnostic repeat.

The first operations are public client construction and native version. The
full flow then covers bootstrap, root and A/B SOCKS traffic, close isolation,
bounded StateFlow session activation after resume, public onion/policy rejection,
synthetic invalid configuration preservation/redaction, stale handles, restart,
capacity/resources and a safe local occupied-port async BIND callback. SOCKS
targets are unresolved domain names sent to Tor; no direct DNS/traffic fallback
exists. The raw SOCKS HTTP fixture requires no Android cleartext-policy opt-in.
No real bridge is used. Timing and memory samples are observations, not promises.

This executable app does not treat DEX metadata presence as proof of runtime
callbacks. A release decision also requires a completed hardware log ending in
`GATE_COMPLETED,PASS`, inspection for secret markers, and the remaining release
checks documented in the hardware gate report.

For the finite listener/request diagnostic, add `--es mode reliability` to the
launch. It exercises root RUNNING and session ACTIVE greeting barriers, old-port
refusal after pause, 100 retained-session resume cycles and three cold starts.
Local phases send only SOCKS greetings. The program stops at the first local
failure; only a completed local phase proceeds to four fixed HTTPS requests
with explicit SOCKS/TLS/HTTP stage evidence. The pre-construction control marker
supports nonempty, process-scoped Logcat redaction verification. Preserve each
uniquely named run, including failed and interrupted attempts. This mode is a
release investigation tool and does not change the normal integrated flow.

Use `--es mode reliability-local` for the same finite local phases with no
external CONNECT matrix. Integrated teardown probes now require ECONNREFUSED
immediately after synchronous close/pause/shutdown; no teardown delay is used.
