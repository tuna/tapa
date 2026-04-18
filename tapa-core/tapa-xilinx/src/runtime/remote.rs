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

impl ToolRunner for RemoteToolRunner {
    fn run(&self, inv: &ToolInvocation) -> Result<ToolOutput> {
        // Ports the Python behavior in `tapa/remote/popen.py`: when a
        // mux failure kills an in-flight invocation, tear the master
        // down, re-establish the control socket, and retry the command
        // once. Callers see a single attempted-and-recovered error
        // path, not `SshMuxLost` requiring an external retry loop.
        match self.run_once(inv) {
            Ok(out) => Ok(out),
            Err(err)
                if matches!(
                    err,
                    XilinxError::SshMuxLost { .. }
                        | XilinxError::RemoteTransfer(_)
                ) && is_recoverable_mux_error(&err) =>
            {
                self.session.reset_mux();
                self.session.ensure_established()?;
                self.run_once(inv)
            }
            Err(e) => Err(e),
        }
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

/// Abstraction over the two remote operations
/// [`sync_remote_vendor_includes`] needs: running a shell command over
/// SSH and capturing stdout/stderr/exit-code, and streaming a remote
/// directory's contents into a local destination via tar-pipe. Exposed
/// as a trait so unit tests can drive the algorithm without a live
/// `SshSession`.
pub trait VendorRemoteFs {
    /// Run `cmd` on the remote in a login-style shell. Returns
    /// `(exit_code, stdout_bytes, stderr_bytes)`.
    fn ssh_exec(&self, cmd: &str) -> Result<(i32, Vec<u8>, Vec<u8>)>;

    /// Stream the remote directory at `remote_path` into the local
    /// directory `local_dest` (created if missing). Equivalent to
    /// `ssh … tar -czf - -C remote_path . | tar -xzf - -C local_dest`.
    fn download_dir(&self, remote_path: &str, local_dest: &Path) -> Result<()>;
}

struct SshVendorFs<'a> {
    session: &'a SshSession,
}

impl VendorRemoteFs for SshVendorFs<'_> {
    fn ssh_exec(&self, cmd: &str) -> Result<(i32, Vec<u8>, Vec<u8>)> {
        let mut args = self.session.build_ssh_args();
        args.push(self.session.ssh_target());
        args.push(cmd.to_string());
        let out = Command::new("ssh").args(&args).output().map_err(|e| {
            XilinxError::RemoteTransfer(format!("spawn ssh exec: {e}"))
        })?;
        Ok((out.status.code().unwrap_or(-1), out.stdout, out.stderr))
    }

    fn download_dir(&self, remote_path: &str, local_dest: &Path) -> Result<()> {
        std::fs::create_dir_all(local_dest).map_err(|e| {
            XilinxError::RemoteTransfer(format!(
                "mkdir {}: {e}",
                local_dest.display()
            ))
        })?;
        let remote_cmd =
            format!("tar -czf - -C {} .", shell_quote(remote_path));
        let mut args = self.session.build_ssh_args();
        args.push(self.session.ssh_target());
        args.push(remote_cmd);
        let mut ssh = Command::new("ssh")
            .args(&args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| {
                XilinxError::RemoteTransfer(format!("spawn ssh download: {e}"))
            })?;
        let ssh_stdout = ssh
            .stdout
            .take()
            .ok_or_else(|| XilinxError::RemoteTransfer("ssh stdout lost".into()))?;
        let mut tar_local = Command::new("tar")
            .arg("-xzf")
            .arg("-")
            .arg("-C")
            .arg(local_dest)
            .stdin(Stdio::from(ssh_stdout))
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| {
                XilinxError::RemoteTransfer(format!(
                    "spawn local tar -xz: {e}"
                ))
            })?;
        let tar_status = tar_local.wait().map_err(|e| {
            XilinxError::RemoteTransfer(format!("wait tar -xz: {e}"))
        })?;
        let ssh_status = ssh.wait().map_err(|e| {
            XilinxError::RemoteTransfer(format!("wait ssh download: {e}"))
        })?;
        if !ssh_status.success() {
            return Err(XilinxError::RemoteTransfer(format!(
                "remote tar -cz failed: {ssh_status}"
            )));
        }
        if !tar_status.success() {
            return Err(XilinxError::RemoteTransfer(format!(
                "local tar -xz failed: {tar_status}"
            )));
        }
        Ok(())
    }
}

