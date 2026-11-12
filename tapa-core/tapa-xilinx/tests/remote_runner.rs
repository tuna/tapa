//! `RemoteToolRunner` end to end through a stand-in `ssh` that runs the
//! remote command on this machine: the upload, path relocation, remote
//! script, and download stages all execute for real, with no live host.

use std::os::unix::fs::PermissionsExt as _;
use std::sync::{Arc, Once};

use camino::Utf8PathBuf;
use tapa_xilinx::{RemoteConfig, RemoteToolRunner, SshSession, ToolInvocation, ToolRunner};

/// `ssh [-o K=V | -p N | -i KEY | -O OP]... TARGET COMMAND...`, run locally
/// the way sshd hands COMMAND to the login shell.
const FAKE_SSH: &str = r#"#!/bin/sh
while [ $# -gt 0 ]; do
  case "$1" in
    -o|-p|-i) shift 2 ;;
    -O) exit 0 ;;
    *) break ;;
  esac
done
shift
exec sh -c "$*"
"#;

fn install_fake_ssh() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let bin = tempfile::Builder::new()
            .prefix("fake-ssh")
            .tempdir()
            .expect("fake ssh dir")
            .keep();
        let ssh = bin.join("ssh");
        std::fs::write(&ssh, FAKE_SSH).expect("write fake ssh");
        std::fs::set_permissions(&ssh, std::fs::Permissions::from_mode(0o755))
            .expect("chmod fake ssh");
        let path = std::env::var_os("PATH").unwrap_or_default();
        let mut paths = vec![bin];
        paths.extend(std::env::split_paths(&path));
        // The only environment change in this test binary, made once before
        // any runner spawns `ssh`.
        std::env::set_var("PATH", std::env::join_paths(paths).expect("PATH"));
    });
}

/// A runner whose remote work dir and local tree both sit under `root`,
/// which contains a space so every quoting stage is exercised.
fn runner(root: &Utf8PathBuf) -> RemoteToolRunner {
    install_fake_ssh();
    let cfg = RemoteConfig {
        host: "fake-host".into(),
        user: "tapa".into(),
        port: 22,
        key_file: None,
        xilinx_settings: None,
        work_dir: root.join("remote work").into_string(),
        ssh_control_dir: None,
        ssh_control_persist: "30m".into(),
        ssh_multiplex: false,
    };
    RemoteToolRunner::new(Arc::new(SshSession::new(cfg)))
}

fn temp_root() -> (tempfile::TempDir, Utf8PathBuf) {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = Utf8PathBuf::from_path_buf(temp.path().join("tapa root")).expect("UTF-8");
    std::fs::create_dir_all(root.join("local work")).expect("local work dir");
    (temp, root)
}

#[test]
fn a_failed_tool_reports_its_own_failure_not_a_missing_download() {
    let (_temp, root) = temp_root();
    let work = root.join("local work");
    let mut inv = ToolInvocation::new("sh")
        .arg("-c")
        .arg("echo 'ERROR: the real diagnostic' >&2; exit 3");
    inv.cwd = Some(work.clone());
    inv.downloads.push(work.join("never-written.xclbin"));

    let out = runner(&root)
        .run(&inv)
        .expect("a tool failure is a tool output, not a transfer error");
    assert_eq!(out.exit_code, 3);
    assert!(out.stderr.contains("the real diagnostic"), "{}", out.stderr);
}

/// ssh exits 255 for its own failures and forwards a tool's status
/// otherwise, so a tool whose own log says "Broken pipe" failed once: it
/// must neither be run again nor be reported as a lost SSH connection.
#[test]
fn a_tool_failure_mentioning_a_broken_pipe_is_not_retried() {
    let (_temp, root) = temp_root();
    let work = root.join("local work");
    let runs = root.join("runs");
    let mut inv = ToolInvocation::new("sh").arg("-c").arg(format!(
        "echo run >> '{runs}'; echo 'ERROR: write failed: Broken pipe' >&2; exit 1"
    ));
    inv.cwd = Some(work);

    let out = runner(&root)
        .run(&inv)
        .expect("a tool failure is a tool output, not an SSH error");
    assert_eq!(out.exit_code, 1);
    assert!(out.stderr.contains("Broken pipe"), "{}", out.stderr);
    assert_eq!(std::fs::read_to_string(&runs).expect("runs"), "run\n");
}

#[test]
fn a_successful_tool_output_is_downloaded_to_the_local_path() {
    let (_temp, root) = temp_root();
    let work = root.join("local work");
    std::fs::write(work.join("input.txt"), "uploaded\n").expect("input");
    let mut inv = ToolInvocation::new("sh")
        .arg("-c")
        .arg("mkdir -p 'out dir' && cp input.txt 'out dir/result.txt'");
    inv.cwd = Some(work.clone());
    inv.downloads.push(work.join("out dir"));

    let out = runner(&root).run(&inv).expect("remote run");
    assert_eq!(out.exit_code, 0, "{}", out.stderr);
    assert_eq!(
        std::fs::read_to_string(work.join("out dir/result.txt")).expect("downloaded"),
        "uploaded\n"
    );
}

#[test]
fn a_successful_tool_missing_its_output_is_an_error() {
    let (_temp, root) = temp_root();
    let work = root.join("local work");
    let mut inv = ToolInvocation::new("true");
    inv.cwd = Some(work.clone());
    inv.downloads.push(work.join("never-written.xclbin"));

    runner(&root)
        .run(&inv)
        .expect_err("a requested output the tool did not write must not pass silently");
}
