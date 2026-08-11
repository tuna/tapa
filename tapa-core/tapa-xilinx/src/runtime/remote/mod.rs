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
use std::process::{Command, Stdio};
use std::sync::Arc;

use camino::Utf8PathBuf;

use self::transport::{cleanup_session, local_to_remote_path, unique_session_id, upload_batch};
pub(crate) use self::transport::{download_tree, shell_quote};
use crate::error::{Result, XilinxError};
use crate::runtime::process::{unified_hls_args, ToolInvocation, ToolOutput, ToolRunner};
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
        self.session.exec_cmd(remote_cmd)
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

/// Rewrite every occurrence of a local absolute path in `text` to its
/// session-scoped remote equivalent. Longest-match-first ensures a
/// path that is a prefix of another (e.g. `/a/b` vs `/a/b/c`) is not
/// double-replaced.
fn rewrite_abs_paths(text: &str, local_paths: &[Utf8PathBuf], session_dir: &str) -> String {
    if local_paths.is_empty() {
        return text.to_string();
    }
    let mut sorted: Vec<&Utf8PathBuf> = local_paths.iter().collect();
    sorted.sort_by_key(|p| std::cmp::Reverse(p.as_str().len()));
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0usize;
    let bytes = text.as_bytes();
    'outer: while cursor < bytes.len() {
        for p in &sorted {
            let ps = p.as_str();
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

/// Environment variables forwarded to the remote. Anything else is
/// dropped unless the key begins with `TAPA_`.
const REMOTE_ENV_ALLOWLIST: &[&str] = &["HOME", "LANG", "LC_ALL", "LC_CTYPE"];

fn is_forwardable_env(key: &str) -> bool {
    REMOTE_ENV_ALLOWLIST.contains(&key) || key.starts_with("TAPA_")
}

/// Everything [`RemoteToolRunner::run_once`] stages before the
/// transport is touched: the freshly minted session directory, the
/// deduped set of local paths the invocation references (the input to
/// path rewriting), the existing paths worth uploading, and the
/// absolutized cwd/downloads that mirror `ToolInvocation` positionally
/// so the collect stage can map them back to the caller-facing paths.
struct UploadPlan {
    session_dir: String,
    referenced: Vec<Utf8PathBuf>,
    to_upload: Vec<Utf8PathBuf>,
    cwd_abs: Option<Utf8PathBuf>,
    downloads_abs: Vec<Utf8PathBuf>,
}

/// Second stage of [`RemoteToolRunner::run_once`]: assemble the remote
/// shell script for a planned invocation -- mkdir each download
/// target, source the Xilinx settings script when configured, export
/// the forwardable env entries with absolute local paths rewritten to
/// their session-scoped counterparts, then `cd` into the remote cwd
/// and `exec` the rewritten command line. Kept pure (no session /
/// transport access) so unit tests can pin the generated text without
/// ssh.
fn build_remote_script(
    plan: &UploadPlan,
    inv: &ToolInvocation,
    xilinx_settings: Option<&str>,
) -> String {
    let remote_cwd = match plan.cwd_abs.as_ref() {
        Some(cwd) => local_to_remote_path(cwd, &plan.session_dir),
        None => format!("{}/rootfs", plan.session_dir),
    };

    let mut parts: Vec<String> = Vec::new();
    for dl in &plan.downloads_abs {
        let remote_dl = local_to_remote_path(dl, &plan.session_dir);
        parts.push(format!("mkdir -p {}", shell_quote(&remote_dl)));
    }
    if let Some(xs) = xilinx_settings {
        if !xs.trim().is_empty() {
            parts.push(format!("source {}", shell_quote(xs)));
        }
    }
    for (k, v) in &inv.env {
        if !is_forwardable_env(k) {
            continue;
        }
        let rv = rewrite_abs_paths(v, &plan.referenced, &plan.session_dir);
        parts.push(format!("export {}={}", k, shell_quote(&rv)));
    }
    let rewritten_args: Vec<String> = inv
        .args
        .iter()
        .map(|a| rewrite_abs_paths(a, &plan.referenced, &plan.session_dir))
        .collect();
    let exec = std::iter::once(shell_quote(&inv.program))
        .chain(rewritten_args.iter().map(|a| shell_quote(a)))
        .collect::<Vec<_>>()
        .join(" ");
    if inv.program == "vitis_hls" {
        // Vitis 2025.1+ removed the classic `vitis_hls` executable in
        // favor of the unified `vitis-run` CLI. Which form applies is a
        // property of the remote installation, so decide in the remote
        // shell (after its settings script populated PATH).
        let unified_exec = std::iter::once(shell_quote("vitis-run"))
            .chain(
                unified_hls_args(&rewritten_args)
                    .iter()
                    .map(|a| shell_quote(a)),
            )
            .collect::<Vec<_>>()
            .join(" ");
        parts.push(format!(
            "cd {} && if ! command -v vitis_hls >/dev/null 2>&1 \
                && command -v vitis-run >/dev/null 2>&1; \
                then exec {}; else exec {}; fi",
            shell_quote(&remote_cwd),
            unified_exec,
            exec
        ));
    } else {
        parts.push(format!("cd {} && exec {}", shell_quote(&remote_cwd), exec));
    }
    let full_cmd = parts.join(" ; ");
    format!("bash -c {}", shell_quote(&full_cmd))
}

impl RemoteToolRunner {
    /// Opens a per-invocation session directory with a `rootfs/`
    /// subtree, mirrors the local `cwd` plus any extra uploads under
    /// that rootfs, rewrites absolute local paths in the command
    /// args / env / stdin to their session-relative remote
    /// equivalents, executes the command with the remote working
    /// directory pointed at the rewritten cwd, and then tar-pipes
    /// each requested download path back from its rootfs
    /// counterpart.
    fn run_once(&self, inv: &ToolInvocation) -> Result<ToolOutput> {
        self.session.ensure_established()?;
        let plan = self.prepare_upload(inv);
        upload_batch(&self.session, &plan.session_dir, &plan.to_upload)?;
        let wrapped =
            build_remote_script(&plan, inv, self.session.config().xilinx_settings.as_deref());
        self.exec_and_collect(inv, &plan, &wrapped)
    }

    /// First stage of [`run_once`](Self::run_once): mint the session
    /// directory, then plan the upload -- absolutize the invocation's
    /// cwd / upload / download paths against the caller's working
    /// directory, dedup them into the referenced set used for path
    /// rewriting, and pick the existing paths for the transport to
    /// upload.
    fn prepare_upload(&self, inv: &ToolInvocation) -> UploadPlan {
        let session_dir = format!("{}/{}", self.session.config().work_dir, unique_session_id());

        // Accept relative `--work-dir ./work.out` and
        // relative upload/download paths by absolutizing against the
        // caller's cwd. Without this, the default `tapa synth` / `pack`
        // invocation drops the work tree + RTL + C++ sources from the
        // upload batch, leaving the remote Vitis HLS with nothing to
        // compile.
        let absolutize = |p: &Utf8PathBuf| crate::util::absolutize(p);
        let cwd_abs: Option<Utf8PathBuf> = inv.cwd.as_ref().map(absolutize);
        let uploads_abs: Vec<Utf8PathBuf> = inv.uploads.iter().map(absolutize).collect();
        let downloads_abs: Vec<Utf8PathBuf> = inv.downloads.iter().map(absolutize).collect();

        let mut referenced: Vec<Utf8PathBuf> = Vec::new();
        if let Some(cwd) = cwd_abs.as_ref() {
            referenced.push(cwd.clone());
        }
        referenced.extend(uploads_abs.iter().cloned());
        referenced.extend(downloads_abs.iter().cloned());
        let mut seen: std::collections::HashSet<Utf8PathBuf> = std::collections::HashSet::new();
        referenced.retain(|p| seen.insert(p.clone()));

        let mut to_upload: Vec<Utf8PathBuf> = Vec::new();
        if let Some(cwd) = cwd_abs.as_ref() {
            if cwd.exists() {
                to_upload.push(cwd.clone());
            }
        }
        for p in &uploads_abs {
            if p.exists() {
                to_upload.push(p.clone());
            }
        }
        let mut seen2: std::collections::HashSet<Utf8PathBuf> = std::collections::HashSet::new();
        to_upload.retain(|p| seen2.insert(p.clone()));

        UploadPlan {
            session_dir,
            referenced,
            to_upload,
            cwd_abs,
            downloads_abs,
        }
    }

    /// Final stage of [`run_once`](Self::run_once): execute the
    /// wrapped remote script through the transport and collect the
    /// results -- stream `inv.stdin` when present, tear the session
    /// down before surfacing a transient mux failure as a classified
    /// error, otherwise tar-pipe every requested download path back
    /// (regardless of exit code, so failure logs survive) and remove
    /// the session directory.
    fn exec_and_collect(
        &self,
        inv: &ToolInvocation,
        plan: &UploadPlan,
        wrapped: &str,
    ) -> Result<ToolOutput> {
        let mut ssh = self.ssh_cmd(wrapped);
        ssh.stdout(Stdio::piped());
        ssh.stderr(Stdio::piped());
        if inv.stdin.is_some() {
            ssh.stdin(Stdio::piped());
        }
        let mut child = ssh
            .spawn()
            .map_err(|e| XilinxError::RemoteTransfer(format!("spawn ssh exec: {e}")))?;
        if let Some(bytes) = &inv.stdin {
            if let Some(mut si) = child.stdin.take() {
                si.write_all(bytes)
                    .map_err(|e| XilinxError::RemoteTransfer(format!("write stdin: {e}")))?;
            }
        }
        let out = child
            .wait_with_output()
            .map_err(|e| XilinxError::RemoteTransfer(format!("wait ssh exec: {e}")))?;
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        let code = out.status.code().unwrap_or(-1);
        if code != 0
            && !stderr.is_empty()
            && classify_ssh_error(&stderr) == SshErrorKind::TransientMux
        {
            cleanup_session(&self.session, &plan.session_dir);
            return Err(self.classify_remote_failure(&stderr));
        }

        // Download artifacts regardless of exit code so that failure logs
        // and `--keep-hls-work-dir` project trees are preserved locally.
        for (raw, abs) in inv.downloads.iter().zip(plan.downloads_abs.iter()) {
            let remote_src = local_to_remote_path(abs, &plan.session_dir);
            // Download back to the caller's requested path (raw), which
            // may be relative — keeping the caller-facing contract.
            download_tree(&self.session, &remote_src, raw.as_std_path())?;
        }

        cleanup_session(&self.session, &plan.session_dir);

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
/// and then retries `attempt` exactly once. Non-recoverable errors pass
/// through unchanged.
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
}

fn is_recoverable_mux_error(err: &XilinxError) -> bool {
    match err {
        XilinxError::SshMuxLost { .. } => true,
        XilinxError::RemoteTransfer(msg) => classify_ssh_error(msg) == SshErrorKind::TransientMux,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    //! Unit tests for the `run_with_mux_retry` helper and the pure
    //! `build_remote_script` stage. Neither needs a live SSH session,
    //! so this block pins their contracts independently of any host.
    //! Integration tests that drive a real `RemoteToolRunner` live
    //! under `tests/integration_remote.rs`.

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

    #[test]
    fn build_script_splices_downloads_settings_env_and_exec() {
        // Representative full invocation: one download-target mkdir,
        // the Xilinx settings source line, one allowlisted env entry
        // whose value is rewritten (with a non-allowlisted entry
        // dropped), and absolute paths rewritten in the args before
        // the `cd && exec` line.
        let plan = UploadPlan {
            session_dir: "/tmp/tapa-remote/tapa-1-2-3".to_string(),
            referenced: vec![
                Utf8PathBuf::from("/work/top"),
                Utf8PathBuf::from("/work/top/run.tcl"),
                Utf8PathBuf::from("/work/top/work.out"),
            ],
            to_upload: vec![Utf8PathBuf::from("/work/top")],
            cwd_abs: Some(Utf8PathBuf::from("/work/top")),
            downloads_abs: vec![Utf8PathBuf::from("/work/top/work.out")],
        };
        let inv = ToolInvocation::new("vitis_hls")
            .arg("-f")
            .arg("/work/top/run.tcl")
            .env("TAPA_TCL", "/work/top/run.tcl")
            .env("AWS_SECRET_KEY", "s3cr3t");
        let script =
            build_remote_script(&plan, &inv, Some("/opt/Xilinx/Vitis/2023.2/settings64.sh"));
        assert_eq!(
            script,
            "bash -c 'mkdir -p /tmp/tapa-remote/tapa-1-2-3/rootfs/work/top/work.out ; \
                source /opt/Xilinx/Vitis/2023.2/settings64.sh ; \
                export TAPA_TCL=/tmp/tapa-remote/tapa-1-2-3/rootfs/work/top/run.tcl ; \
                cd /tmp/tapa-remote/tapa-1-2-3/rootfs/work/top && \
                if ! command -v vitis_hls >/dev/null 2>&1 && \
                command -v vitis-run >/dev/null 2>&1; \
                then exec vitis-run --mode hls --tcl \
                /tmp/tapa-remote/tapa-1-2-3/rootfs/work/top/run.tcl; \
                else exec vitis_hls -f \
                /tmp/tapa-remote/tapa-1-2-3/rootfs/work/top/run.tcl; fi'"
        );
    }

    #[test]
    fn build_script_rewrites_paths_longest_match_first() {
        // A referenced path that prefixes another (`/opt/a` vs
        // `/opt/a/b`) must not double- or mis-rewrite the longer one.
        let plan = UploadPlan {
            session_dir: "/tmp/tapa-remote/tapa-9-9-9".to_string(),
            referenced: vec![Utf8PathBuf::from("/opt/a"), Utf8PathBuf::from("/opt/a/b")],
            to_upload: vec![Utf8PathBuf::from("/opt/a")],
            cwd_abs: Some(Utf8PathBuf::from("/opt/a")),
            downloads_abs: vec![],
        };
        let inv = ToolInvocation::new("vivado")
            .arg("-source")
            .arg("/opt/a/b/run.tcl")
            .arg("-log")
            .arg("/opt/a/vivado.log");
        let script = build_remote_script(&plan, &inv, None);
        assert_eq!(
            script,
            "bash -c 'cd /tmp/tapa-remote/tapa-9-9-9/rootfs/opt/a && \
                exec vivado -source /tmp/tapa-remote/tapa-9-9-9/rootfs/opt/a/b/run.tcl \
                -log /tmp/tapa-remote/tapa-9-9-9/rootfs/opt/a/vivado.log'"
        );
    }

    #[test]
    fn build_script_quotes_words_containing_spaces() {
        // A cwd / rewritten arg with a space is single-quoted at its
        // point of use. The outer `bash -c` wrap re-quotes the
        // assembled script via `shell_quote` (pinned separately in
        // `transport`), so this test pins the assembled script text.
        let plan = UploadPlan {
            session_dir: "/tmp/tapa-remote/tapa-7-7-7".to_string(),
            referenced: vec![Utf8PathBuf::from("/proj/hello world")],
            to_upload: vec![Utf8PathBuf::from("/proj/hello world")],
            cwd_abs: Some(Utf8PathBuf::from("/proj/hello world")),
            downloads_abs: vec![],
        };
        let inv = ToolInvocation::new("v++")
            .arg("--kernel")
            .arg("/proj/hello world/kernel.cpp")
            .arg("--output")
            .arg("my kernel.xo");
        let script = build_remote_script(&plan, &inv, None);
        let full_cmd = "cd '/tmp/tapa-remote/tapa-7-7-7/rootfs/proj/hello world' \
            && exec v++ --kernel '/tmp/tapa-remote/tapa-7-7-7/rootfs/proj/hello world/kernel.cpp' \
            --output 'my kernel.xo'";
        assert_eq!(script, format!("bash -c {}", shell_quote(full_cmd)));
    }

    #[test]
    fn build_script_without_cwd_uses_rootfs_and_skips_blank_settings() {
        // No cwd -> the remote cwd falls back to the session's
        // `rootfs/`; a whitespace-only `xilinx_settings` contributes
        // no source line and empty download/env lists add no parts.
        let plan = UploadPlan {
            session_dir: "/tmp/tapa-remote/tapa-5-5-5".to_string(),
            referenced: vec![],
            to_upload: vec![],
            cwd_abs: None,
            downloads_abs: vec![],
        };
        let inv = ToolInvocation::new("echo").arg("hello");
        let script = build_remote_script(&plan, &inv, Some("  "));
        assert_eq!(
            script,
            "bash -c 'cd /tmp/tapa-remote/tapa-5-5-5/rootfs && exec echo hello'"
        );
    }
}