/// Parse the `KEY=VAL` lines produced by the remote
/// `echo XILINX_HLS=$XILINX_HLS && echo XILINX_VITIS=$XILINX_VITIS`
/// probe. Empty values are dropped (matches the Python loader).
pub(crate) fn parse_remote_xilinx_paths(
    stdout: &str,
) -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    for line in stdout.lines() {
        if let Some((k, v)) = line.split_once('=') {
            let v = v.trim();
            if !v.is_empty() {
                out.insert(k.trim().to_string(), v.to_string());
            }
        }
    }
    out
}

/// Compute the deterministic cache directory under
/// `$XDG_CACHE_HOME/tapa/vendor-headers/<key>` where `<key>` is the
/// first 16 hex chars of `sha256(host:port:xilinx_settings)` (matches
/// `tapa/remote/vendor.py::_cache_key`).
pub(crate) fn vendor_cache_dir(
    host: &str,
    port: u16,
    xilinx_settings: &str,
) -> Result<PathBuf> {
    use sha2::{Digest, Sha256};
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .ok_or_else(|| {
            XilinxError::RemoteTransfer(
                "vendor cache dir: neither XDG_CACHE_HOME nor HOME is set".into(),
            )
        })?;
    let raw = format!("{host}:{port}:{xilinx_settings}");
    let hash = Sha256::digest(raw.as_bytes());
    let mut key = String::with_capacity(16);
    for b in &hash[..8] {
        use std::fmt::Write as _;
        let _ = write!(key, "{b:02x}");
    }
    Ok(base.join("tapa").join("vendor-headers").join(key))
}

/// Apply the macOS libc++ compatibility patch to
/// `<cache_dir>/include/etc/ap_*_special.h`. Replaces the forward-
/// declaration block (see `tapa/remote/vendor.py::_patch_vendor_headers_for_macos`)
/// with `#include <complex>`. Idempotent: writes a marker
/// `.patched_macos_complex` to skip on subsequent calls. On non-macOS
/// hosts this is a no-op (matches Python's `platform.system() != "Darwin"`
/// short-circuit).
pub(crate) fn apply_macos_vendor_patch(cache_dir: &Path) -> Result<()> {
    if !cfg!(target_os = "macos") {
        return Ok(());
    }
    let marker = cache_dir.join(".patched_macos_complex");
    if marker.is_file() {
        return Ok(());
    }
    let etc_dir = cache_dir.join("include").join("etc");
    if !etc_dir.is_dir() {
        return Ok(());
    }
    // Pattern: forward-decl block in ap_*_special.h. Literal copy from
    // `tapa/remote/vendor.py::_patch_vendor_headers_for_macos`.
    #[allow(
        clippy::trivial_regex,
        reason = "the Python patch uses a literal multi-line match; keep the \
                  pattern structure identical for source-of-truth parity even \
                  though Rust could do str::contains here"
    )]
    let pattern = regex::Regex::new(concat!(
        r"// FIXME AP_AUTOCC cannot handle many standard headers, so declare instead of\n",
        r"// include\.\n",
        r"// #include <complex>\n",
        r"namespace std \{\n",
        r"template<typename _Tp> class complex;\n",
        r"\}",
    ))
    .expect("static macOS patch pattern must compile");
    let replacement = "#include <complex>";
    let mut any = false;
    for entry in std::fs::read_dir(&etc_dir).map_err(|e| {
        XilinxError::RemoteTransfer(format!("read_dir {}: {e}", etc_dir.display()))
    })? {
        let entry = entry.map_err(|e| {
            XilinxError::RemoteTransfer(format!("read_dir entry: {e}"))
        })?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        if !(name.starts_with("ap_") && name.ends_with("_special.h")) {
            continue;
        }
        let content = std::fs::read_to_string(&path).map_err(|e| {
            XilinxError::RemoteTransfer(format!("read {}: {e}", path.display()))
        })?;
        let new_content = pattern.replace(&content, replacement);
        if new_content != content {
            std::fs::write(&path, new_content.as_bytes()).map_err(|e| {
                XilinxError::RemoteTransfer(format!("write {}: {e}", path.display()))
            })?;
            any = true;
        }
    }
    if any {
        std::fs::write(&marker, b"patched\n").map_err(|e| {
            XilinxError::RemoteTransfer(format!(
                "write macOS patch marker: {e}"
            ))
        })?;
    }
    Ok(())
}

