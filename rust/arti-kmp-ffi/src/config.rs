//! Exact client configuration identity and caller-supplied directory resolution.
use crate::{ArtiConfig, ArtiError, BridgesEnabled};
use arti_client::config::{
    BoolOrAuto, BridgeConfigBuilder, TorClientConfig, TorClientConfigBuilder,
};
use std::path::PathBuf;
use std::time::Duration;

/// TorClient-defining configuration: changing any of these requires a new
/// TorClient/bootstrap. `socks_port` is deliberately excluded (rebind only).
/// Bridge comparison is exact: non-empty → empty counts as a change.
pub(super) fn tor_client_config_changed(old: &ArtiConfig, new: &ArtiConfig) -> bool {
    old.data_dir != new.data_dir
        || old.state_dir != new.state_dir
        || old.cache_dir != new.cache_dir
        || old.bridges != new.bridges
        || old.bridges_enabled != new.bridges_enabled
        || old.allow_onion_addrs != new.allow_onion_addrs
        || old.connect_timeout_nanos != new.connect_timeout_nanos
        || old.resolve_timeout_nanos != new.resolve_timeout_nanos
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

/// Validate every supplied line using pinned Arti syntax before mapping policy.
/// OFF drops only already-validated lines. Never display/debug upstream bridge
/// errors: their payloads may contain addresses, identities or PT settings.
pub(super) fn build_tor_config(config: &ArtiConfig) -> Result<TorClientConfig, ArtiError> {
    let (connect_timeout, resolve_timeout) = stream_timeouts(config)?;
    let invalid_bridge = || ArtiError::Config {
        msg: "invalid bridge configuration".into(),
    };
    let mut validated = Vec::with_capacity(config.bridges.len());
    for line in &config.bridges {
        let bridge = line
            .parse::<BridgeConfigBuilder>()
            .map_err(|_| invalid_bridge())?;
        bridge.build().map_err(|_| invalid_bridge())?;
        validated.push(bridge);
    }
    let (state_dir, cache_dir) = resolve_dirs(config);
    let mut builder = TorClientConfigBuilder::from_directories(state_dir, cache_dir);
    builder
        .address_filter()
        .allow_onion_addrs(config.allow_onion_addrs);
    builder.stream_timeouts().connect_timeout(connect_timeout);
    builder.stream_timeouts().resolve_timeout(resolve_timeout);
    builder.bridges().enabled(match config.bridges_enabled {
        BridgesEnabled::Auto => BoolOrAuto::Auto,
        BridgesEnabled::On => BoolOrAuto::Explicit(true),
        BridgesEnabled::Off => BoolOrAuto::Explicit(false),
    });
    if config.bridges_enabled != BridgesEnabled::Off {
        *builder.bridges().bridges() = validated;
    }
    // Includes upstream's ON + empty pre-build check. Building the whole
    // config here also prevents construction errors from reaching a worker.
    builder.build().map_err(|_| ArtiError::Config {
        msg: "invalid client configuration".into(),
    })
}

/// Signed nanoseconds preserve Kotlin Duration precision over UniFFI.
/// Validation occurs before any engine transition or client construction.
pub(super) fn stream_timeouts(config: &ArtiConfig) -> Result<(Duration, Duration), ArtiError> {
    let convert = |nanos: i64| {
        u64::try_from(nanos)
            .map(Duration::from_nanos)
            .map_err(|_| ArtiError::Config {
                msg: "stream timeouts must be nonnegative".into(),
            })
    };
    Ok((
        convert(config.connect_timeout_nanos)?,
        convert(config.resolve_timeout_nanos)?,
    ))
}
