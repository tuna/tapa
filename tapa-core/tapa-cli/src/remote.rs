//! Remote-config bootstrap for the `tapa` CLI entry.
//!
//! Mirrors `tapa/__main__.py::entry_point` lines 130-181:
//!
//! 1. `load_remote_config(remote_host)` — read `~/.taparc` (YAML),
//!    optionally splice in `--remote-host=user@host[:port]`. Returns
//!    `None` when neither source provides a `host`.
//! 2. `_apply_remote_overrides(...)` — overlay the remaining
//!    `--remote-*` CLI flags onto the loaded config. CLI wins.
//! 3. `sync_remote_vendor_includes(config)` — when a config is
//!    active, mirror `$XILINX_HLS/include` (and `gcc-*/include`)
//!    into a local cache and export `XILINX_HLS` / `XILINX_VITIS`
//!    via `setenv`-with-default semantics. Sync failures are
//!    non-fatal: the Python loader logs a warning and continues so
//!    the in-tree `tapacc` flow can still run.
//!
//! The `~/.taparc` location is resolved via:
//!   - `TAPA_RC_PATH` env var (test override), then
//!   - `$HOME/.taparc`, then
//!   - skipped if `HOME` is unset (matches Python's silent skip).
//!
//! The function is deliberately small — anything more complex belongs
//! in `tapa-xilinx` (where the `RemoteConfig` schema lives).

use std::path::{Path, PathBuf};

use tapa_xilinx::{
    sync_remote_vendor_includes, RemoteConfig, SshMuxOptions, SshSession,
};

use crate::error::{CliError, Result};
use crate::globals::GlobalArgs;

/// Env var consulted before falling back to `$HOME/.taparc`. Tests use
/// this to point the loader at a tempdir; production users do not need
/// to set it. Mirrors a common pattern in this repo (`REMOTE_*`).
pub const TAPARC_PATH_ENV: &str = "TAPA_RC_PATH";

/// Resolve the on-disk path to `~/.taparc`. Returns `None` when neither
/// `TAPA_RC_PATH` nor `HOME` is set — matches Python's silent skip.
fn taparc_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os(TAPARC_PATH_ENV) {
        return Some(PathBuf::from(p));
    }
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".taparc"))
}

/// Parse `user@host[:port]` into the three optional pieces.
/// Mirrors `tapa/remote/config.py::_parse_remote_host`.
fn parse_remote_host_spec(
    spec: &str,
) -> std::result::Result<RemoteHostSpec, String> {
    let (user, rest) = match spec.split_once('@') {
        Some((u, r)) => (Some(u.to_string()), r),
        None => (None, spec),
    };
    let (host, port) = match rest.rsplit_once(':') {
        Some((h, p)) => {
            let port: u16 = p
                .parse()
                .map_err(|e| format!("invalid port `{p}` in `--remote-host`: {e}"))?;
            (h.to_string(), Some(port))
        }
        None => (rest.to_string(), None),
    };
    if host.is_empty() {
        return Err(format!("`--remote-host` is missing a host: `{spec}`"));
    }
    Ok(RemoteHostSpec { user, host, port })
}

#[derive(Debug)]
struct RemoteHostSpec {
    user: Option<String>,
    host: String,
    port: Option<u16>,
}

/// Read `~/.taparc` and return the `remote` mapping as a YAML value.
/// Returns `None` when the file is absent, unreadable, malformed, or
/// its `remote:` section is missing — Python's
/// `tapa.remote.config.load_remote_config` logs a warning and
/// continues for every one of these cases, and Rust must match that
/// parity behavior (a fatal Rust error used to block `tapa version`
/// for users with a stale `~/.taparc`).
fn load_taparc_remote_section(path: &Path) -> Option<serde_yaml::Value> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => {
            log::warn!(
                "ignoring `{}`: {} (matching Python's warn-and-skip)",
                path.display(),
                e,
            );
            return None;
        }
    };
    let value: serde_yaml::Value = match serde_yaml::from_str(&text) {
        Ok(v) => v,
        Err(e) => {
            log::warn!(
                "ignoring `{}`: invalid YAML ({e}); proceeding without remote config",
                path.display(),
            );
            return None;
        }
    };
    let map = match value {
        serde_yaml::Value::Mapping(m) => m,
        serde_yaml::Value::Null => return None,
        other @ (serde_yaml::Value::Bool(_)
        | serde_yaml::Value::Number(_)
        | serde_yaml::Value::String(_)
        | serde_yaml::Value::Sequence(_)
        | serde_yaml::Value::Tagged(_)) => {
            log::warn!(
                "ignoring `{}`: expected a top-level YAML mapping, got {other:?}",
                path.display(),
            );
            return None;
        }
    };
    map.get("remote").cloned()
}

/// Splice the CLI `--remote-host=user@host[:port]` triple into a YAML
/// mapping (creating one if needed). The CLI value wins on every key.
fn apply_remote_host_to_yaml(
    base: Option<serde_yaml::Value>,
    spec: &RemoteHostSpec,
) -> serde_yaml::Mapping {
    let mut map = match base {
        Some(serde_yaml::Value::Mapping(m)) => m,
        _ => serde_yaml::Mapping::new(),
    };
    map.insert(
        serde_yaml::Value::String("host".into()),
        serde_yaml::Value::String(spec.host.clone()),
    );
    if let Some(u) = spec.user.as_ref() {
        map.insert(
            serde_yaml::Value::String("user".into()),
            serde_yaml::Value::String(u.clone()),
        );
    }
    if let Some(p) = spec.port {
        map.insert(
            serde_yaml::Value::String("port".into()),
            serde_yaml::Value::Number(serde_yaml::Number::from(p)),
        );
    }
    map
}

