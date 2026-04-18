//! Remote tool runner: tar-pipe uploads / downloads and remote
//! invocation through a shared `SshSession`.
//!
//! The live implementation (tar-pipe, env allowlist, reconnect via
//! `classify_ssh_error`) is deferred to the remote-execution
//! milestone. This module fixes the type shape the orchestrators and
//! the PyO3 wrapper compile against.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;

use crate::error::{Result, XilinxError};
use crate::runtime::process::{ToolInvocation, ToolOutput, ToolRunner};
use crate::runtime::ssh::{classify_ssh_error, SshErrorKind, SshSession};

pub struct RemoteToolRunner {
    session: Arc<SshSession>,
}

impl RemoteToolRunner {
    pub fn new(session: Arc<SshSession>) -> Self {
        Self { session }
    }

    pub fn session(&self) -> &SshSession {
        &self.session
    }

    /// Build the base `ssh <target>` command populated with the
    /// session's multiplexing args.
    fn ssh_cmd(&self, remote_cmd: &str) -> Command {
        let mut args = self.session.build_ssh_args();
        args.push(self.session.ssh_target());
        args.push(remote_cmd.to_string());
        let mut cmd = Command::new("ssh");
        cmd.args(&args);
        cmd
    }

    fn classify_remote_failure(&self, stderr: &str) -> XilinxError {
        match classify_ssh_error(stderr) {
            SshErrorKind::TransientMux => {
                self.session.invalidate();
                XilinxError::SshMuxLost {
                    detail: stderr.to_string(),
                }
            }
            _ => XilinxError::RemoteTransfer(stderr.to_string()),
        }
    }
}

fn remote_work_dir(session: &SshSession) -> String {
    session.config().work_dir.clone()
}

fn shell_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        if c == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

/// Tar-pipe `src` (a directory or a single file) into `remote_work`
/// on the remote. Uploads via `tar -cf - ... | ssh <target> "cd <work>
/// && tar -xf -"`.
fn upload_path(session: &SshSession, src: &Path, remote_work: &str) -> Result<()> {
    if !src.exists() {
        return Err(XilinxError::RemoteTransfer(format!(
            "upload source does not exist: {}",
            src.display()
        )));
    }
    let (tar_dir, tar_target) = if src.is_dir() {
        (src.to_path_buf(), ".".to_string())
    } else {
        (
            src.parent().map_or_else(|| PathBuf::from("."), Path::to_path_buf),
            src.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
        )
    };

    let mut tar_local = Command::new("tar")
        .arg("-cf")
        .arg("-")
        .arg("-C")
        .arg(&tar_dir)
        .arg(&tar_target)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| XilinxError::RemoteTransfer(format!("spawn local tar: {e}")))?;

    let remote_cmd = format!(
        "mkdir -p {work} && cd {work} && tar -xf -",
        work = shell_quote(remote_work)
    );
    let mut args = session.build_ssh_args();
    args.push(session.ssh_target());
    args.push(remote_cmd);
    let mut ssh_child = Command::new("ssh")
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| XilinxError::RemoteTransfer(format!("spawn ssh for upload: {e}")))?;

    let mut tar_out = tar_local
        .stdout
        .take()
        .ok_or_else(|| XilinxError::RemoteTransfer("tar stdout lost".into()))?;
    let mut ssh_in = ssh_child
        .stdin
        .take()
        .ok_or_else(|| XilinxError::RemoteTransfer("ssh stdin lost".into()))?;
    std::io::copy(&mut tar_out, &mut ssh_in)
        .map_err(|e| XilinxError::RemoteTransfer(format!("tar-pipe copy: {e}")))?;
    drop(ssh_in);

    let tar_status = tar_local
        .wait()
        .map_err(|e| XilinxError::RemoteTransfer(format!("wait local tar: {e}")))?;
    let ssh_out = ssh_child
        .wait_with_output()
        .map_err(|e| XilinxError::RemoteTransfer(format!("wait ssh upload: {e}")))?;
    if !tar_status.success() {
        return Err(XilinxError::RemoteTransfer(format!(
            "local tar failed: {tar_status}"
        )));
    }
    if !ssh_out.status.success() {
        return Err(XilinxError::RemoteTransfer(format!(
            "remote tar-extract failed: {}",
            String::from_utf8_lossy(&ssh_out.stderr)
        )));
    }
    Ok(())
}

/// Download a single remote path (relative to `remote_work`) into
/// `dest` on the local side, using `ssh "tar -cf -" | tar -xf -`.
fn download_path(
    session: &SshSession,
    remote_work: &str,
    remote_path: &str,
    dest: &Path,
) -> Result<()> {
    std::fs::create_dir_all(dest).map_err(|e| {
        XilinxError::RemoteTransfer(format!("mkdir {}: {e}", dest.display()))
    })?;
    let remote_cmd = format!(
        "cd {work} && tar -cf - {path}",
        work = shell_quote(remote_work),
        path = shell_quote(remote_path),
    );
    let mut args = session.build_ssh_args();
    args.push(session.ssh_target());
    args.push(remote_cmd);
    let mut ssh_child = Command::new("ssh")
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| XilinxError::RemoteTransfer(format!("spawn ssh for download: {e}")))?;

    let ssh_out = ssh_child
        .stdout
        .take()
        .ok_or_else(|| XilinxError::RemoteTransfer("ssh stdout lost".into()))?;
    let mut tar_local = Command::new("tar")
        .arg("-xf")
        .arg("-")
        .arg("-C")
        .arg(dest)
        .stdin(Stdio::from(ssh_out))
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| XilinxError::RemoteTransfer(format!("spawn local tar -x: {e}")))?;

    let tar_status = tar_local
        .wait()
        .map_err(|e| XilinxError::RemoteTransfer(format!("wait local tar -x: {e}")))?;
    let ssh_status = ssh_child
        .wait()
        .map_err(|e| XilinxError::RemoteTransfer(format!("wait ssh download: {e}")))?;
    if !ssh_status.success() {
        return Err(XilinxError::RemoteTransfer(format!(
            "remote tar -c failed: exit {ssh_status}"
        )));
    }
    if !tar_status.success() {
        return Err(XilinxError::RemoteTransfer(format!(
            "local tar -x failed: {tar_status}"
        )));
    }
    Ok(())
}

