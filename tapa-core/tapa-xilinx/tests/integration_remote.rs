//! SSH control-master lifecycle integration. Requires a live remote
//! host (`VARS.local.bzl`-style `REMOTE_HOST` / `REMOTE_USER` / etc.).

mod common;

use std::sync::Arc;

use tapa_xilinx::{RemoteToolRunner, SshMuxOptions, SshSession, ToolInvocation, ToolRunner};

#[test]
#[ignore = "requires configured remote host"]
fn control_master_lifecycle() {
    let Some(cfg) = common::has_remote_config() else {
        eprintln!("integration_remote: no REMOTE_HOST; skipping");
        return;
    };
    let session = Arc::new(SshSession::new(cfg, SshMuxOptions::default()));

    // 1. First ensure_established must open or reuse the socket.
    session
        .ensure_established()
        .expect("first ensure_established must succeed");

    // 2. Remote exec returns the expected stdout and zero exit code.
    let runner = RemoteToolRunner::new(Arc::clone(&session));
    let out = runner
        .run(&ToolInvocation::new("echo").arg("tapa-remote-ok"))
        .expect("remote echo must succeed");
    assert_eq!(out.exit_code, 0, "stderr = {}", out.stderr);
    assert!(
        out.stdout.contains("tapa-remote-ok"),
        "unexpected stdout: {}",
        out.stdout
    );

    // 3. Invalidate + re-establish — the master must come back.
    session.invalidate();
    session
        .ensure_established()
        .expect("re-establish after invalidate must succeed");
    let out2 = runner
        .run(&ToolInvocation::new("echo").arg("post-restart"))
        .expect("remote echo after restart must succeed");
    assert!(out2.stdout.contains("post-restart"));
}
