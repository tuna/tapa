//! Remote tool runner: tar-pipe uploads / downloads and remote
//! invocation through a shared `SshSession`.
//!
//! Each `RemoteToolRunner::run` call opens a per-invocation
//! `<work_dir>/<session_id>` directory on the remote, mirrors the
//! caller's `cwd` + uploads under `rootfs/`, rewrites every absolute
//! local path in the command args / env / stdin to its
//! session-scoped remote equivalent, executes the tool with the
//! remote working directory pointed at the rewritten `cwd`, then
//! tar-pipes each requested download path back from its rootfs
//! counterpart. On a transient mux failure `run_with_mux_retry`
//! tears the master down, re-establishes the control socket, and
//! retries the in-flight command once.

mod transport;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;

pub(crate) use self::transport::shell_quote;
use self::transport::{
    cleanup_session, download_tree, local_to_remote_path, unique_session_id,
    upload_batch,
};
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

#[allow(dead_code, reason = "retained for future diagnostic paths; \
         run_once now routes everything through session_dir")]
fn remote_work_dir(session: &SshSession) -> String {
    session.config().work_dir.clone()
}

/// Rewrite every occurrence of a local absolute path in `text` to its
/// session-scoped remote equivalent. Longest-match-first ensures a
/// path that is a prefix of another (e.g. `/a/b` vs `/a/b/c`) is not
/// double-replaced. Mirrors `tapa/remote/popen.py::_rewrite_paths_in_string`.
fn rewrite_abs_paths(
    text: &str,
    local_paths: &[PathBuf],
    session_dir: &str,
) -> String {
    if local_paths.is_empty() {
        return text.to_string();
    }
    let mut sorted: Vec<&PathBuf> = local_paths.iter().collect();
    sorted.sort_by_key(|p| std::cmp::Reverse(p.as_os_str().len()));
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0usize;
    let bytes = text.as_bytes();
    'outer: while cursor < bytes.len() {
        for p in &sorted {
            let ps = p.to_string_lossy();
            let ps = ps.as_ref();
            if ps.is_empty() {
                continue;
            }
            if bytes[cursor..].starts_with(ps.as_bytes()) {
                out.push_str(&local_to_remote_path(p, session_dir));
                cursor += ps.len();
                continue 'outer;
            }
        }
        let rest = &text[cursor..];
        let ch = rest.chars().next().unwrap();
        out.push(ch);
        cursor += ch.len_utf8();
    }
    out
}

/// Environment variable allowlist mirroring
/// `tapa/remote/popen.py::_REMOTE_ENV_ALLOWLIST`. Anything else is
/// dropped unless the key begins with `TAPA_`.
const REMOTE_ENV_ALLOWLIST: &[&str] = &["HOME", "LANG", "LC_ALL", "LC_CTYPE"];

fn is_forwardable_env(key: &str) -> bool {
    REMOTE_ENV_ALLOWLIST.contains(&key) || key.starts_with("TAPA_")
}

