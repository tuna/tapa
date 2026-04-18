//! End-to-end HLS integration. Gated `#[ignore]`; the body runs only
//! when the environment supplies either a local Xilinx install or
//! `VARS.local.bzl`-style remote host variables.

mod common;

#[test]
#[ignore = "requires real vitis_hls or configured remote host"]
fn vitis_hls_round_trips_vadd_fixture() {
    if common::should_skip_without_env() {
        eprintln!("integration_hls: no XILINX_HLS and no REMOTE_HOST; skipping");
        return;
    }
    // Live invocation lands alongside the remote-execution milestone.
    eprintln!("integration_hls: environment available; body pending");
}