/// Expand a leading `~` against `$HOME`. Mirrors
/// `os.path.expanduser` for the two paths the override layer touches.
fn expand_home(input: &str) -> PathBuf {
    if let Some(rest) = input.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    } else if input == "~" {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home);
        }
    }
    PathBuf::from(input)
}

/// Apply the remaining CLI override flags. Mirrors
/// `tapa/__main__.py::_apply_remote_overrides`.
fn apply_cli_overrides(cfg: &mut RemoteConfig, globals: &GlobalArgs) {
    if let Some(p) = globals.remote_key_file.as_deref() {
        cfg.key_file = Some(expand_home(p));
    }
    if let Some(s) = globals.remote_xilinx_settings.as_deref() {
        cfg.xilinx_settings = Some(s.to_string());
    }
    if let Some(d) = globals.remote_ssh_control_dir.as_deref() {
        cfg.ssh_control_dir = Some(expand_home(d));
    }
    if let Some(p) = globals.remote_ssh_control_persist.as_deref() {
        cfg.ssh_control_persist = p.to_string();
    }
    if globals.remote_disable_ssh_mux {
        cfg.ssh_multiplex = false;
    }
}

/// Build the active `RemoteConfig` (or `None`) from `~/.taparc` plus
/// the CLI override flags. Pure: does not touch the network or env.
pub fn build_remote_config(globals: &GlobalArgs) -> Result<Option<RemoteConfig>> {
    let cli_spec = match globals.remote_host.as_deref() {
        Some(s) => Some(
            parse_remote_host_spec(s).map_err(CliError::InvalidArg)?,
        ),
        None => None,
    };

    let file_remote = taparc_path().and_then(|p| load_taparc_remote_section(&p));

    if cli_spec.is_none() && file_remote.is_none() {
        return Ok(None);
    }

    let map = match cli_spec.as_ref() {
        Some(spec) => apply_remote_host_to_yaml(file_remote, spec),
        None => match file_remote {
            Some(serde_yaml::Value::Mapping(m)) => m,
            _ => return Ok(None),
        },
    };

    if !map.contains_key(serde_yaml::Value::String("host".into())) {
        return Ok(None);
    }

    // Re-emit the spliced mapping under `remote:` so we can reuse the
    // canonical `RemoteConfig::from_yaml_str` path-expansion + default
    // logic that lives in `tapa-xilinx`.
    let mut top = serde_yaml::Mapping::new();
    top.insert(
        serde_yaml::Value::String("remote".into()),
        serde_yaml::Value::Mapping(map),
    );
    let yaml_text = serde_yaml::to_string(&serde_yaml::Value::Mapping(top))
        .map_err(|e| CliError::RemoteConfigParse {
            path: PathBuf::from("<merged>"),
            message: e.to_string(),
        })?;
    let mut cfg = RemoteConfig::from_yaml_str(&yaml_text, "<merged>")
        .map_err(|e| CliError::RemoteConfigParse {
            path: PathBuf::from("<merged>"),
            message: e.to_string(),
        })?;
    apply_cli_overrides(&mut cfg, globals);
    Ok(Some(cfg))
}

/// Side effects after a config is resolved: mirror the Python
/// `sync_remote_vendor_includes` + `os.environ.setdefault` block.
/// Sync failures are logged and swallowed so test environments
/// without SSH still run.
fn sync_and_export_env(cfg: &RemoteConfig) {
    let session = SshSession::new(cfg.clone(), SshMuxOptions::default());
    match sync_remote_vendor_includes(&session) {
        Ok(cache_dir) => {
            let s = cache_dir.to_string_lossy().into_owned();
            // `setdefault` semantics: never clobber an explicit
            // `XILINX_HLS=...` already exported by the user.
            if std::env::var_os("XILINX_HLS").is_none() {
                std::env::set_var("XILINX_HLS", &s);
            }
            if std::env::var_os("XILINX_VITIS").is_none() {
                std::env::set_var("XILINX_VITIS", &s);
            }
        }
        Err(e) => {
            log::warn!("remote vendor-header sync failed: {e}");
        }
    }
}

/// Top-level bootstrap. Reads `~/.taparc`, applies CLI overrides,
/// triggers the vendor-include sync, exports `XILINX_HLS` /
/// `XILINX_VITIS`, and returns the active config (or `None`).
pub fn bootstrap_remote(globals: &GlobalArgs) -> Result<Option<RemoteConfig>> {
    let cfg = build_remote_config(globals)?;
    if let Some(c) = cfg.as_ref() {
        log::info!(
            "using remote host {}@{}:{} for vendor tools",
            c.user,
            c.host,
            c.port,
        );
        sync_and_export_env(c);
    }
    Ok(cfg)
}
#[cfg(test)]
#[path = "remote/tests.rs"]
mod tests;