impl RemoteToolRunner {
    #[allow(
        clippy::too_many_lines,
        reason = "mirrors `tapa/remote/popen.py::RemoteToolProcess.communicate` \
                  verbatim; splitting further would obscure the Python parity"
    )]
    /// Ports `tapa/remote/popen.py::RemoteToolProcess.communicate`:
    /// opens a per-invocation session directory with a `rootfs/`
    /// subtree, mirrors the local `cwd` plus any extra uploads under
    /// that rootfs, rewrites absolute local paths in the command
    /// args / env / stdin to their session-relative remote
    /// equivalents, executes the command with the remote working
    /// directory pointed at the rewritten cwd, and then tar-pipes
    /// each requested download path back from its rootfs
    /// counterpart.
    fn run_once(&self, inv: &ToolInvocation) -> Result<ToolOutput> {
        self.session.ensure_established()?;
        let cfg = self.session.config();
        let session_dir = format!("{}/{}", cfg.work_dir, unique_session_id());

        let mut referenced: Vec<PathBuf> = Vec::new();
        if let Some(cwd) = inv.cwd.as_ref() {
            if cwd.is_absolute() {
                referenced.push(cwd.clone());
            }
        }
        for p in &inv.uploads {
            if p.is_absolute() {
                referenced.push(p.clone());
            }
        }
        for p in &inv.downloads {
            if p.is_absolute() {
                referenced.push(p.clone());
            }
        }
        let mut seen: std::collections::HashSet<PathBuf> =
            std::collections::HashSet::new();
        referenced.retain(|p| seen.insert(p.clone()));

        let mut to_upload: Vec<PathBuf> = Vec::new();
        if let Some(cwd) = inv.cwd.as_ref() {
            if cwd.is_absolute() && cwd.exists() {
                to_upload.push(cwd.clone());
            }
        }
        for p in &inv.uploads {
            if p.is_absolute() && p.exists() {
                to_upload.push(p.clone());
            }
        }
        let mut seen2: std::collections::HashSet<PathBuf> =
            std::collections::HashSet::new();
        to_upload.retain(|p| seen2.insert(p.clone()));
        upload_batch(&self.session, &session_dir, &to_upload)?;

        let remote_cwd = match inv.cwd.as_ref().filter(|p| p.is_absolute()) {
            Some(cwd) => local_to_remote_path(cwd, &session_dir),
            None => format!("{session_dir}/rootfs"),
        };

        let mut parts: Vec<String> = Vec::new();
        if let Some(xs) = cfg.xilinx_settings.as_ref() {
            if !xs.trim().is_empty() {
                parts.push(format!("source {}", shell_quote(xs)));
            }
        }
        for (k, v) in &inv.env {
            if !is_forwardable_env(k) {
                continue;
            }
            let rv = rewrite_abs_paths(v, &referenced, &session_dir);
            parts.push(format!("export {}={}", k, shell_quote(&rv)));
        }
        let rewritten_args: Vec<String> = inv
            .args
            .iter()
            .map(|a| rewrite_abs_paths(a, &referenced, &session_dir))
            .collect();
        let exec = std::iter::once(shell_quote(&inv.program))
            .chain(rewritten_args.iter().map(|a| shell_quote(a)))
            .collect::<Vec<_>>()
            .join(" ");
        parts.push(format!(
            "cd {} && exec {}",
            shell_quote(&remote_cwd),
            exec
        ));
        let full_cmd = parts.join(" ; ");
        let wrapped = format!("bash -c {}", shell_quote(&full_cmd));

        let mut ssh = self.ssh_cmd(&wrapped);
        ssh.stdout(Stdio::piped());
        ssh.stderr(Stdio::piped());
        if inv.stdin.is_some() {
            ssh.stdin(Stdio::piped());
        }
        let mut child = ssh.spawn().map_err(|e| {
            XilinxError::RemoteTransfer(format!("spawn ssh exec: {e}"))
        })?;
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
            cleanup_session(&self.session, &session_dir);
            return Err(self.classify_remote_failure(&stderr));
        }

        for dl in &inv.downloads {
            if !dl.is_absolute() {
                continue;
            }
            let remote_src = local_to_remote_path(dl, &session_dir);
            download_tree(&self.session, &remote_src, dl)?;
        }

        cleanup_session(&self.session, &session_dir);

        Ok(ToolOutput {
            exit_code: code,
            stdout,
            stderr,
        })
    }
}

/// Pure mux-retry driver. Takes two callbacks — the attempt and the
/// recovery — so the retry branch is unit-testable without a live
/// `SshSession`. `attempt` is invoked once; if it returns a
/// recoverable mux error (per [`is_recoverable_mux_error`]) the
/// caller runs `recover` (typically `reset_mux` + `ensure_established`)
/// and then retries `attempt` exactly once. Non-recoverable errors
/// pass through unchanged, matching
/// `tapa/remote/popen.py::RemoteToolProcess` semantics.
fn run_with_mux_retry<A, R>(mut attempt: A, mut recover: R) -> Result<ToolOutput>
where
    A: FnMut() -> Result<ToolOutput>,
    R: FnMut() -> Result<()>,
{
    match attempt() {
        Ok(out) => Ok(out),
        Err(err) if is_recoverable_mux_error(&err) => {
            recover()?;
            attempt()
        }
        Err(e) => Err(e),
    }
}

impl ToolRunner for RemoteToolRunner {
    fn run(&self, inv: &ToolInvocation) -> Result<ToolOutput> {
        run_with_mux_retry(
            || self.run_once(inv),
            || {
                self.session.reset_mux();
                self.session.ensure_established()
            },
        )
    }

