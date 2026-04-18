//! SSH control-master lifecycle integration. Requires a live remote
//! host (`VARS.local.bzl`-style `REMOTE_HOST` / `REMOTE_USER` / etc.).
//!
//! The test drives the *real* auto-restart path: after
//! `ensure_established()` opens a control-master socket, the test
//! tears the master down via `ssh -O exit` **and** force-removes any
//! `cm-*` files still on disk. The next `RemoteToolRunner::run()`
//! must survive the resulting transient mux failure by reconnecting
//! and retrying the in-flight command; the invocation must report
//! success, its stdout must contain the expected marker, and a new
//! control socket must appear on disk. This mirrors OpenSSH's real
//! failure mode (master evicted by cleanup / reboot) rather than
//! calling `invalidate()` on the in-process flag or asserting on a
//! subsequent cold connect.

mod common;

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;

use tapa_xilinx::{
    RemoteToolRunner, SshMuxOptions, SshSession, ToolInvocation, ToolRunner,
};

fn existing_control_sockets(dir: &PathBuf) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|s| s.starts_with("cm-"))
        })
        .collect()
}

#[test]
#[ignore = "requires configured remote host"]
fn control_master_lifecycle() {
    let Some(cfg) = common::has_remote_config() else {
        eprintln!("integration_remote: no REMOTE_HOST; skipping");
        return;
    };
    let session = Arc::new(SshSession::new(cfg, SshMuxOptions::default()));

    // 1. First ensure_established opens (or reuses) the socket.
    session
        .ensure_established()
        .expect("first ensure_established must succeed");
    assert!(
        session.control_master_alive(),
        "`ssh -O check` must report alive after ensure_established"
    );

    // 2. Baseline remote exec via `RemoteToolRunner`.
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

    // 3. Tear the master down the same way OpenSSH's own tooling would
    //    observe failure: issue `ssh -O exit` to close the master
    //    cleanly, then belt-and-suspenders remove every cm-* file
    //    still on disk. This emulates the production failure mode
    //    (master killed, socket evicted).
    let ctrl_dir = session.control_dir();
    let exit_status = {
        // Use the same ssh argv the session would.
        let mut args = Vec::new();
        args.push("-o".into());
        args.push("BatchMode=yes".into());
        args.push("-o".into());
        args.push(format!("ControlPath={}/cm-%C", ctrl_dir.display()));
        args.push("-O".into());
        args.push("exit".into());
        args.push(format!(
            "{}@{}",
            session.config().user,
            session.config().host
        ));
        Command::new("ssh")
            .args(&args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
    };
    let _ = exit_status; // may or may not succeed depending on race
    for s in existing_control_sockets(&ctrl_dir) {
        std::fs::remove_file(&s).expect("unlink control socket must succeed");
        assert!(!s.exists(), "socket still present: {}", s.display());
    }
    assert!(
        !session.control_master_alive(),
        "`ssh -O check` must report dead after reset"
    );

    // 4. The next invocation must succeed. `RemoteToolRunner::run`
    //    internally detects the transient mux error, resets the
    //    master, and retries the command once — the caller sees a
    //    clean success, not `SshMuxLost`.
    let out2 = runner
        .run(&ToolInvocation::new("echo").arg("post-restart"))
        .expect(
            "remote echo after mux teardown must succeed via auto-restart",
        );
    assert_eq!(out2.exit_code, 0, "stderr after restart: {}", out2.stderr);
    assert!(
        out2.stdout.contains("post-restart"),
        "unexpected stdout after restart: {}",
        out2.stdout
    );

    // 5. A fresh socket must now be present.
    let restored = existing_control_sockets(&ctrl_dir);
    assert!(
        !restored.is_empty(),
        "no control socket re-created under {}",
        ctrl_dir.display()
    );
    assert!(
        session.control_master_alive(),
        "`ssh -O check` must report alive after auto-restart"
    );
}
