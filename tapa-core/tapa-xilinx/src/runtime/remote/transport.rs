//! Low-level SSH transport helpers: shell quoting, tar-pipe
//! upload/download, per-invocation session ids, and cleanup.
//!
//! These are extracted from `remote/mod.rs` so the runner there can
//! focus on the invocation lifecycle (rootfs layout, path
//! rewriting, retry) without the batched-tar mechanics mixed in.

use std::path::Path;
use std::process::{Command, Stdio};

use camino::Utf8PathBuf;

use crate::error::{Result, XilinxError};
use crate::runtime::ssh::SshSession;

pub fn shell_quote(s: &str) -> String {
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
/// the implementation: absolute local
/// paths are pasted verbatim under `<session_dir>/rootfs/` after
/// stripping the leading `/`.
pub(super) fn local_to_remote_path(local: &Utf8PathBuf, session_dir: &str) -> String {
    let s = local.as_str();
    let rel = s.trim_start_matches('/');
    format!("{session_dir}/rootfs/{rel}")
}

/// Generate a process-unique id for the per-invocation session dir.
/// uses `uuid.uuid4()`; a combination of pid + monotonic ns +
/// counter gives comparable uniqueness without adding a uuid crate.
pub(super) fn unique_session_id() -> String {
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

/// Upload a batch of local absolute paths into `session_dir/rootfs`
/// via a single `tar | ssh tar -xzf -` session. Each path is added
/// to the archive preserving its absolute layout (minus the leading
/// `/`) so the remote tree mirrors the local one. Matches
/// the implementation.
#[allow(
    clippy::too_many_lines,
    reason = "streams the in-memory tar archive inline for one SSH session; \
              splitting into helpers would obscure the batched-upload flow"
)]
pub(super) fn upload_batch(
    session: &SshSession,
    session_dir: &str,
    local_paths: &[Utf8PathBuf],
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
        .map_err(|e| XilinxError::RemoteTransfer(format!("spawn ssh for upload: {e}")))?;

    let ssh_in = ssh_child
        .stdin
        .take()
        .ok_or_else(|| XilinxError::RemoteTransfer("ssh stdin lost".into()))?;
    let gz = flate2::write::GzEncoder::new(ssh_in, flate2::Compression::fast());
    let mut builder = tar::Builder::new(gz);
    builder.follow_symlinks(true);
    for p in local_paths {
        if !p.exists() {
            continue;
        }
        let rel = p.as_str();
        let rel = rel.trim_start_matches('/');
        if p.is_dir() {
            builder
                .append_dir(rel, p.as_std_path())
                .map_err(|e| XilinxError::RemoteTransfer(format!("tar append dir {rel}: {e}")))?;
            for ent in std::fs::read_dir(p).map_err(|e| {
                XilinxError::RemoteTransfer(format!("read_dir {}: {e}", p))
            })? {
                let ent =
                    ent.map_err(|e| XilinxError::RemoteTransfer(format!("read_dir entry: {e}")))?;
                let arc = format!("{rel}/{}", ent.file_name().to_string_lossy());
                let ent_path = ent.path();
                let metadata = std::fs::metadata(&ent_path).map_err(|e| {
                    XilinxError::RemoteTransfer(format!("metadata {}: {e}", ent_path.display()))
                })?;
                if metadata.is_dir() {
                    builder.append_dir_all(&arc, &ent_path).map_err(|e| {
                        XilinxError::RemoteTransfer(format!("tar append dir {arc}: {e}"))
                    })?;
                } else if metadata.is_file() {
                    let mut f = std::fs::File::open(&ent_path).map_err(|e| {
                        XilinxError::RemoteTransfer(format!("open {}: {e}", ent_path.display()))
                    })?;
                    builder.append_file(&arc, &mut f).map_err(|e| {
                        XilinxError::RemoteTransfer(format!("tar append file {arc}: {e}"))
                    })?;
                }
            }
        } else if p.is_file() {
            let mut f = std::fs::File::open(p.as_std_path())
                .map_err(|e| XilinxError::RemoteTransfer(format!("open {}: {e}", p)))?;
            builder
                .append_file(rel, &mut f)
                .map_err(|e| XilinxError::RemoteTransfer(format!("tar append {rel}: {e}")))?;
        }
    }
    let gz = builder
        .into_inner()
        .map_err(|e| XilinxError::RemoteTransfer(format!("finish tar: {e}")))?;
    let stdin_handle = gz
        .finish()
        .map_err(|e| XilinxError::RemoteTransfer(format!("finish gz: {e}")))?;
    drop(stdin_handle);

    let out = ssh_child
        .wait_with_output()
        .map_err(|e| XilinxError::RemoteTransfer(format!("wait ssh upload: {e}")))?;
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

/// Download the contents of a remote directory into `dest` via
/// `ssh … tar -czf - -C <remote_dir> .` decoded in-process with
/// `flate2`/`tar`. SSH stderr is captured so transient mux failures
/// surface in the returned error and the outer retry path can classify
/// them.
pub(super) fn download_tree(session: &SshSession, remote_dir: &str, dest: &Path) -> Result<()> {
    std::fs::create_dir_all(dest)
        .map_err(|e| XilinxError::RemoteTransfer(format!("mkdir {}: {e}", dest.display())))?;
    let remote_cmd = format!("tar -czf - -C {} .", shell_quote(remote_dir));
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
    let mut ssh_err = ssh_child
        .stderr
        .take()
        .ok_or_else(|| XilinxError::RemoteTransfer("ssh stderr lost".into()))?;

    let stderr_handle = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = std::io::Read::read_to_end(&mut ssh_err, &mut buf);
        buf
    });

    let unpack_result = {
        let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(ssh_out));
        archive.unpack(dest)
    };

    let ssh_status = ssh_child
        .wait()
        .map_err(|e| XilinxError::RemoteTransfer(format!("wait ssh download: {e}")))?;
    let stderr_bytes = stderr_handle.join().unwrap_or_default();
    let stderr = String::from_utf8_lossy(&stderr_bytes).into_owned();
    if !ssh_status.success() {
        return Err(XilinxError::RemoteTransfer(format!(
            "remote tar -cz failed (exit {}): {}",
            ssh_status.code().unwrap_or(-1),
            stderr.trim()
        )));
    }
    unpack_result.map_err(|e| XilinxError::RemoteTransfer(format!("unpack tar download: {e}")))?;
    Ok(())
}

