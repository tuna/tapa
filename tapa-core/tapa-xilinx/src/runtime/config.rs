//! `~/.taparc` YAML schema for remote Xilinx tool execution.
//!
//! Mirrors the Pydantic model in `tapa/remote/config.py::RemoteConfig`:
//! - `user` defaults to the current login name, not `None`;
//! - `~` is expanded in `key_file` and `ssh_control_dir`;
//! - unknown fields are ignored so Python additions do not fault the
//!   Rust loader;
//! - `from_env` seeds from the `REMOTE_*` names used in `VARS.local.bzl`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Result, XilinxError};

fn default_port() -> u16 {
    22
}

fn default_work_dir() -> String {
    "/tmp/tapa-remote".to_string()
}

fn default_ssh_control_persist() -> String {
    "30m".to_string()
}

fn default_ssh_multiplex() -> bool {
    true
}

fn current_username() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "unknown".to_string())
}

fn expand_tilde(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    } else if s == "~" {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home);
        }
    }
    p.to_path_buf()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteConfig {
    pub host: String,

    #[serde(default = "current_username")]
    pub user: String,

    #[serde(default = "default_port")]
    pub port: u16,

    #[serde(default)]
    pub key_file: Option<PathBuf>,

    #[serde(default)]
    pub xilinx_settings: Option<String>,

    #[serde(default = "default_work_dir")]
    pub work_dir: String,

    #[serde(default)]
    pub ssh_control_dir: Option<PathBuf>,

    #[serde(default = "default_ssh_control_persist")]
    pub ssh_control_persist: String,

    #[serde(default = "default_ssh_multiplex")]
    pub ssh_multiplex: bool,
}

impl RemoteConfig {
    fn normalize_paths(&mut self) {
        if let Some(p) = self.key_file.take() {
            self.key_file = Some(expand_tilde(&p));
        }
        if let Some(p) = self.ssh_control_dir.take() {
            self.ssh_control_dir = Some(expand_tilde(&p));
        }
    }

    /// Parse a `.taparc`-style YAML document. Accepts either
    /// `{remote: {...}}` (the `~/.taparc` top-level shape) or a bare
    /// `RemoteConfig` mapping.
    pub fn from_yaml_str(text: &str, path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let value: serde_yaml::Value = serde_yaml::from_str(text).map_err(|source| {
            XilinxError::Config {
                path: path.clone(),
                source,
            }
        })?;
        let inner = match value {
            serde_yaml::Value::Mapping(ref m) if m.contains_key("remote") => m
                .get("remote")
                .cloned()
                .unwrap_or(serde_yaml::Value::Null),
            serde_yaml::Value::Null => {
                return Err(XilinxError::Config {
                    path,
                    source: missing_mapping_error(),
                });
            }
            serde_yaml::Value::Mapping(_)
            | serde_yaml::Value::Bool(_)
            | serde_yaml::Value::Number(_)
            | serde_yaml::Value::String(_)
            | serde_yaml::Value::Sequence(_)
            | serde_yaml::Value::Tagged(_) => value,
        };
        let mut cfg: Self = serde_yaml::from_value(inner).map_err(|source| {
            XilinxError::Config {
                path: path.clone(),
                source,
            }
        })?;
        cfg.normalize_paths();
        Ok(cfg)
    }

    /// Build a `RemoteConfig` from environment variables matching the
    /// `VARS.local.bzl` naming used by the integration tests.
    /// `REMOTE_HOST` is required; everything else falls back to the
    /// same defaults as the YAML parser.
    pub fn from_env() -> Option<Self> {
        let host = std::env::var("REMOTE_HOST").ok()?;
        let mut cfg = Self {
            host,
            user: std::env::var("REMOTE_USER").unwrap_or_else(|_| current_username()),
            port: std::env::var("REMOTE_PORT")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or_else(default_port),
            key_file: std::env::var("REMOTE_KEY_FILE").ok().map(PathBuf::from),
            xilinx_settings: std::env::var("REMOTE_XILINX_SETTINGS")
                .ok()
                .or_else(|| std::env::var("REMOTE_XILINX_TOOL_PATH").ok()),
            work_dir: std::env::var("REMOTE_WORK_DIR").unwrap_or_else(|_| default_work_dir()),
            ssh_control_dir: std::env::var("REMOTE_SSH_CONTROL_DIR")
                .ok()
                .map(PathBuf::from),
            ssh_control_persist: std::env::var("REMOTE_SSH_CONTROL_PERSIST")
                .unwrap_or_else(|_| default_ssh_control_persist()),
            ssh_multiplex: std::env::var("REMOTE_SSH_MULTIPLEX").ok().is_none_or(|s| {
                matches!(s.trim().to_lowercase().as_str(), "true" | "yes" | "1" | "on")
            }),
        };
        cfg.normalize_paths();
        Some(cfg)
    }
}

