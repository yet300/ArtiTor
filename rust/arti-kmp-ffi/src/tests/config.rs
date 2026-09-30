//! Exact config identity and directory resolution.
use super::*;

#[test]
fn identical_config_needs_no_new_client() {
    let a = test_config();
    assert!(!tor_client_config_changed(&a, &a));
}

#[test]
fn socks_port_only_needs_no_new_client() {
    let a = test_config();
    let mut b = test_config();
    b.socks_port = 19050;
    assert!(!tor_client_config_changed(&a, &b));
}

#[test]
fn data_dir_change_needs_new_client() {
    let a = test_config();
    let mut b = test_config();
    b.data_dir = "/tmp/other".into();
    assert!(tor_client_config_changed(&a, &b));
}

#[test]
fn state_dir_override_change_needs_new_client() {
    let a = test_config();
    let mut b = test_config();
    b.state_dir = Some("/tmp/other-state".into());
    assert!(tor_client_config_changed(&a, &b));
}

#[test]
fn cache_dir_override_change_needs_new_client() {
    let a = test_config();
    let mut b = test_config();
    b.cache_dir = Some("/tmp/other-cache".into());
    assert!(tor_client_config_changed(&a, &b));
}

#[test]
fn bridges_empty_to_nonempty_needs_new_client() {
    let a = test_config();
    let mut b = test_config();
    b.bridges = vec!["obfs4 1.2.3.4:443 FINGERPRINT".into()];
    assert!(tor_client_config_changed(&a, &b));
}

#[test]
fn bridges_nonempty_to_empty_needs_new_client() {
    // Regression: the old resume path silently kept the bootstrapped client
    // (built with bridges) when the new config cleared them.
    let mut a = test_config();
    a.bridges = vec!["obfs4 1.2.3.4:443 FINGERPRINT".into()];
    let b = test_config();
    assert!(tor_client_config_changed(&a, &b));
}

#[test]
fn same_bridges_need_no_new_client() {
    let mut a = test_config();
    a.bridges = vec!["obfs4 1.2.3.4:443 FINGERPRINT".into()];
    let mut b = test_config();
    b.bridges = a.bridges.clone();
    assert!(!tor_client_config_changed(&a, &b));
}

#[test]
fn resolve_dirs_defaults_and_overrides() {
    let c = test_config();
    let (s, ca) = resolve_dirs(&c);
    assert_eq!(s, PathBuf::from("/tmp/artitor-test/state"));
    assert_eq!(ca, PathBuf::from("/tmp/artitor-test/cache"));

    let mut c2 = test_config();
    c2.state_dir = Some("/s".into());
    c2.cache_dir = Some("/c".into());
    let (s2, c3) = resolve_dirs(&c2);
    assert_eq!(s2, PathBuf::from("/s"));
    assert_eq!(c3, PathBuf::from("/c"));
}

#[test]
fn malformed_bridge_is_rejected_before_worker_creation() {
    let engine = ArtiTor::new();
    let listener = ReentrantEngineListener::new(&engine);
    let mut config = test_config();
    config.bridges = vec!["203.0.113.44:49123 SECRET_BRIDGE_FINGERPRINT_MARKER".into()];
    let result = engine.start(config, Box::new(listener.clone()));
    // Cleanup first so a regression never leaves a worker running.
    let (worker_started, runtime_created, client_created) = {
        let inner = engine.inner.lock().unwrap();
        (
            inner.worker.is_some(),
            inner.runtime.is_some(),
            inner.shared.client().is_some(),
        )
    };
    engine.shutdown();
    assert!(matches!(result, Err(ArtiError::Config { .. })));
    assert!(!worker_started && !runtime_created && !client_created);
    assert!(listener
        .logs
        .lock()
        .unwrap()
        .iter()
        .all(|line| !line.contains("SECRET_BRIDGE")));
}

// Syntax derived from tor-guardmgr 0.46 config.rs parse tests, with
// documentation addresses and deterministic synthetic RSA fingerprints.
const DIRECT_A: &str = "192.0.2.10:443 $1111111111111111111111111111111111111111";
const DIRECT_B: &str = "Bridge [2001:db8::42]:123 $2222222222222222222222222222222222222222";
const PT: &str = "obfs4 203.0.113.44:49123 $1111111111111111111111111111111111111111 password=SECRET_TRANSPORT_OPTION_MARKER";

#[test]
fn bridge_enablement_matrix() {
    use crate::config::build_tor_config;
    use arti_client::config::{BoolOrAuto, BridgesConfig, BridgesConfigBuilder};
    use BridgesEnabled::*;
    for (mode, lines, enabled, applied) in [
        (Auto, vec![], false, 0),
        (Auto, vec![DIRECT_A], true, 1),
        (Auto, vec![DIRECT_A, DIRECT_B], true, 2),
        (On, vec![DIRECT_A], true, 1),
        (Off, vec![], false, 0),
        (Off, vec![DIRECT_A], false, 0),
        (Off, vec![DIRECT_A, DIRECT_B], false, 0),
    ] {
        let mut config = test_config();
        config.bridges_enabled = mode;
        config.bridges = lines.into_iter().map(String::from).collect();
        let built = build_tor_config(&config).unwrap();
        let bridges: &BridgesConfig = built.as_ref();
        let mut expected = BridgesConfigBuilder::default();
        expected.enabled(match mode {
            Auto => BoolOrAuto::Auto,
            On => BoolOrAuto::Explicit(true),
            Off => BoolOrAuto::Explicit(false),
        });
        if enabled {
            for line in &config.bridges {
                expected.bridges().push(line.parse().unwrap());
            }
        }
        assert_eq!(expected.bridges().len(), applied);
        // BridgesConfig has no public getters in 0.46; equality includes
        // both the actual enablement and the complete applied bridge list.
        assert_eq!(bridges, &expected.build().unwrap());
    }
}

