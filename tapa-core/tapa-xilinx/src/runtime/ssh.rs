//! SSH multiplexing helpers.
//!
//! `classify_ssh_error` is the pure function the reconnect heuristic
//! keys off; its fixture set is ported from the patterns in
//! `tapa/remote/ssh.py::_MUX_FAILURE_PATTERNS`. `SshSession` owns the
//! control-master lifecycle; extending the live connection / auto-
//! restart logic is deferred to the remote execution milestone.

use std::path::PathBuf;
use std::sync::Mutex;

use crate::error::{Result, XilinxError};
use crate::runtime::config::RemoteConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SshErrorKind {
    /// Transient mux failure — socket dead, broken pipe, etc. Safe to
    /// retry after tearing down and re-establishing the control master.
    TransientMux,
    /// Auth failure — a retry will not help without operator action.
    Auth,
    /// Host resolution failure — likely a config error.
    HostUnreachable,
    /// Nothing matched; treat as non-transient and let the caller
    /// decide.
    Unknown,
}

const TRANSIENT_PATTERNS: &[&str] = &[
    "control socket connect",
    "mux_client_hello_exchange",
    "mux_client_request_session",
    "mux_client_read_packet",
    "read from master failed",
    "master is dead",
    "stale control socket",
    "master refused session request",
    "broken pipe",
];

const AUTH_PATTERNS: &[&str] = &[
    "permission denied",
    "too many authentication failures",
    "no supported authentication methods",
];

const UNREACHABLE_PATTERNS: &[&str] = &[
    "could not resolve hostname",
    "connection refused",
    "no route to host",
    "network is unreachable",
];

/// Classify raw OpenSSH stderr into an actionable kind.
#[must_use]
pub fn classify_ssh_error(stderr: &str) -> SshErrorKind {
    let lower = stderr.to_lowercase();
    if TRANSIENT_PATTERNS.iter().any(|p| lower.contains(p)) {
        SshErrorKind::TransientMux
    } else if AUTH_PATTERNS.iter().any(|p| lower.contains(p)) {
        SshErrorKind::Auth
    } else if UNREACHABLE_PATTERNS.iter().any(|p| lower.contains(p)) {
        SshErrorKind::HostUnreachable
    } else {
        SshErrorKind::Unknown
    }
}

/// Options that tweak the ControlMaster invocation. Defaults match the
/// Python loader.
#[derive(Debug, Clone)]
pub struct SshMuxOptions {
    pub control_persist: String,
    pub server_alive_interval: u32,
    pub server_alive_count_max: u32,
}

impl Default for SshMuxOptions {
    fn default() -> Self {
        Self {
            control_persist: "30m".into(),
            server_alive_interval: 30,
            server_alive_count_max: 3,
        }
    }
}

/// Live ControlMaster session. The establish/restart lifecycle is
/// implemented incrementally — for now the struct tracks the config,
/// the resolved control-path, and a mutable "master-ready" flag so the
/// remote tool runner can wire up reconnection when it lands.
pub struct SshSession {
    cfg: RemoteConfig,
    options: SshMuxOptions,
    state: Mutex<SessionState>,
}

#[derive(Debug, Default)]
struct SessionState {
    control_path: Option<PathBuf>,
    ready: bool,
}

impl SshSession {
    pub fn new(cfg: RemoteConfig, options: SshMuxOptions) -> Self {
        Self {
            cfg,
            options,
            state: Mutex::new(SessionState::default()),
        }
    }

    pub fn config(&self) -> &RemoteConfig {
        &self.cfg
    }

    pub fn options(&self) -> &SshMuxOptions {
        &self.options
    }

    /// Directory holding control-master sockets.
    ///
    /// Matches `tapa/remote/ssh.py::_default_ssh_control_dir` +
    /// `get_ssh_control_dir`: the user-supplied `ssh_control_dir` from
    /// `RemoteConfig` wins; otherwise `$XDG_RUNTIME_DIR/tapa/ssh` when
    /// set, else `/tmp/tapa-ssh-mux`.
    #[must_use]
    pub fn control_dir(&self) -> PathBuf {
        if let Some(dir) = self.cfg.ssh_control_dir.as_ref() {
            return dir.clone();
        }
        if let Some(xdg) = std::env::var_os("XDG_RUNTIME_DIR") {
            return PathBuf::from(xdg).join("tapa").join("ssh");
        }
        PathBuf::from("/tmp/tapa-ssh-mux")
    }

    /// Build the base OpenSSH CLI argument vector. Matches
    /// `tapa/remote/ssh.py::build_ssh_args`, including the
    /// `ControlPath=<dir>/cm-%C` entry when multiplexing is enabled.
    pub fn build_ssh_args(&self) -> Vec<String> {
        let mut args = vec![
            "-o".into(),
            "BatchMode=yes".into(),
            "-o".into(),
            "StrictHostKeyChecking=accept-new".into(),
            "-o".into(),
            "ConnectTimeout=10".into(),
            "-p".into(),
            self.cfg.port.to_string(),
        ];
        if let Some(key) = self.cfg.key_file.as_ref() {
            args.push("-i".into());
            args.push(key.display().to_string());
        }
        if self.cfg.ssh_multiplex {
            let control_path = self.control_dir().join("cm-%C");
            args.extend([
                "-o".into(),
                "ControlMaster=auto".into(),
                "-o".into(),
                format!("ControlPath={}", control_path.display()),
                "-o".into(),
                format!("ControlPersist={}", self.cfg.ssh_control_persist),
                "-o".into(),
                format!("ServerAliveInterval={}", self.options.server_alive_interval),
                "-o".into(),
                format!(
                    "ServerAliveCountMax={}",
                    self.options.server_alive_count_max
                ),
            ]);
        }
        args
    }

