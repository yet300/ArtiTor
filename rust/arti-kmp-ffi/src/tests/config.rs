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
