//! SPIKE: Synchronous wrapper around the async `openssh` crate.
//!
//! This module prototypes replacing the hand-rolled
//! `std::process::Command("ssh")` calls in `ssh.rs` with the `openssh`
//! crate while keeping the public API synchronous.
//!
//! # Feasibility findings
//!
//! ## 1. Async→sync bridging is mandatory
//! The `openssh` crate is **async-only** (built on `tokio`).  Every
//! public method on the spike type must `block_on` a future via an
//! internal `tokio::runtime::Runtime`.  This adds ~15 lines of boiler-
//! plate per fallible call site and forces us to carry `tokio` as a
//! dependency even though the rest of `tapa-xilinx` is synchronous.
//!
//! ## 2. Missing SSH options
//! `openssh::SessionBuilder` exposes a **small subset** of OpenSSH
//! flags:
//!
//! | Option we need                 | `SessionBuilder` support |
//! |--------------------------------|--------------------------|
//! | `BatchMode=yes`                | ❌ not exposed           |
//! | `StrictHostKeyChecking=accept-new` | ❌ (`KnownHosts::AcceptNew` is close but not identical) |
//! | `ServerAliveCountMax`          | ❌ not exposed           |
//! | `MaxSessions`                  | ❌ not exposed           |
//! | `ControlMaster=auto`           | ✅ implicit in `connect_mux` |
//! | `ControlPath=<dir>/cm-%C`      | ⚠️  partial (`control_directory` only) |
//! | `ControlPersist=<time>`        | ✅ via `control_persist` |
//! | `ServerAliveInterval`          | ✅ via `server_alive_interval` |
//! | `-i <key>`                     | ✅ via `keyfile`         |
//! | `-p <port>`                    | ✅ via `port`            |
//!
//! Options we cannot set programmatically would have to be pushed into
//! the user’s `~/.ssh/config`, which breaks the current "all config in
//! `RemoteConfig`" contract.
//!
//! ## 3. Shell wrapping is still required
//! `openssh::Command` has **no `env()` or `current_dir()`** methods.
//! The crate docs explicitly recommend prefixing commands with
//! `cd dir &&` or using `env(1)`.  Consequently `RemoteToolRunner`
//! would still construct the exact same `bash -c 'cd … && export … &&
//! exec …'` string that it does today.  `openssh` does provide
//! `Session::shell(cmd)` which runs `sh -c <cmd>`, but that only saves
//! us the outer `bash -c` wrapper — the inner path rewriting, export
//! setup, and `exec` remain unchanged.
//!
//! ## 4. Streaming tar-pipe becomes awkward
//! The current vendor-sync path spawns `ssh … tar -czf - …` and pipes
//! the stdout **synchronously** into `flate2::read::GzDecoder` →
//! `tar::Archive`.  With `openssh`, `Child::stdout()` returns a
//! `tokio::io::AsyncRead` handle.  Consuming it from synchronous code
//! requires either:
//!
//!   a. Buffering the entire tarball into memory (shown in the spike).
//!   b. Writing to a temp file async, then unpacking sync.
//!   c. Adding `tokio-util` + `SyncIoBridge` for async→sync adaptors.
//!
//! All three options add complexity or memory/disk overhead compared
//! with the current direct pipe.
//!
//! ## 5. Error mapping is a wash
//! `openssh::Error` gives typed variants (`Master`, `Connect`,
//! `Disconnected`, `SshMux`, …) which are nicer than raw strings for
//! *connection* failures.  However, once a remote command is running,
//! mux/auth/host-unreachable failures still surface through **stderr
//! text** (exit 255, broken pipe, …).  We would still need the exact
//! same `classify_ssh_error` pattern-matching table, plus an extra
//! layer translating `openssh::Error` → `XilinxError` on top.
//!
//! ## 6. Testability degrades
//! The current layer is easy to test without a live SSH server:
//! * `classify_ssh_error` is a pure function with exhaustive unit tests.
//! * `build_ssh_args` is deterministic and tested.
//! * `run_with_mux_retry` is fully pure.
//! * `VendorRemoteFs` is a trait, so `sync_vendor_includes_impl` is
//!   tested with a mock.
//!
//! The `openssh` crate offers no trait abstraction; `Session` is a
//! concrete struct with private fields.  Mocking it for unit tests
//! would require inventing our own trait (which we already have with
//! `VendorRemoteFs`) and wrapping `openssh` a second time, or relying
//! on integration tests against a real sshd.
//!
//! ## 7. Control-master lifecycle is less visible
//! `openssh` manages the control socket internally inside a temp
//! directory.  While this reduces code, it also means:
//! * We cannot force a specific `ControlPath=…/cm-%C` template.
//! * We cannot reliably unlink stale sockets on reset (openssh cleans
//!   up on `Session::close`, but crashes may leak `.ssh-connection-*`
//!   temp dirs).
//! * The `control_master_alive` check becomes `Session::check()`, which
//!   is cleaner but opaque.
//!
//! # Verdict (preliminary)
//! The `openssh` crate **does not reduce code size or risk** for our
//! use case.  It eliminates the manual `Command::new("ssh")` boiler-
//! plate (~30 lines) but replaces it with:
//! * an internal `tokio::Runtime` (~10 lines),
//! * async→sync bridging at every call site (~20 lines),
//! * an extra error-translation layer (~20 lines),
//! * loss of fine-grained SSH option control,
//! * awkward streaming for tar-pipe downloads.
//!
//! Because the public API must stay synchronous and our two main call
//! sites (`RemoteToolRunner::run_once` and `SshVendorFs::download_dir`)
//! both need low-level stdin/stdout/stderr control, the hand-rolled
//! `std::process::Command("ssh")` approach is actually **simpler and
//! more predictable**.  **Recommendation: NO-GO.**