/// Best-effort teardown of the per-invocation session directory. Run
/// after every attempt so the remote work dir doesn't accumulate
/// stale rootfs trees. Errors are ignored — cleanup failures must
/// not mask the real tool output or swallow a retry trigger.
pub(super) fn cleanup_session(session: &SshSession, session_dir: &str) {
    let mut args = session.build_ssh_args();
    args.push(session.ssh_target());
    args.push(format!("rm -rf {}", shell_quote(session_dir)));
    let _ = Command::new("ssh")
        .args(&args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_quote_empty_string() {
        assert_eq!(shell_quote(""), "''");
    }

    #[test]
    fn shell_quote_simple_word() {
        assert_eq!(shell_quote("hello"), "'hello'");
    }

    #[test]
    fn shell_quote_preserves_spaces() {
        assert_eq!(shell_quote("hello world"), "'hello world'");
    }

    #[test]
    fn shell_quote_escapes_single_quotes() {
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
    }

    #[test]
    fn shell_quote_multiple_single_quotes() {
        assert_eq!(shell_quote("a'b'c"), "'a'\\''b'\\''c'");
    }

    #[test]
    fn local_to_remote_path_strips_leading_slash() {
        assert_eq!(
            local_to_remote_path(&Utf8PathBuf::from("/home/alice/project"), "/tmp/session"),
            "/tmp/session/rootfs/home/alice/project"
        );
    }

    #[test]
    fn local_to_remote_path_preserves_relative() {
        assert_eq!(
            local_to_remote_path(&Utf8PathBuf::from("relative/path"), "/tmp/session"),
            "/tmp/session/rootfs/relative/path"
        );
    }

    #[test]
    fn unique_session_ids_are_unique() {
        let a = unique_session_id();
        let b = unique_session_id();
        assert_ne!(a, b);
        assert!(a.starts_with("tapa-"));
        assert!(b.starts_with("tapa-"));
    }
}
