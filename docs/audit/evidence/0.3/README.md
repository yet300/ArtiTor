# Integrated 0.3 evidence

These are implementation/sensitivity results, not final independent review acceptance.

- Native mutation JSON and patches record green baselines and intended assertion failures in disposable copies.
- CAS JSON records two green baselines, two mutant failures and restored green output.
- Privacy red/green logs use synthetic documentation addresses and secret markers, never real bridge material.
- Resource JSON is an offline host fixture; simulator text is a real bootstrapped foreground process with lifetime peak RSS.
- Release JSON records actual packaged ELF segments, pristine/posthook/published archive symbols, feature identity and like-profile bytes. Stale i686 build leftovers are excluded from the supported matrix.
- Consumer fixture uses only published coordinates and links separate iOS device and simulator frameworks. Strict linker probes set iOS 15.0 for passing device/simulator controls and iOS 14.0 for the expected-failure support-boundary checks. From repository root, publish with `rtk ./gradlew :tor:publishToMavenLocal -PreleaseBuild=true`; set the SDK location for your environment, then run the repository wrapper with `-p docs/audit/evidence/0.3/consumer` and the tasks in the integrated report. No project-source dependency or direct coroutines workaround is present.
- Consumer packaging JSON includes minified APK native/JNA names and actual linked simulator framework symbol count. Runtime on Android hardware remains pending.
- Final simulator attempt1 retains the103-test/1-failure result; a separate unchanged targeted live rerun passed. The earlier remote timeout is not claimed fixed.

Raw logs and XML from every attempt remain under `/tmp/artitor-integrated`; the integrated report preserves commands, outcomes and limitations.