/// One-shot vendor header sync from the configured remote.
///
/// Ports `tapa/remote/vendor.py::sync_remote_vendor_includes`:
///
/// 1. Source the remote `xilinx_settings` script and read back
///    `XILINX_HLS` / `XILINX_VITIS`.
/// 2. Stream `$XILINX_HLS/include` into
///    `<cache_dir>/include` via tar-pipe.
/// 3. Glob `$XILINX_HLS/tps/lnx64/gcc-*/include` on the remote and
///    mirror each one under `<cache_dir>/tps/lnx64/gcc-*/include`.
/// 4. On macOS hosts, patch the `ap_*_special.h` headers so libc++
///    (std::__1::complex) resolves cleanly.
///
/// The cache directory is keyed by
/// `sha256(host:port:xilinx_settings)[:16]` so distinct remote
/// toolchains don't collide. Writing a `.synced` marker makes the
/// function idempotent — a second call with the same config is a
/// no-op aside from re-running the macOS patch pass (itself guarded
/// by its own marker).
pub fn sync_remote_vendor_includes(session: &SshSession) -> Result<PathBuf> {
    let cfg = session.config();
    let xilinx_settings = cfg.xilinx_settings.clone().ok_or_else(|| {
        XilinxError::RemoteTransfer(
            "sync_remote_vendor_includes: remote xilinx_settings unset".into(),
        )
    })?;
    session.ensure_established()?;
    let fs = SshVendorFs { session };
    let cache_dir = vendor_cache_dir(&cfg.host, cfg.port, &xilinx_settings)?;
    sync_vendor_includes_impl(&fs, &xilinx_settings, &cache_dir)
}

