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

#[allow(dead_code, reason = "retained for future diagnostic paths; \
         run_once now routes everything through session_dir")]
fn remote_work_dir(session: &SshSession) -> String {
    session.config().work_dir.clone()
}

pub(crate) fn shell_quote(s: &str) -> String {
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

/// Map a local absolute path to the corresponding remote path under
/// the session's `rootfs/` prefix. Mirrors
/// `tapa/remote/popen.py::_local_to_remote_path`: absolute local
/// paths are pasted verbatim under `<session_dir>/rootfs/` after
/// stripping the leading `/`.
fn local_to_remote_path(local: &Path, session_dir: &str) -> String {
    let s = local.to_string_lossy();
    let rel = s.trim_start_matches('/');
    format!("{session_dir}/rootfs/{rel}")
}

/// Upload a batch of local absolute paths into `session_dir/rootfs`
/// via a single `tar | ssh tar -xzf -` session. Each path is added
/// to the archive preserving its absolute layout (minus the leading
/// `/`) so the remote tree mirrors the local one. Matches
/// `tapa/remote/popen.py::_upload_paths`.
#[allow(
    clippy::too_many_lines,
    reason = "streams the in-memory tar archive inline for one SSH session; \
              splitting into helpers would obscure the batched-upload flow"
)]
fn upload_batch(
    session: &SshSession,
    session_dir: &str,
    local_paths: &[PathBuf],
) -> Result<()> {
    let rootfs = format!("{session_dir}/rootfs");
    let remote_cmd = format!(
        "mkdir -p {rf} && tar -xzf - -C {rf} --no-same-owner",
        rf = shell_quote(&rootfs),
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
        .map_err(|e| {
            XilinxError::RemoteTransfer(format!("spawn ssh for upload: {e}"))
        })?;

    let ssh_in = ssh_child
        .stdin
        .take()
        .ok_or_else(|| XilinxError::RemoteTransfer("ssh stdin lost".into()))?;
    // Build the tar archive in-memory from the set of local paths.
    // Using the `tar` crate keeps the streaming logic in one place
    // and mirrors Python's `tarfile` batched upload verbatim.
    let gz = flate2::write::GzEncoder::new(ssh_in, flate2::Compression::fast());
    let mut builder = tar::Builder::new(gz);
    builder.follow_symlinks(true);
    for p in local_paths {
        if !p.exists() {
            continue;
        }
        let rel = p.to_string_lossy();
        let rel = rel.trim_start_matches('/');
        if p.is_dir() {
            for ent in std::fs::read_dir(p)
                .map_err(|e| {
                    XilinxError::RemoteTransfer(format!(
                        "read_dir {}: {e}",
                        p.display()
                    ))
                })?
            {
                let ent = ent.map_err(|e| {
                    XilinxError::RemoteTransfer(format!("read_dir entry: {e}"))
                })?;
                let arc = format!(
                    "{rel}/{}",
                    ent.file_name().to_string_lossy()
                );
                let ty = ent.file_type().map_err(|e| {
                    XilinxError::RemoteTransfer(format!("file_type: {e}"))
                })?;
                if ty.is_dir() {
                    builder.append_dir_all(&arc, ent.path()).map_err(|e| {
                        XilinxError::RemoteTransfer(format!(
                            "tar append dir {arc}: {e}"
                        ))
                    })?;
                } else if ty.is_file() {
                    let mut f = std::fs::File::open(ent.path()).map_err(
                        |e| {
                            XilinxError::RemoteTransfer(format!(
                                "open {}: {e}",
                                ent.path().display()
                            ))
                        },
                    )?;
                    builder.append_file(&arc, &mut f).map_err(|e| {
                        XilinxError::RemoteTransfer(format!(
                            "tar append file {arc}: {e}"
                        ))
                    })?;
                }
            }
        } else if p.is_file() {
            let mut f = std::fs::File::open(p).map_err(|e| {
                XilinxError::RemoteTransfer(format!(
                    "open {}: {e}",
                    p.display()
                ))
            })?;
            builder.append_file(rel, &mut f).map_err(|e| {
                XilinxError::RemoteTransfer(format!(
                    "tar append {rel}: {e}"
                ))
            })?;
        }
    }
    let gz = builder.into_inner().map_err(|e| {
        XilinxError::RemoteTransfer(format!("finish tar: {e}"))
    })?;
    let stdin_handle = gz.finish().map_err(|e| {
        XilinxError::RemoteTransfer(format!("finish gz: {e}"))
    })?;
    drop(stdin_handle);

    let out = ssh_child.wait_with_output().map_err(|e| {
        XilinxError::RemoteTransfer(format!("wait ssh upload: {e}"))
    })?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        return Err(XilinxError::RemoteTransfer(format!(
            "remote upload tar-extract failed (exit {}): {}",
            out.status.code().unwrap_or(-1),
            stderr.trim()
        )));
    }
    Ok(())
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
            if bytes[cursor..]
                .starts_with(ps.as_bytes())
            {
                out.push_str(&local_to_remote_path(p, session_dir));
                cursor += ps.len();
                continue 'outer;
            }
        }
        // Safe utf-8 step: append the current code point.
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

/// Generate a process-unique id for the per-invocation session dir.
/// Python uses `uuid.uuid4()`; a combination of pid + monotonic ns +
/// counter gives comparable uniqueness without adding a uuid crate.
fn unique_session_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let pid = std::process::id();
    let ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("tapa-{pid}-{ns}-{n}")
}