use std::path::Path;
use std::sync::Mutex;

use camino::Utf8PathBuf;
use openssh::{ControlPersist, KnownHosts, Session, SessionBuilder};
use tokio::runtime::Runtime;

use crate::error::{Result, XilinxError};
use crate::runtime::config::RemoteConfig;
use crate::runtime::remote::shell_quote;
use crate::runtime::ssh::{classify_ssh_error, SshErrorKind, SshMuxOptions};

/// Synchronous façade over an async `openssh::Session`.
pub struct OpenSshSession {
    cfg: RemoteConfig,
    options: SshMuxOptions,
    rt: Runtime,
    inner: Mutex<Option<Session>>,
}

impl OpenSshSession {
    /// Create a new session wrapper.  A `tokio::Runtime` is spawned
    /// here and lives for the lifetime of the struct.
    ///
    /// # Errors
    /// Returns `SshConnect` if the tokio runtime cannot be created.
    pub fn new(cfg: RemoteConfig, options: SshMuxOptions) -> Result<Self> {
        let rt = Runtime::new().map_err(|e| XilinxError::SshConnect {
            host: cfg.host.clone(),
            detail: format!("tokio runtime: {e}"),
        })?;
        Ok(Self {
            cfg,
            options,
            rt,
            inner: Mutex::new(None),
        })
    }

    pub fn config(&self) -> &RemoteConfig {
        &self.cfg
    }

    pub fn options(&self) -> &SshMuxOptions {
        &self.options
    }

    /// Idempotently establish the control-master connection.
    ///
    /// If a session already exists and `Session::check()` succeeds,
    /// returns immediately.  Otherwise spawns a new multiplexed
    /// session via `SessionBuilder::connect_mux`.
    pub fn ensure_established(&self) -> Result<()> {
        let mut guard = self.inner.lock().unwrap();
        if let Some(sess) = guard.as_ref() {
            let alive = self
                .rt
                .block_on(async { sess.check().await.is_ok() });
            if alive {
                return Ok(());
            }
        }

        let sess = self.rt.block_on(self.build_session())?;
        *guard = Some(sess);
        Ok(())
    }

    /// Tear down the control master and drop the session.
    ///
    /// Unlike the current `reset_mux`, we cannot forcibly remove stale
    /// sockets because `openssh` hides the control-path template.
    pub fn reset_mux(&self) {
        let mut guard = self.inner.lock().unwrap();
        if let Some(sess) = guard.take() {
            let _ = self.rt.block_on(async { sess.close().await });
        }
    }

    /// Probe liveness via `Session::check`.
    #[must_use]
    pub fn control_master_alive(&self) -> bool {
        let guard = self.inner.lock().unwrap();
        let Some(sess) = guard.as_ref() else {
            return false;
        };
        self.rt
            .block_on(async { sess.check().await.is_ok() })
    }