/// Pure algorithm driving the vendor include sync, parameterized over a
/// [`VendorRemoteFs`] and an explicit cache root so unit tests can
/// exercise every branch (probe success/failure, tar-pipe download,
/// macOS patch, idempotency) without a live SSH session and without
/// racing on the process-wide `XDG_CACHE_HOME` env var.
pub(crate) fn sync_vendor_includes_impl<F: VendorRemoteFs>(
    fs: &F,
    xilinx_settings: &str,
    cache_dir: &Path,
) -> Result<PathBuf> {
    let cache_dir = cache_dir.to_path_buf();
    let marker = cache_dir.join(".synced");
    if marker.is_file() {
        apply_macos_vendor_patch(&cache_dir)?;
        return Ok(cache_dir);
    }

    // Probe remote XILINX_HLS / XILINX_VITIS.
    let probe = format!(
        "source {s} && echo XILINX_HLS=$XILINX_HLS && echo XILINX_VITIS=$XILINX_VITIS",
        s = shell_quote(xilinx_settings),
    );
    let (rc, stdout, stderr) = fs.ssh_exec(&probe)?;
    if rc != 0 {
        return Err(XilinxError::RemoteTransfer(format!(
            "probe xilinx_settings: exit {rc}: {}",
            String::from_utf8_lossy(&stderr).trim()
        )));
    }
    let paths = parse_remote_xilinx_paths(&String::from_utf8_lossy(&stdout));
    let xilinx_tool = paths
        .get("XILINX_HLS")
        .or_else(|| paths.get("XILINX_VITIS"))
        .cloned()
        .ok_or_else(|| {
            XilinxError::RemoteTransfer(
                "remote XILINX_HLS / XILINX_VITIS not set after sourcing xilinx_settings".into(),
            )
        })?;

    std::fs::create_dir_all(&cache_dir).map_err(|e| {
        XilinxError::RemoteTransfer(format!(
            "mkdir cache {}: {e}",
            cache_dir.display()
        ))
    })?;

    // Remove any stale macOS patch marker so the patch re-applies
    // after a fresh header download.
    let patch_marker = cache_dir.join(".patched_macos_complex");
    if patch_marker.exists() {
        std::fs::remove_file(&patch_marker).map_err(|e| {
            XilinxError::RemoteTransfer(format!(
                "remove stale patch marker: {e}"
            ))
        })?;
    }

    // Download include/.
    let remote_include = format!("{xilinx_tool}/include");
    let local_include = cache_dir.join("include");
    fs.download_dir(&remote_include, &local_include)?;

    // Glob remote tps/lnx64/gcc-*/include directories and mirror each
    // one under the same relative path locally.
    let ls_cmd = format!(
        "ls -d {xt}/tps/lnx64/gcc-*/include 2>/dev/null || true",
        xt = shell_quote(&xilinx_tool),
    );
    let (_, ls_out, _) = fs.ssh_exec(&ls_cmd)?;
    for remote_gcc_inc in String::from_utf8_lossy(&ls_out)
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let rel = remote_gcc_inc
            .strip_prefix(&format!("{xilinx_tool}/"))
            .unwrap_or(remote_gcc_inc)
            .to_string();
        let local_gcc = cache_dir.join(&rel);
        fs.download_dir(remote_gcc_inc, &local_gcc)?;
    }

    apply_macos_vendor_patch(&cache_dir)?;
    std::fs::write(&marker, format!("{xilinx_tool}\n")).map_err(|e| {
        XilinxError::RemoteTransfer(format!("write synced marker: {e}"))
    })?;
    Ok(cache_dir)
}