/// Download the contents of a remote directory into `dest` via
/// `ssh … tar -czf - -C <remote_dir> . | tar -xzf - -C <dest>`. SSH
/// stderr is captured so transient mux failures surface in the
/// returned error and the outer retry path can classify them.
fn download_tree(
    session: &SshSession,
    remote_dir: &str,
    dest: &Path,
) -> Result<()> {
    std::fs::create_dir_all(dest).map_err(|e| {
        XilinxError::RemoteTransfer(format!("mkdir {}: {e}", dest.display()))
    })?;
    let remote_cmd =
        format!("tar -czf - -C {} .", shell_quote(remote_dir));
    let mut args = session.build_ssh_args();
    args.push(session.ssh_target());
    args.push(remote_cmd);
    let mut ssh_child = Command::new("ssh")
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            XilinxError::RemoteTransfer(format!("spawn ssh for download: {e}"))
        })?;

    let ssh_out = ssh_child
        .stdout
        .take()
        .ok_or_else(|| XilinxError::RemoteTransfer("ssh stdout lost".into()))?;
    let ssh_err = ssh_child
        .stderr
        .take()
        .ok_or_else(|| XilinxError::RemoteTransfer("ssh stderr lost".into()))?;
    let mut tar_local = Command::new("tar")
        .arg("-xzf")
        .arg("-")
        .arg("-C")
        .arg(dest)
        .stdin(Stdio::from(ssh_out))
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            XilinxError::RemoteTransfer(format!(
                "spawn local tar -xz: {e}"
            ))
        })?;

    // Drain ssh stderr concurrently so a busy channel does not stall.
    let stderr_handle = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let mut r = ssh_err;
        let _ = std::io::Read::read_to_end(&mut r, &mut buf);
        buf
    });

    let tar_status = tar_local.wait().map_err(|e| {
        XilinxError::RemoteTransfer(format!("wait local tar -xz: {e}"))
    })?;
    let ssh_status = ssh_child.wait().map_err(|e| {
        XilinxError::RemoteTransfer(format!("wait ssh download: {e}"))
    })?;
    let stderr_bytes = stderr_handle.join().unwrap_or_default();
    let stderr = String::from_utf8_lossy(&stderr_bytes).into_owned();
    if !ssh_status.success() {
        return Err(XilinxError::RemoteTransfer(format!(
            "remote tar -cz failed (exit {}): {}",
            ssh_status.code().unwrap_or(-1),
            stderr.trim()
        )));
    }
    if !tar_status.success() {
        return Err(XilinxError::RemoteTransfer(format!(
            "local tar -xz failed: {tar_status}: {}",
            stderr.trim()
        )));
    }
    Ok(())
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
    /// counterpart. The rewrite pass keeps TCL scripts, CFLAGS,
    /// tool paths, etc. resolvable on the remote host without
    /// requiring the caller to know anything about the rootfs
    /// layout.
    fn run_once(&self, inv: &ToolInvocation) -> Result<ToolOutput> {
        self.session.ensure_established()?;
        let cfg = self.session.config();
        let session_dir = format!("{}/{}", cfg.work_dir, unique_session_id());

        // Collect absolute local paths referenced by this invocation.
        // The rewrite pass needs *all* of these, including download
        // targets, so an absolute-path string inside the TCL body
        // ends up pointing into the same rootfs on both ends.
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

        // Upload cwd + extras (the download set alone lives on the
        // remote side, not local).
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

        // Remote cwd: mirror of local cwd under rootfs.
        let remote_cwd = match inv.cwd.as_ref().filter(|p| p.is_absolute()) {
            Some(cwd) => local_to_remote_path(cwd, &session_dir),
            None => format!("{session_dir}/rootfs"),
        };

        // Build the remote bash command: source xilinx_settings if
        // configured, export allowlisted env (with paths rewritten),
        // then cd + exec the rewritten program + args.
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
                    XilinxError::RemoteTransfer(format!(
                        "write stdin: {e}"
                    ))
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

        // Download each requested path: its remote source is the
        // mirror under rootfs, and the local destination is the path
        // itself (pre-existing or created here).
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

/// Best-effort teardown of the per-invocation session directory. Run
/// after every attempt so the remote work dir doesn't accumulate
/// stale rootfs trees. Errors are ignored — cleanup failures must
/// not mask the real tool output or swallow a retry trigger.
fn cleanup_session(session: &SshSession, session_dir: &str) {
    let mut args = session.build_ssh_args();
    args.push(session.ssh_target());
    args.push(format!("rm -rf {}", shell_quote(session_dir)));
    let _ = Command::new("ssh")
        .args(&args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
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
        // Ports the Python behavior in `tapa/remote/popen.py`: when a
        // mux failure kills an in-flight invocation, tear the master
        // down, re-establish the control socket, and retry the command
        // once. Callers see a single attempted-and-recovered error
        // path, not `SshMuxLost` requiring an external retry loop.
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
        // The rootfs-based `run_once` already pulls every caller-
        // requested absolute-local download back into place, so the
        // explicit harvest step is a no-op on this runner. Kept for
        // interface symmetry and so tests that rely on the default
        // trait method still succeed.
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
        // First attempt returns a transient mux error, recovery
        // fires, second attempt succeeds.
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
        // A `RemoteTransfer` whose stderr classifies as
        // transient-mux also goes through the retry branch.
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