impl ToolRunner for RemoteToolRunner {
    fn run(&self, inv: &ToolInvocation) -> Result<ToolOutput> {
        self.session.ensure_established()?;
        let work_dir = remote_work_dir(&self.session);

        for src in &inv.uploads {
            upload_path(&self.session, src, &work_dir)?;
        }

        // Build `cd <work> && <env> <program> <args>`.
        let mut env_prefix = String::new();
        for (k, v) in &inv.env {
            use std::fmt::Write as _;
            let _ = write!(env_prefix, "{}={} ", k, shell_quote(v));
        }
        let mut cmd_str = format!(
            "cd {work} && {env}{prog}",
            work = shell_quote(&work_dir),
            env = env_prefix,
            prog = shell_quote(&inv.program),
        );
        for a in &inv.args {
            cmd_str.push(' ');
            cmd_str.push_str(&shell_quote(a));
        }

        let mut ssh = self.ssh_cmd(&cmd_str);
        ssh.stdout(Stdio::piped());
        ssh.stderr(Stdio::piped());
        if inv.stdin.is_some() {
            ssh.stdin(Stdio::piped());
        }
        let mut child = ssh.spawn().map_err(|e| XilinxError::RemoteTransfer(format!(
            "spawn ssh exec: {e}"
        )))?;
        if let Some(bytes) = &inv.stdin {
            if let Some(mut si) = child.stdin.take() {
                si.write_all(bytes).map_err(|e| {
                    XilinxError::RemoteTransfer(format!("write stdin: {e}"))
                })?;
            }
        }
        let out = child.wait_with_output().map_err(|e| {
            XilinxError::RemoteTransfer(format!("wait ssh exec: {e}"))
        })?;
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        let code = out.status.code().unwrap_or(-1);
        if code != 0
            && !stderr.is_empty()
            && classify_ssh_error(&stderr) == SshErrorKind::TransientMux
        {
            return Err(self.classify_remote_failure(&stderr));
        }

        for dl in &inv.downloads {
            if let Some(name) = dl.file_name().map(|n| n.to_string_lossy().into_owned()) {
                let parent = dl.parent().unwrap_or_else(|| Path::new("."));
                download_path(&self.session, &work_dir, &name, parent)?;
            }
        }

        Ok(ToolOutput {
            exit_code: code,
            stdout,
            stderr,
        })
    }
}

/// One-shot vendor header sync from the configured remote.
///
/// Ports `tapa/remote/vendor.py`. Uses tar-pipe to stream
/// `<remote_xilinx_tool_path>/Vitis_HLS/<version>/include` into a
/// deterministic cache dir under the local user's cache.
///
/// Idempotent: if the cache's `.stamp` file matches the configured
/// remote path, skips the re-sync.
pub fn sync_remote_vendor_includes(session: &SshSession) -> Result<PathBuf> {
    let cfg = session.config();
    let remote_root = cfg
        .xilinx_settings
        .clone()
        .ok_or_else(|| {
            XilinxError::RemoteTransfer(
                "sync_remote_vendor_includes: remote xilinx_settings unset".into(),
            )
        })?;
    session.ensure_established()?;
    let cache_root = cache_dir_for(&cfg.host)?;
    let stamp = cache_root.join(".vendor_stamp");
    let expected = remote_root;
    if stamp.is_file() {
        if let Ok(prev) = std::fs::read_to_string(&stamp) {
            if prev.trim() == expected {
                return Ok(cache_root);
            }
        }
    }
    std::fs::create_dir_all(&cache_root)
        .map_err(|e| XilinxError::RemoteTransfer(format!("mkdir cache: {e}")))?;

    // Stream the include tree directly from the remote root.
    let remote_cmd = format!(
        "cd {root} && tar -cf - .",
        root = shell_quote(&expected),
    );
    let mut args = session.build_ssh_args();
    args.push(session.ssh_target());
    args.push(remote_cmd);
    let ssh = Command::new("ssh")
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| XilinxError::RemoteTransfer(format!("spawn ssh: {e}")))?;
    let ssh_out = ssh
        .stdout
        .ok_or_else(|| XilinxError::RemoteTransfer("ssh stdout lost".into()))?;
    let mut tar_local = Command::new("tar")
        .arg("-xf")
        .arg("-")
        .arg("-C")
        .arg(&cache_root)
        .stdin(Stdio::from(ssh_out))
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| XilinxError::RemoteTransfer(format!("spawn local tar -x: {e}")))?;
    let status = tar_local
        .wait()
        .map_err(|e| XilinxError::RemoteTransfer(format!("wait local tar -x: {e}")))?;
    if !status.success() {
        return Err(XilinxError::RemoteTransfer(format!(
            "vendor-includes tar -x failed: {status}"
        )));
    }
    std::fs::write(&stamp, expected)
        .map_err(|e| XilinxError::RemoteTransfer(format!("write stamp: {e}")))?;
    Ok(cache_root)
}

fn cache_dir_for(host: &str) -> Result<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .ok_or_else(|| {
            XilinxError::RemoteTransfer("cannot resolve cache dir (no HOME/XDG_CACHE_HOME)".into())
        })?;
    Ok(base.join("tapa").join("vendor").join(host))
}
