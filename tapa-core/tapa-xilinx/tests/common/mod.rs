//! Helpers shared by the `#[ignore]` integration tests: path utilities, and
//! the gates a test checks first (`should_skip_without_env`,
//! `has_remote_config`) so that it returns cleanly instead of failing when
//! neither a local Xilinx install nor a `VARS.local.bzl`-derived remote host
//! is available.

use tapa_xilinx::RemoteConfig;

/// Lossy conversion to `camino::Utf8PathBuf` (TAPA paths are UTF-8 in
/// practice; a non-UTF-8 path degrades to its lossy form rather than
/// failing the test).
#[allow(
    dead_code,
    reason = "used by integration_{hls,remote,pack_xo} but not integration_vivado"
)]
pub fn utf8(p: impl AsRef<std::path::Path>) -> camino::Utf8PathBuf {
    camino::Utf8PathBuf::from(p.as_ref().to_string_lossy().into_owned())
}

/// tapa-core/tapa-xilinx/tests/ → …/tapa (repo root).
#[allow(
    dead_code,
    reason = "used by integration_{hls,pack_xo} but not integration_{remote,vivado}"
)]
pub fn repo_root() -> camino::Utf8PathBuf {
    utf8(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(std::path::Path::parent)
            .expect("manifest parent"),
    )
}

#[allow(
    dead_code,
    reason = "used by integration_{hls,vivado} but not integration_remote"
)]
pub fn has_local_xilinx() -> bool {
    cfg!(target_os = "linux") && std::env::var("XILINX_HLS").is_ok()
}

pub fn has_remote_config() -> Option<RemoteConfig> {
    RemoteConfig::from_env()
}

#[allow(
    dead_code,
    reason = "used by integration_{hls,vivado} but not integration_remote"
)]
pub fn should_skip_without_env() -> bool {
    !has_local_xilinx() && has_remote_config().is_none()
}