    pub fn ssh_target(&self) -> String {
        format!("{}@{}", self.cfg.user, self.cfg.host)
    }

    /// Mark the master as torn down so the next remote invocation
    /// re-establishes it. Called by the remote tool runner when
    /// `classify_ssh_error` returns `TransientMux`.
    pub fn invalidate(&self) {
        let mut s = self.state.lock().unwrap();
        s.ready = false;
        s.control_path = None;
    }

    /// Establish (or reuse) the ControlMaster socket.
    ///
    /// Idempotent: if the state flag says "ready" and the socket still
    /// exists, returns immediately. Otherwise spawns a no-op `ssh
    /// <target> true` invocation with ControlMaster=auto — OpenSSH
    /// opens or reuses the master socket as a side-effect. Transient
    /// mux errors are classified via [`classify_ssh_error`] and
    /// surface as `SshMuxLost`; auth / unreachable faults surface as
    /// `SshConnect`.
    pub fn ensure_established(&self) -> Result<()> {
        {
            let s = self.state.lock().unwrap();
            if s.ready {
                if let Some(cp) = &s.control_path {
                    if cp.exists() {
                        return Ok(());
                    }
                }
            }
        }
        if self.cfg.ssh_multiplex {
            let dir = self.control_dir();
            if let Err(e) = std::fs::create_dir_all(&dir) {
                return Err(XilinxError::SshConnect {
                    host: self.cfg.host.clone(),
                    detail: format!("create control dir {}: {e}", dir.display()),
                });
            }
        }
        let mut args = self.build_ssh_args();
        args.push(self.ssh_target());
        args.push("true".into());
        let out = std::process::Command::new("ssh")
            .args(&args)
            .output()
            .map_err(|e| XilinxError::SshConnect {
                host: self.cfg.host.clone(),
                detail: format!("spawn ssh: {e}"),
            })?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
            return Err(match classify_ssh_error(&stderr) {
                SshErrorKind::TransientMux => XilinxError::SshMuxLost {
                    detail: stderr,
                },
                _ => XilinxError::SshConnect {
                    host: self.cfg.host.clone(),
                    detail: stderr,
                },
            });
        }
        let mut s = self.state.lock().unwrap();
        s.ready = true;
        if self.cfg.ssh_multiplex {
            s.control_path = Some(self.control_dir().join("cm-%C"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transient_patterns_classify() {
        for p in TRANSIENT_PATTERNS {
            assert_eq!(classify_ssh_error(p), SshErrorKind::TransientMux);
            let wrapped = format!("ssh: {p} at line 42");
            assert_eq!(classify_ssh_error(&wrapped), SshErrorKind::TransientMux);
        }
    }

    #[test]
    fn auth_and_unreachable_classify() {
        assert_eq!(
            classify_ssh_error("Permission denied (publickey)"),
            SshErrorKind::Auth
        );
        assert_eq!(
            classify_ssh_error("ssh: Could not resolve hostname unknown"),
            SshErrorKind::HostUnreachable
        );
    }

    #[test]
    fn unknown_stderr_does_not_misclassify() {
        assert_eq!(
            classify_ssh_error("something entirely different"),
            SshErrorKind::Unknown
        );
    }

    fn base_cfg() -> RemoteConfig {
        RemoteConfig {
            host: "h".into(),
            user: "u".into(),
            port: 22,
            key_file: Some(PathBuf::from("/tmp/key")),
            xilinx_settings: None,
            work_dir: "/tmp/tapa-remote".into(),
            ssh_control_dir: None,
            ssh_control_persist: "30m".into(),
            ssh_multiplex: true,
        }
    }

    #[test]
    fn ssh_args_include_control_master_and_path_when_enabled() {
        let sess = SshSession::new(base_cfg(), SshMuxOptions::default());
        let args = sess.build_ssh_args();
        assert!(args.iter().any(|a| a == "ControlMaster=auto"));
        assert!(args.iter().any(|a| a == "/tmp/key"));
        assert!(
            args.iter().any(|a| a.starts_with("ControlPath=") && a.ends_with("/cm-%C")),
            "missing ControlPath; args = {args:?}"
        );
    }

    #[test]
    fn ssh_args_use_configured_control_dir() {
        let mut cfg = base_cfg();
        cfg.ssh_control_dir = Some(PathBuf::from("/var/run/taparc"));
        let sess = SshSession::new(cfg, SshMuxOptions::default());
        let args = sess.build_ssh_args();
        assert!(args
            .iter()
            .any(|a| a == "ControlPath=/var/run/taparc/cm-%C"));
    }
}
