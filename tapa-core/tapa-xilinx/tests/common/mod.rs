//! Gating helpers shared by the `#[ignore]` integration tests.
//!
//! Each integration test file calls `skip_unless_any(...)` at the top
//! of its body; when neither a local Xilinx install nor a
//! `VARS.local.bzl`-derived remote host is available, the test returns
//! cleanly instead of reporting a spurious failure.

use tapa_xilinx::RemoteConfig;

#[allow(dead_code, reason = "used by integration_{hls,vivado} but not integration_remote")]
pub fn has_local_xilinx() -> bool {
    cfg!(target_os = "linux") && std::env::var("XILINX_HLS").is_ok()
}

pub fn has_remote_config() -> Option<RemoteConfig> {
    RemoteConfig::from_env()
}

#[allow(dead_code, reason = "used by integration_{hls,vivado} but not integration_remote")]
pub fn should_skip_without_env() -> bool {
    !has_local_xilinx() && has_remote_config().is_none()
}