    /// Execute a shell command on the remote host.
    ///
    /// The command is passed through `sh -c` (openssh’s `shell`
    /// method).  Returns `(exit_code, stdout, stderr)`.
    ///
    /// # Errors
    /// Returns `SshMuxLost` for connection-level failures and
    /// `RemoteTransfer` for spawn/wait failures.
    pub fn exec(&self, remote_cmd: &str) -> Result<(i32, Vec<u8>, Vec<u8>)> {
        self.ensure_established()?;
        let guard = self.inner.lock().unwrap();
        let sess = guard.as_ref().ok_or_else(|| XilinxError::SshMuxLost {
            detail: "session not established".into(),
        })?;

        let output = self
            .rt
            .block_on(async { sess.shell(remote_cmd).output().await })
            .map_err(|e| map_openssh_error(&self.cfg.host, e))?;

        let code = output.status.code().unwrap_or(-1);
        Ok((code, output.stdout, output.stderr))
    }

    /// Stream a remote directory into a local path via tar-pipe.
    ///
    /// **SPIKE LIMITATION:** Because `openssh::Child::stdout()` is an
    /// async handle (`tokio::io::AsyncRead`) and `tar::Archive`
    /// expects a synchronous `std::io::Read`, this implementation
    /// buffers the entire tarball into memory before unpacking.
    /// Production code would need either `tokio-util::io::SyncIoBridge`
    /// or a temp-file dance, both of which add dependencies and
    /// overhead compared with the current direct pipe.
    pub fn download_dir(&self, remote_path: &str, local_dest: &Path) -> Result<()> {
        self.ensure_established()?;
        let guard = self.inner.lock().unwrap();
        let sess = guard.as_ref().ok_or_else(|| XilinxError::SshMuxLost {
            detail: "session not established".into(),
        })?;

        let remote_cmd = format!("tar -czf - -C {} .", shell_quote(remote_path));

        let output = self
            .rt
            .block_on(async { sess.shell(&remote_cmd).output().await })
            .map_err(|e| map_openssh_error(&self.cfg.host, e))?;

        let stderr = String::from_utf8_lossy(&output.stderr);
        let code = output.status.code().unwrap_or(-1);
        if code != 0 {
            return Err(XilinxError::RemoteTransfer(format!(
                "remote tar -cz failed (exit {code}): {}",
                stderr.trim()
            )));
        }

        std::fs::create_dir_all(local_dest).map_err(|e| {
            XilinxError::RemoteTransfer(format!("mkdir {}: {e}", local_dest.display()))
        })?;

        let cursor = std::io::Cursor::new(output.stdout);
        let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(cursor));
        archive
            .unpack(local_dest)
            .map_err(|e| XilinxError::RemoteTransfer(format!("unpack tar download: {e}")))?;
        Ok(())
    }

    // ----------------------------------------------------------------
    // Internal helpers
    // ----------------------------------------------------------------

    async fn build_session(&self) -> Result<Session> {
        let mut builder = SessionBuilder::default();
        builder
            .user(self.cfg.user.clone())
            .port(self.cfg.port)
            .known_hosts_check(KnownHosts::Accept)
            .connect_timeout(std::time::Duration::from_secs(10))
            .server_alive_interval(std::time::Duration::from_secs(
                self.options.server_alive_interval.into(),
            ));

        if let Some(key) = self.cfg.key_file.as_ref() {
            builder.keyfile(key.as_std_path());
        }

        if self.cfg.ssh_multiplex {
            let seconds = parse_control_persist(&self.cfg.ssh_control_persist)?;
            if let Some(idle) = std::num::NonZeroUsize::new(seconds as usize) {
                builder.control_persist(ControlPersist::IdleFor(idle));
            } else {
                builder.control_persist(ControlPersist::Forever);
            }
            // `control_directory` only sets the *parent* dir; openssh
            // creates its own `.ssh-connection-*` temp subdir inside it.
            // We lose the exact `cm-%C` template we use today.
            builder.control_directory(self.control_dir().as_std_path());
        }

        let dest = format!("{}@{}", self.cfg.user, self.cfg.host);
        let sess = builder
            .connect(&dest)
            .await
            .map_err(|e| map_openssh_error(&self.cfg.host, e))?;
        Ok(sess)
    }

    fn control_dir(&self) -> Utf8PathBuf {
        if let Some(dir) = self.cfg.ssh_control_dir.as_ref() {
            return dir.clone();
        }
        if let Some(xdg) = std::env::var_os("XDG_RUNTIME_DIR") {
            return Utf8PathBuf::from(xdg.to_string_lossy().into_owned())
                .join("tapa")
                .join("ssh");
        }
        Utf8PathBuf::from("/tmp/tapa-ssh-mux")
    }
}