    fn harvest(
        &self,
        _relative_from_cwd: &Path,
        _local_root: &Path,
    ) -> Result<()> {
        // `run_once` already pulls every caller-requested absolute-
        // local download back into place, so this is a no-op on the
        // remote runner. Kept for interface symmetry.
        Ok(())
    }
}

fn is_recoverable_mux_error(err: &XilinxError) -> bool {
    match err {
        XilinxError::SshMuxLost { .. } => true,
        XilinxError::RemoteTransfer(msg) => {
            classify_ssh_error(msg) == SshErrorKind::TransientMux
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    //! Unit tests for the `run_with_mux_retry` helper. The retry
    //! branch is pure (no SSH required), so this block proves the
    //! contract independently of any live host. Integration tests
    //! that drive a real `RemoteToolRunner` live under
    //! `tests/integration_remote.rs`.

    use super::*;
    use crate::runtime::process::ToolOutput;
    use std::cell::Cell;

    #[test]
    fn mux_retry_recovers_then_returns_success() {
        let call_count = Cell::new(0u32);
        let recover_count = Cell::new(0u32);
        let out = run_with_mux_retry(
            || {
                let n = call_count.get();
                call_count.set(n + 1);
                if n == 0 {
                    Err(XilinxError::SshMuxLost {
                        detail: "mux_client_read_packet: broken pipe".into(),
                    })
                } else {
                    Ok(ToolOutput {
                        exit_code: 0,
                        stdout: "post-retry-ok".into(),
                        stderr: String::new(),
                    })
                }
            },
            || {
                recover_count.set(recover_count.get() + 1);
                Ok(())
            },
        )
        .expect("retry must surface the second attempt's Ok");
        assert_eq!(out.stdout, "post-retry-ok");
        assert_eq!(call_count.get(), 2);
        assert_eq!(recover_count.get(), 1);
    }

    #[test]
    fn mux_retry_non_recoverable_err_propagates_without_retry() {
        let call_count = Cell::new(0u32);
        let recover_count = Cell::new(0u32);
        let err = run_with_mux_retry(
            || {
                call_count.set(call_count.get() + 1);
                Err(XilinxError::ToolFailure {
                    program: "vivado".into(),
                    code: 1,
                    stderr: "license error".into(),
                })
            },
            || {
                recover_count.set(recover_count.get() + 1);
                Ok(())
            },
        )
        .expect_err("non-recoverable error must propagate");
        assert!(matches!(err, XilinxError::ToolFailure { .. }));
        assert_eq!(call_count.get(), 1);
        assert_eq!(recover_count.get(), 0);
    }

    #[test]
    fn mux_retry_transfer_stage_transient_remote_transfer_recovers() {
        let call_count = Cell::new(0u32);
        let out = run_with_mux_retry(
            || {
                let n = call_count.get();
                call_count.set(n + 1);
                if n == 0 {
                    Err(XilinxError::RemoteTransfer(
                        "remote tar -cz failed: mux_client_read_packet: read from master failed: Broken pipe"
                            .into(),
                    ))
                } else {
                    Ok(ToolOutput {
                        exit_code: 0,
                        stdout: "download-ok".into(),
                        stderr: String::new(),
                    })
                }
            },
            || Ok(()),
        )
        .expect("transfer-stage transient must retry");
        assert_eq!(out.stdout, "download-ok");
        assert_eq!(call_count.get(), 2);
    }

    #[test]
    fn mux_retry_second_attempt_err_propagates() {
        let call_count = Cell::new(0u32);
        let err = run_with_mux_retry(
            || {
                let n = call_count.get();
                call_count.set(n + 1);
                if n == 0 {
                    Err(XilinxError::SshMuxLost {
                        detail: "broken pipe".into(),
                    })
                } else {
                    Err(XilinxError::ToolFailure {
                        program: "vivado".into(),
                        code: 2,
                        stderr: "real failure".into(),
                    })
                }
            },
            || Ok(()),
        )
        .expect_err("second attempt err must propagate");
        assert!(matches!(err, XilinxError::ToolFailure { code: 2, .. }));
        assert_eq!(call_count.get(), 2);
    }
}
