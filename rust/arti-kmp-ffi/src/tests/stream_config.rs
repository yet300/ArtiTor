//! Stable address policy and precise stream timeout configuration.
use super::*;

#[test]
fn native_stream_config_matches_independent_upstream_builders() {
    use arti_client::config::TorClientConfigBuilder;
    for (allow, connect, resolve) in [
        (true, 10_000_000_000, 10_000_000_000),
        (false, 0, 1),
        (true, 12_345_678, i64::MAX),
    ] {
        let mut config = test_config();
        config.allow_onion_addrs = allow;
        config.connect_timeout_nanos = connect;
        config.resolve_timeout_nanos = resolve;
        let built = crate::config::build_tor_config(&config).unwrap();
        let (state, cache) = resolve_dirs(&config);
        let mut expected = TorClientConfigBuilder::from_directories(state, cache);
        expected.address_filter().allow_onion_addrs(allow);
        expected
            .stream_timeouts()
            .connect_timeout(Duration::from_nanos(connect as u64));
        expected
            .stream_timeouts()
            .resolve_timeout(Duration::from_nanos(resolve as u64));
        // The two nested settings are private and expose no AsRef/getters in
        // 0.46. Whole-config equality checks the actual built values.
        assert_eq!(built, expected.build().unwrap());
    }
}

#[test]
fn native_negative_stream_timeouts_are_config_errors() {
    for (connect, resolve) in [
        (-1, 0),
        (0, -1),
        (i64::MIN, 10_000_000_000),
        (10_000_000_000, i64::MIN),
    ] {
        let mut config = test_config();
        config.connect_timeout_nanos = connect;
        config.resolve_timeout_nanos = resolve;
        assert!(matches!(
            crate::validate_config(config),
            Err(ArtiError::Config { .. })
        ));
    }
}

#[test]
fn native_stream_settings_change_client_identity() {
    let original = test_config();
    let mut changed = original.clone();
    changed.allow_onion_addrs = !changed.allow_onion_addrs;
    assert!(tor_client_config_changed(&original, &changed));
    changed = original.clone();
    changed.connect_timeout_nanos += 1;
    assert!(tor_client_config_changed(&original, &changed));
    changed = original.clone();
    changed.resolve_timeout_nanos += 1;
    assert!(tor_client_config_changed(&original, &changed));
}

#[test]
fn native_stream_changes_invalidate_retained_session_before_rebuild() {
    let changes: [fn(&mut ArtiConfig); 3] = [
        |config| config.allow_onion_addrs = false,
        |config| config.connect_timeout_nanos += 1,
        |config| config.resolve_timeout_nanos += 1,
    ];
    for change in changes {
        let (engine, recorder) = running_engine("stream-config-replacement");
        let session = engine.create_session(Box::new(recorder.clone())).unwrap();
        engine.pause();
        let mut config = test_config();
        let epoch = {
            let mut inner = engine.inner.lock().unwrap();
            inner.last_config = Some(config.clone());
            inner.test_spawn_cold_failure = true;
            inner.shared.client_epoch.load(Ordering::SeqCst)
        };
        change(&mut config);
        assert!(matches!(
            engine.start(config, Box::new(ReentrantEngineListener::new(&engine))),
            Err(ArtiError::Runtime { .. })
        ));
        assert_eq!(session.status_snapshot().state, SessionState::Invalidated);
        assert_eq!(session.status_snapshot().port, None);
        assert_eq!(
            recorder.last_for(&session.id()),
            Some((SessionState::Invalidated, None))
        );
        let inner = engine.inner.lock().unwrap();
        assert!(inner.shared.client().is_none());
        assert!(inner.shared.client_epoch.load(Ordering::SeqCst) > epoch);
        assert!(!inner.shared.bootstrap_done.load(Ordering::SeqCst));
        drop(inner);
        engine.shutdown();
    }
}

#[tokio::test]
async fn actual_arti_onion_policy_and_malformed_target_fail_before_bootstrap() {
    use arti_client::HasKind;
    // Valid v3 address from pinned arti-client address.rs:658,810.
    let valid_onion = "eweiibe6tdjsdprb4px6rqrzzcsi22m4koia44kc5pcjr7nec2rlxyad.onion";
    for (allow_onion, target, expected) in [
        (
            false,
            valid_onion,
            arti_client::ErrorKind::ForbiddenStreamTarget,
        ),
        (
            true,
            "malformed.onion",
            arti_client::ErrorKind::InvalidStreamTarget,
        ),
    ] {
        let dir = std::env::temp_dir().join(format!(
            "artitor-onion-policy-{}-{allow_onion}",
            std::process::id()
        ));
        let mut config = test_config();
        config.data_dir = dir.to_string_lossy().into_owned();
        config.allow_onion_addrs = allow_onion;
        let native = crate::config::build_tor_config(&config).unwrap();
        let client = TorClient::builder()
            .config(native)
            .create_unbootstrapped()
            .unwrap();
        let error = client
            .connect((target, 443))
            .await
            .err()
            .expect("policy must reject before bootstrap");
        assert_eq!(error.kind(), expected);
        assert_eq!(
            crate::error::classify_upstream(&error),
            crate::TorErrorKind::TargetRejected
        );
    }
}

#[tokio::test]
async fn configured_stream_deadlines_expire_pending_runtime_future() {
    use tor_rtcompat::SleepProviderExt;
    let mut config = test_config();
    config.connect_timeout_nanos = 2_000_000;
    config.resolve_timeout_nanos = 4_000_000;
    let (connect, resolve) = crate::config::stream_timeouts(&config).unwrap();
    let runtime = PreferredRuntime::current().unwrap();
    // Same runtime timer primitive used by Arti. This exercises converted
    // deadlines against a black-hole future, not private BEGIN/circuit code.
    for deadline in [connect, resolve] {
        assert!(runtime
            .timeout(deadline, futures::future::pending::<()>())
            .await
            .is_err());
    }
}

#[test]
fn native_stream_defaults_match_unmodified_upstream_config() {
    let config = test_config();
    assert!(config.allow_onion_addrs);
    assert_eq!(config.connect_timeout_nanos, 10_000_000_000);
    assert_eq!(config.resolve_timeout_nanos, 10_000_000_000);
    let (state, cache) = resolve_dirs(&config);
    let expected = arti_client::config::TorClientConfigBuilder::from_directories(state, cache)
        .build()
        .unwrap();
    assert_eq!(crate::config::build_tor_config(&config).unwrap(), expected);
}