// ------------------------------------------------------------------
// Error mapping: openssh::Error → XilinxError
// ------------------------------------------------------------------

fn map_openssh_error(host: &str, err: openssh::Error) -> XilinxError {
    let detail = err.to_string();
    match err {
        openssh::Error::Master(_) |
        openssh::Error::Disconnected => XilinxError::SshMuxLost { detail },
        openssh::Error::Connect(_) => XilinxError::SshConnect {
            host: host.to_string(),
            detail,
        },
        // Remote / RemoteProcessTerminated / ChildIo etc. are ambiguous;
        // fall back to the same string classification we already use.
        _ => {
            let kind = classify_ssh_error(&detail);
            match kind {
                SshErrorKind::TransientMux => XilinxError::SshMuxLost { detail },
                _ => XilinxError::SshConnect {
                    host: host.to_string(),
                    detail,
                },
            }
        }
    }
}

// ------------------------------------------------------------------
// Helpers
// ------------------------------------------------------------------

fn parse_control_persist(s: &str) -> Result<u64> {
    // Quick-and-dirty parse: supports "<n>m" (minutes) or plain seconds.
    // The real config validation lives in RemoteConfig; this is just
    // for the spike.
    let s = s.trim();
    if s.eq_ignore_ascii_case("yes") || s.eq_ignore_ascii_case("forever") {
        return Ok(0); // ControlPersist::Forever would be used in real code
    }
    if let Some(min) = s.strip_suffix('m') {
        min.parse::<u64>()
            .map(|v| v * 60)
            .map_err(|e| XilinxError::SshConnect {
                host: String::new(),
                detail: format!("bad ControlPersist '{s}': {e}"),
            })
    } else {
        s.parse::<u64>().map_err(|e| XilinxError::SshConnect {
            host: String::new(),
            detail: format!("bad ControlPersist '{s}': {e}"),
        })
    }
}

// ------------------------------------------------------------------
// Example refactored runner (shows API shape, not wired into mod.rs)
// ------------------------------------------------------------------

use crate::runtime::process::{ToolInvocation, ToolOutput, ToolRunner};
use std::sync::Arc;

/// What `RemoteToolRunner` would look like if backed by `OpenSshSession`.
#[allow(dead_code)]
pub struct OpenSshToolRunner {
    session: Arc<OpenSshSession>,
}

#[allow(dead_code)]
impl OpenSshToolRunner {
    pub fn new(session: Arc<OpenSshSession>) -> Self {
        Self { session }
    }

    /// The body of `run_once` collapses considerably because we no
    /// longer manually build `Command::new("ssh")`.  However, the
    /// *shell command construction* (path rewriting, exports, cd,
    /// exec) is identical to the current implementation.
    fn run_once(&self, _inv: &ToolInvocation) -> Result<ToolOutput> {
        self.session.ensure_established()?;
        let cfg = self.session.config();
        let _session_dir = format!("{}/{}", cfg.work_dir, "placeholder-id");

        // --- all the same path-rewriting logic as today ---
        // (omitted for brevity; it would be literally identical)

        let remote_cmd = "echo placeholder".to_string();
        let (code, stdout_bytes, stderr_bytes) = self.session.exec(&remote_cmd)?;
        let stdout = String::from_utf8_lossy(&stdout_bytes).into_owned();
        let stderr = String::from_utf8_lossy(&stderr_bytes).into_owned();

        if code != 0
            && !stderr.is_empty()
            && classify_ssh_error(&stderr) == SshErrorKind::TransientMux
        {
            // Note: OpenSshSession does not expose `build_ssh_args`,
            // so `cleanup_session` (which shells out to ssh) would
            // need to be reimplemented or the session would need
            // a `raw_ssh_cmd` escape hatch.
            return Err(XilinxError::SshMuxLost {
                detail: stderr.clone(),
            });
        }

        Ok(ToolOutput {
            exit_code: code,
            stdout,
            stderr,
        })
    }
}

#[allow(dead_code)]
impl ToolRunner for OpenSshToolRunner {
    fn run(&self, inv: &ToolInvocation) -> Result<ToolOutput> {
        // `run_with_mux_retry` would be identical; omitted.
        self.run_once(inv)
    }

    fn harvest(&self, _relative_from_cwd: &std::path::Path, _local_root: &std::path::Path) -> Result<()> {
        Ok(())
    }
}