#[cfg(test)]
mod vendor_tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::VecDeque;

    /// Mock implementation driving the algorithm through canned
    /// responses. `ssh_exec_responses` is consumed FIFO; each call to
    /// `ssh_exec` pops one. `download_dir` writes a synthetic file
    /// tree into the destination so downstream logic (the macOS
    /// patch, the marker write) exercises real filesystem paths.
    type SshCannedResponse = (i32, Vec<u8>, Vec<u8>);

    struct MockFs {
        ssh_exec_responses: RefCell<VecDeque<SshCannedResponse>>,
        download_fail_on: Option<String>,
        recorded_downloads: RefCell<Vec<String>>,
        write_ap_special: bool,
    }

    impl MockFs {
        fn new(responses: Vec<SshCannedResponse>) -> Self {
            Self {
                ssh_exec_responses: RefCell::new(responses.into()),
                download_fail_on: None,
                recorded_downloads: RefCell::new(Vec::new()),
                write_ap_special: false,
            }
        }
    }

    impl VendorRemoteFs for MockFs {
        fn ssh_exec(&self, _cmd: &str) -> Result<(i32, Vec<u8>, Vec<u8>)> {
            self.ssh_exec_responses
                .borrow_mut()
                .pop_front()
                .ok_or_else(|| {
                    XilinxError::RemoteTransfer(
                        "MockFs: no more canned ssh responses".into(),
                    )
                })
        }
        fn download_dir(&self, remote_path: &str, local_dest: &Path) -> Result<()> {
            self.recorded_downloads
                .borrow_mut()
                .push(remote_path.to_string());
            if self
                .download_fail_on
                .as_deref()
                .is_some_and(|frag| remote_path.contains(frag))
            {
                return Err(XilinxError::RemoteTransfer(format!(
                    "mock tar-pipe failed for {remote_path}"
                )));
            }
            std::fs::create_dir_all(local_dest).map_err(|e| {
                XilinxError::RemoteTransfer(format!(
                    "mock mkdir {}: {e}",
                    local_dest.display()
                ))
            })?;
            // Touch a sentinel file so the caller can see the dir exists.
            std::fs::write(local_dest.join(".mock_download"), remote_path)
                .map_err(|e| XilinxError::RemoteTransfer(format!("mock write: {e}")))?;
            if self.write_ap_special && local_dest.ends_with("include") {
                let etc = local_dest.join("etc");
                std::fs::create_dir_all(&etc).unwrap();
                let body = concat!(
                    "// FIXME AP_AUTOCC cannot handle many standard headers, so declare instead of\n",
                    "// include.\n",
                    "// #include <complex>\n",
                    "namespace std {\n",
                    "template<typename _Tp> class complex;\n",
                    "}\n",
                    "struct rest_of_header {};\n",
                );
                std::fs::write(etc.join("ap_fixed_special.h"), body).unwrap();
            }
            Ok(())
        }
    }

    #[test]
    fn parse_remote_xilinx_paths_handles_mixed_lines() {
        let text = "XILINX_HLS=/opt/xilinx/hls\nXILINX_VITIS=\nnoise";
        let m = parse_remote_xilinx_paths(text);
        assert_eq!(m.get("XILINX_HLS").unwrap(), "/opt/xilinx/hls");
        assert!(!m.contains_key("XILINX_VITIS"));
    }

    fn isolate_cache() -> (tempfile::TempDir, PathBuf) {
        let td = tempfile::tempdir().expect("tempdir");
        let key = td.path().join("tapa").join("vendor-headers").join("k");
        (td, key)
    }

    #[test]
    fn cache_dir_is_deterministic_and_keyed() {
        // Set XDG_CACHE_HOME via a temp dir for this one lookup. Env
        // races are avoided because we only read the env — no persistent
        // state in the cache dir path computation. Use a serialization
        // lock so concurrent tests don't overwrite each other's env.
        static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _g = ENV_LOCK.lock().unwrap();
        let td = tempfile::tempdir().expect("tempdir");
        let prev = std::env::var_os("XDG_CACHE_HOME");
        std::env::set_var("XDG_CACHE_HOME", td.path());
        let a = vendor_cache_dir("h1", 22, "/opt/settings64.sh").unwrap();
        let b = vendor_cache_dir("h1", 22, "/opt/settings64.sh").unwrap();
        let c = vendor_cache_dir("h2", 22, "/opt/settings64.sh").unwrap();
        if let Some(p) = prev {
            std::env::set_var("XDG_CACHE_HOME", p);
        } else {
            std::env::remove_var("XDG_CACHE_HOME");
        }
        drop(td);
        assert_eq!(a, b);
        assert_ne!(a, c);
        let key = a.file_name().unwrap().to_string_lossy();
        assert_eq!(key.len(), 16);
        assert!(key.chars().all(|ch| ch.is_ascii_hexdigit()));
    }

    #[test]
    fn happy_path_downloads_include_and_gcc_dirs() {
        let (_td, cache) = isolate_cache();
        let mock = MockFs::new(vec![
            (0, b"XILINX_HLS=/opt/xilinx/hls\nXILINX_VITIS=\n".to_vec(), Vec::new()),
            (0, b"/opt/xilinx/hls/tps/lnx64/gcc-6.2.0/include\n".to_vec(), Vec::new()),
        ]);
        let out = sync_vendor_includes_impl(&mock, "/opt/settings64.sh", &cache)
            .expect("sync must succeed");
        assert_eq!(out, cache);
        assert!(cache.join("include").join(".mock_download").is_file());
        assert!(cache
            .join("tps/lnx64/gcc-6.2.0/include")
            .join(".mock_download")
            .is_file());
        assert!(cache.join(".synced").is_file());
        let dls = mock.recorded_downloads.borrow().clone();
        assert_eq!(
            dls,
            vec![
                "/opt/xilinx/hls/include".to_string(),
                "/opt/xilinx/hls/tps/lnx64/gcc-6.2.0/include".to_string(),
            ]
        );
    }

    #[test]
    fn missing_remote_xilinx_paths_surfaces_typed_error() {
        let (_td, cache) = isolate_cache();
        let mock = MockFs::new(vec![(
            0,
            b"XILINX_HLS=\nXILINX_VITIS=\n".to_vec(),
            Vec::new(),
        )]);
        let err = sync_vendor_includes_impl(&mock, "/opt/settings64.sh", &cache)
            .expect_err("must error when no tool paths");
        match err {
            XilinxError::RemoteTransfer(msg) => assert!(msg.contains("XILINX_HLS")),
            other => panic!("wrong variant: {other:?}"),
        }
        assert!(!cache.join(".synced").exists());
    }

    #[test]
    fn probe_nonzero_exit_surfaces_typed_error() {
        let (_td, cache) = isolate_cache();
        let mock = MockFs::new(vec![(
            127,
            Vec::new(),
            b"settings64.sh: not found".to_vec(),
        )]);
        let err = sync_vendor_includes_impl(&mock, "/opt/settings64.sh", &cache)
            .expect_err("probe nonzero must error");
        match err {
            XilinxError::RemoteTransfer(msg) => {
                assert!(msg.contains("probe"));
                assert!(msg.contains("127"));
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn tar_pipe_failure_on_include_surfaces_typed_error() {
        let (_td, cache) = isolate_cache();
        let mut mock = MockFs::new(vec![(
            0,
            b"XILINX_HLS=/opt/xilinx/hls\n".to_vec(),
            Vec::new(),
        )]);
        mock.download_fail_on = Some("/include".to_string());
        let err = sync_vendor_includes_impl(&mock, "/opt/settings64.sh", &cache)
            .expect_err("download failure must error");
        assert!(matches!(err, XilinxError::RemoteTransfer(_)));
    }

    #[test]
    fn idempotent_second_call_skips_runner() {
        let (_td, cache) = isolate_cache();
        let mock = MockFs::new(vec![
            (0, b"XILINX_HLS=/opt/xilinx/hls\n".to_vec(), Vec::new()),
            (0, b"".to_vec(), Vec::new()),
        ]);
        let cache1 = sync_vendor_includes_impl(&mock, "/opt/settings64.sh", &cache).unwrap();
        let remaining_before = mock.ssh_exec_responses.borrow().len();
        let cache2 = sync_vendor_includes_impl(&mock, "/opt/settings64.sh", &cache).unwrap();
        assert_eq!(cache1, cache2);
        assert_eq!(remaining_before, mock.ssh_exec_responses.borrow().len());
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn macos_patch_rewrites_ap_special_headers() {
        let (_td, cache) = isolate_cache();
        let mut mock = MockFs::new(vec![
            (0, b"XILINX_HLS=/opt/xilinx/hls\n".to_vec(), Vec::new()),
            (0, b"".to_vec(), Vec::new()),
        ]);
        mock.write_ap_special = true;
        let out =
            sync_vendor_includes_impl(&mock, "/opt/settings64.sh", &cache).unwrap();
        let patched = std::fs::read_to_string(
            out.join("include").join("etc").join("ap_fixed_special.h"),
        )
        .expect("header");
        assert!(patched.contains("#include <complex>"));
        assert!(!patched.contains("template<typename _Tp> class complex"));
        assert!(out.join(".patched_macos_complex").is_file());
    }
}
