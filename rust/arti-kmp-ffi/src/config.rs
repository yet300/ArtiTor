//! Exact client configuration identity and caller-supplied directory resolution.
use crate::ArtiConfig;
use std::path::PathBuf;

/// TorClient-defining configuration: changing any of these requires a new
/// TorClient/bootstrap. `socks_port` is deliberately excluded (rebind only).
/// Bridge comparison is exact: non-empty → empty counts as a change.
pub(super) fn tor_client_config_changed(old: &ArtiConfig, new: &ArtiConfig) -> bool {
    old.data_dir != new.data_dir
        || old.state_dir != new.state_dir
        || old.cache_dir != new.cache_dir
        || old.bridges != new.bridges
}

pub(super) fn resolve_dirs(config: &ArtiConfig) -> (PathBuf, PathBuf) {
    let data = PathBuf::from(&config.data_dir);
    let state = config
        .state_dir
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or_else(|| data.join("state"));
    let cache = config
        .cache_dir
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or_else(|| data.join("cache"));
    (state, cache)
}