#[test]
fn bridge_invalid_matrix_is_secret_safe_and_starts_no_worker() {
    use crate::config::build_tor_config;
    use BridgesEnabled::*;
    let direct_secret = "203.0.113.44:49123 SECRET_BRIDGE_FINGERPRINT_MARKER";
    for mode in [Auto, On, Off] {
        let mut cases = vec![
            vec![],
            vec!["garbage"],
            vec![""],
            vec![" "],
            vec!["\t"],
            vec!["\n"],
            vec![direct_secret],
            vec![PT],
            vec![DIRECT_A, "garbage"],
            vec!["garbage", DIRECT_A, DIRECT_B],
            vec![DIRECT_A, "garbage", DIRECT_B],
            vec![DIRECT_A, DIRECT_B, "garbage"],
        ];
        if mode != On {
            cases.remove(0);
        }
        for lines in cases {
            let engine = ArtiTor::new();
            let listener = ReentrantEngineListener::new(&engine);
            let mut config = test_config();
            config.bridges_enabled = mode;
            config.bridges = lines.into_iter().map(String::from).collect();
            let internal = build_tor_config(&config)
                .err()
                .expect("invalid bridge configuration must fail");
            let result = engine.start(config, Box::new(listener.clone()));
            let inner = engine.inner.lock().unwrap();
            assert!(inner.worker.is_none() && inner.runtime.is_none());
            assert!(inner.shared.client().is_none());
            assert_eq!(inner.shared.worker_revision.load(Ordering::SeqCst), 1);
            drop(inner);
            let error = result.unwrap_err();
            assert!(matches!(error, ArtiError::Config { .. }));
            let detail = ArtiErrorDetail::from(&error);
            assert_eq!(detail.kind, ErrorKind::Config);
            let diagnostics = format!(
                "{error} {error:?} {internal} {internal:?} {} {:?}",
                detail.msg,
                listener.logs.lock().unwrap()
            );
            for secret in [
                "203.0.113.44",
                "49123",
                "SECRET_BRIDGE_FINGERPRINT_MARKER",
                "SECRET_TRANSPORT_OPTION_MARKER",
                "$1111111111111111111111111111111111111111",
            ] {
                assert!(!diagnostics.contains(secret), "bridge material escaped");
            }
            engine.shutdown();
        }
    }
}

#[test]
fn pt_shape_is_rejected_by_pinned_parser_without_support() {
    use arti_client::config::{BridgeConfigBuilder, BridgeParseError};
    assert!(matches!(
        PT.parse::<BridgeConfigBuilder>(),
        Err(BridgeParseError::PluggableTransportsNotSupported { .. })
    ));
}

#[test]
fn bridge_identity_is_exact_in_all_modes() {
    use BridgesEnabled::*;
    for mode in [Auto, On, Off] {
        let mut a = test_config();
        a.bridges_enabled = mode;
        a.bridges = vec![DIRECT_A.into(), DIRECT_B.into()];
        assert!(!tor_client_config_changed(&a, &a.clone()));
        for other in [Auto, On, Off] {
            let mut b = a.clone();
            b.bridges_enabled = other;
            assert_eq!(tor_client_config_changed(&a, &b), mode != other);
        }
        for lines in [
            vec![DIRECT_B.into(), DIRECT_A.into()],
            vec![format!(" {DIRECT_A}"), DIRECT_B.into()],
            vec![DIRECT_B.into()],
        ] {
            let mut b = a.clone();
            b.bridges = lines;
            assert!(tor_client_config_changed(&a, &b));
        }
    }
}

#[test]
fn bridge_changes_use_existing_session_replacement_path() {
    use BridgesEnabled::*;
    for (old_mode, new_mode, old_lines, new_lines) in [
        (Auto, On, vec![DIRECT_A], vec![DIRECT_A]),
        (On, Off, vec![DIRECT_A], vec![DIRECT_A]),
        (Off, Off, vec![DIRECT_A], vec![DIRECT_B]),
        (Auto, Auto, vec![DIRECT_A], vec![DIRECT_B]),
    ] {
        let (engine, rec) = running_engine("bridge-replacement");
        let session = engine.create_session(Box::new(rec.clone())).unwrap();
        engine.pause();
        let mut old = test_config();
        old.bridges_enabled = old_mode;
        old.bridges = old_lines.into_iter().map(String::from).collect();
        let mut new = old.clone();
        new.bridges_enabled = new_mode;
        new.bridges = new_lines.into_iter().map(String::from).collect();
        let before = {
            let mut inner = engine.inner.lock().unwrap();
            inner.last_config = Some(old);
            // Existing seam stops immediately on entry to the cold path;
            // this proves replacement without starting network work.
            inner.test_spawn_cold_failure = true;
            inner.shared.client_epoch.load(Ordering::SeqCst)
        };
        let listener = ReentrantEngineListener::new(&engine);
        assert!(matches!(
            engine.start(new, Box::new(listener)),
            Err(ArtiError::Runtime { .. })
        ));
        assert_eq!(session.status_snapshot().state, SessionState::Invalidated);
        assert_eq!(session.status_snapshot().port, None);
        assert_eq!(
            rec.last_for(&session.id()),
            Some((SessionState::Invalidated, None))
        );
        let inner = engine.inner.lock().unwrap();
        assert!(inner.shared.client().is_none());
        assert!(inner.shared.client_epoch.load(Ordering::SeqCst) > before);
        assert!(!inner.shared.bootstrap_done.load(Ordering::SeqCst));
        drop(inner);
        engine.shutdown();
    }
}