fn missing_mapping_error() -> serde_yaml::Error {
    serde_yaml::from_str::<RemoteConfig>("").unwrap_err()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    const VALID: &str = "
remote:
  host: fpga01.example.com
  user: alice
  port: 2222
  key_file: ~/.ssh/id_ed25519
  xilinx_settings: /opt/xilinx/Vitis/2023.2/settings64.sh
";

    #[test]
    fn parses_valid_taparc_and_expands_tilde() {
        let _lock = ENV_LOCK.lock().unwrap();
        std::env::set_var("HOME", "/home/alice");
        let cfg = RemoteConfig::from_yaml_str(VALID, "/tmp/.taparc").unwrap();
        assert_eq!(cfg.host, "fpga01.example.com");
        assert_eq!(cfg.user, "alice");
        assert_eq!(cfg.port, 2222);
        assert_eq!(
            cfg.key_file.as_deref(),
            Some(Path::new("/home/alice/.ssh/id_ed25519"))
        );
        assert!(cfg.ssh_multiplex);
    }

    #[test]
    fn default_user_is_current_username() {
        let _lock = ENV_LOCK.lock().unwrap();
        std::env::set_var("USER", "testuser");
        let cfg = RemoteConfig::from_yaml_str("remote:\n  host: h\n", "/tmp/.taparc").unwrap();
        assert_eq!(cfg.user, "testuser");
    }

    #[test]
    fn unknown_fields_are_accepted() {
        // Python's pydantic model silently ignores unknown keys; match that.
        let text = "remote:\n  host: h\n  future_field: yes\n";
        let cfg = RemoteConfig::from_yaml_str(text, "/tmp/.taparc").unwrap();
        assert_eq!(cfg.host, "h");
    }

    #[test]
    fn round_trips_through_yaml() {
        let _lock = ENV_LOCK.lock().unwrap();
        std::env::set_var("HOME", "/home/alice");
        let cfg = RemoteConfig::from_yaml_str(VALID, "/tmp/.taparc").unwrap();
        let emitted = serde_yaml::to_string(&cfg).unwrap();
        let again: RemoteConfig = serde_yaml::from_str(&emitted).unwrap();
        assert_eq!(cfg, again);
    }

    #[test]
    fn wrong_type_surfaces_config_error() {
        let text = "remote:\n  host: h\n  port: not-a-number\n";
        let err = RemoteConfig::from_yaml_str(text, "/tmp/.taparc").unwrap_err();
        assert!(matches!(err, XilinxError::Config { .. }));
    }

    #[test]
    fn empty_document_is_error() {
        let err = RemoteConfig::from_yaml_str("", "/tmp/.taparc").unwrap_err();
        assert!(matches!(err, XilinxError::Config { .. }));
    }

    #[test]
    fn from_env_seeds_from_remote_vars() {
        let _lock = ENV_LOCK.lock().unwrap();
        std::env::set_var("REMOTE_HOST", "fpga-ci.example.com");
        std::env::set_var("REMOTE_USER", "ci");
        std::env::set_var("REMOTE_PORT", "2323");
        std::env::set_var("REMOTE_KEY_FILE", "/tmp/ci_key");
        std::env::set_var("REMOTE_XILINX_TOOL_PATH", "/opt/xilinx");
        std::env::set_var("REMOTE_SSH_MULTIPLEX", "false");
        let cfg = RemoteConfig::from_env().expect("from_env with REMOTE_HOST set");
        assert_eq!(cfg.host, "fpga-ci.example.com");
        assert_eq!(cfg.user, "ci");
        assert_eq!(cfg.port, 2323);
        assert_eq!(cfg.key_file.as_deref(), Some(Path::new("/tmp/ci_key")));
        assert_eq!(cfg.xilinx_settings.as_deref(), Some("/opt/xilinx"));
        assert!(!cfg.ssh_multiplex);

        for k in [
            "REMOTE_HOST",
            "REMOTE_USER",
            "REMOTE_PORT",
            "REMOTE_KEY_FILE",
            "REMOTE_XILINX_TOOL_PATH",
            "REMOTE_SSH_MULTIPLEX",
        ] {
            std::env::remove_var(k);
        }
    }

    #[test]
    fn from_env_missing_host_returns_none() {
        let _lock = ENV_LOCK.lock().unwrap();
        std::env::remove_var("REMOTE_HOST");
        assert!(RemoteConfig::from_env().is_none());
    }
}
