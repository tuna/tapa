//! End-to-end HLS integration. Gated `#[ignore]`; the body runs only
//! when the environment supplies either a local Xilinx install or
//! `VARS.local.bzl`-style remote host variables.

mod common;

use std::sync::Arc;

use tapa_xilinx::{
    RemoteToolRunner, SshMuxOptions, SshSession, ToolInvocation, ToolRunner,
};

#[test]
#[ignore = "requires real vitis_hls or configured remote host"]
fn vitis_hls_round_trips_vadd_fixture() {
    if common::should_skip_without_env() {
        eprintln!("integration_hls: no XILINX_HLS and no REMOTE_HOST; skipping");
        return;
    }
    // Happy path: ask the environment for the tool's version string.
    if let Some(cfg) = common::has_remote_config() {
        let session = Arc::new(SshSession::new(cfg, SshMuxOptions::default()));
        session.ensure_established().expect("ssh setup");
        let runner = RemoteToolRunner::new(session);
        let out = runner
            .run(
                &ToolInvocation::new("vitis_hls")
                    .arg("-version"),
            )
            .expect("vitis_hls -version");
        assert!(
            out.stdout.contains("Vitis HLS") || out.stderr.contains("Vitis HLS"),
            "unexpected vitis_hls banner: stdout={} stderr={}",
            out.stdout,
            out.stderr
        );
    } else {
        let out = std::process::Command::new("vitis_hls")
            .arg("-version")
            .output()
            .expect("spawn local vitis_hls");
        let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
        s.push_str(&String::from_utf8_lossy(&out.stderr));
        assert!(s.contains("Vitis HLS"), "banner not seen: {s}");
    }
}
