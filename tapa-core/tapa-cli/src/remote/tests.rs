//! Tests for the remote-config bootstrap.
//!
//! Loaded from `remote.rs` via `#[path = "remote/tests.rs"] mod tests`
//! so the per-file LOC budget on `remote.rs` stays under 450.

use super::*;
use std::sync::Mutex;

// Bootstrap mutates process-wide env (TAPA_RC_PATH, HOME, XILINX_*),
// so all tests in this module must serialize.
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn make_globals() -> GlobalArgs {
    GlobalArgs {
        verbose: 0,
        quiet: 0,
        work_dir: PathBuf::from("./work.out/"),
        temp_dir: None,
        clang_format_quota_in_bytes: 1_000_000,
        remote_host: None,
        remote_key_file: None,
        remote_xilinx_settings: None,
        remote_ssh_control_dir: None,
        remote_ssh_control_persist: None,
        remote_disable_ssh_mux: false,
    }
}

fn write_taparc(td: &tempfile::TempDir, body: &str) -> PathBuf {
    let p = td.path().join(".taparc");
    std::fs::write(&p, body).expect("write taparc");
    p
}

/// Scoped guard that sets env vars on construction and clears them
/// (or restores the prior value) on drop. Avoids cross-test leakage.
struct EnvGuard {
    keys: Vec<(String, Option<std::ffi::OsString>)>,
}
impl EnvGuard {
    fn new() -> Self {
        Self { keys: Vec::new() }
    }
    fn set(&mut self, k: &str, v: &str) {
        self.keys.push((k.to_string(), std::env::var_os(k)));
        std::env::set_var(k, v);
    }
}
impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (k, prev) in self.keys.drain(..).rev() {
            match prev {
                Some(v) => std::env::set_var(&k, v),
                None => std::env::remove_var(&k),
            }
        }
    }
}

#[test]
fn missing_taparc_and_no_cli_returns_none() {
    let _lock = ENV_LOCK.lock().unwrap();
    let mut g = EnvGuard::new();
    let td = tempfile::tempdir().unwrap();
    g.set(
        TAPARC_PATH_ENV,
        td.path().join(".taparc-missing").to_str().unwrap(),
    );
    let cfg = build_remote_config(&make_globals()).unwrap();
    assert!(cfg.is_none(), "no taparc + no --remote-host => None");
}

#[test]
fn taparc_without_host_returns_none() {
    let _lock = ENV_LOCK.lock().unwrap();
    let mut g = EnvGuard::new();
    let td = tempfile::tempdir().unwrap();
    let p = write_taparc(&td, "remote:\n  user: alice\n");
    g.set(TAPARC_PATH_ENV, p.to_str().unwrap());
    let cfg = build_remote_config(&make_globals()).unwrap();
    assert!(
        cfg.is_none(),
        "taparc without `host` must yield None to match Python",
    );
}

#[test]
fn taparc_with_no_remote_section_returns_none() {
    let _lock = ENV_LOCK.lock().unwrap();
    let mut g = EnvGuard::new();
    let td = tempfile::tempdir().unwrap();
    let p = write_taparc(&td, "unrelated: yes\n");
    g.set(TAPARC_PATH_ENV, p.to_str().unwrap());
    let cfg = build_remote_config(&make_globals()).unwrap();
    assert!(cfg.is_none(), "no [remote] section => None");
}

#[test]
fn taparc_defaults_flow_through() {
    let _lock = ENV_LOCK.lock().unwrap();
    let mut g = EnvGuard::new();
    g.set("HOME", "/home/alice");
    let td = tempfile::tempdir().unwrap();
    let p = write_taparc(
        &td,
        "remote:\n  host: fpga01.example.com\n  user: alice\n  port: 2222\n  \
         key_file: ~/.ssh/id_ed25519\n  \
         xilinx_settings: /opt/xilinx/settings64.sh\n",
    );
    g.set(TAPARC_PATH_ENV, p.to_str().unwrap());
    let cfg = build_remote_config(&make_globals())
        .unwrap()
        .expect("config must be Some when taparc has host");
    assert_eq!(cfg.host, "fpga01.example.com");
    assert_eq!(cfg.user, "alice");
    assert_eq!(cfg.port, 2222);
    assert_eq!(
        cfg.key_file.as_deref(),
        Some(Path::new("/home/alice/.ssh/id_ed25519")),
        "key_file must be tilde-expanded by RemoteConfig::from_yaml_str",
    );
    assert_eq!(
        cfg.xilinx_settings.as_deref(),
        Some("/opt/xilinx/settings64.sh"),
    );
    assert!(cfg.ssh_multiplex, "default ssh_multiplex must be true");
}

#[test]
fn cli_remote_host_overrides_taparc() {
    let _lock = ENV_LOCK.lock().unwrap();
    let mut g = EnvGuard::new();
    g.set("HOME", "/home/alice");
    let td = tempfile::tempdir().unwrap();
    let p = write_taparc(
        &td,
        "remote:\n  host: from-file.example.com\n  user: file-user\n  port: 22\n",
    );
    g.set(TAPARC_PATH_ENV, p.to_str().unwrap());
    let mut globals = make_globals();
    globals.remote_host = Some("cli-user@cli-host.example.com:2200".into());
    let cfg = build_remote_config(&globals).unwrap().unwrap();
    assert_eq!(cfg.host, "cli-host.example.com");
    assert_eq!(cfg.user, "cli-user");
    assert_eq!(cfg.port, 2200);
}

#[test]
fn cli_remote_host_without_taparc_constructs_fresh_config() {
    let _lock = ENV_LOCK.lock().unwrap();
    let mut g = EnvGuard::new();
    g.set("USER", "ci");
    g.set(TAPARC_PATH_ENV, "/path/that/cannot/exist/.taparc");
    let mut globals = make_globals();
    globals.remote_host = Some("fpga.example.com".into());
    let cfg = build_remote_config(&globals).unwrap().unwrap();
    assert_eq!(cfg.host, "fpga.example.com");
    // No user/port in the CLI spec => fall back to defaults.
    assert_eq!(cfg.port, 22);
    assert_eq!(cfg.user, "ci");
}

#[test]
fn cli_overrides_take_precedence_for_individual_flags() {
    let _lock = ENV_LOCK.lock().unwrap();
    let mut g = EnvGuard::new();
    g.set("HOME", "/home/alice");
    let td = tempfile::tempdir().unwrap();
    let p = write_taparc(
        &td,
        "remote:\n  host: h\n  user: u\n  \
         key_file: /from/file/key\n  \
         xilinx_settings: /from/file/settings.sh\n  \
         ssh_control_dir: /from/file/ctl\n  \
         ssh_control_persist: 4h\n  \
         ssh_multiplex: true\n",
    );
    g.set(TAPARC_PATH_ENV, p.to_str().unwrap());
    let mut globals = make_globals();
    globals.remote_key_file = Some("~/cli-key".into());
    globals.remote_xilinx_settings = Some("/from/cli/settings.sh".into());
    globals.remote_ssh_control_dir = Some("~/cli-ctl".into());
    globals.remote_ssh_control_persist = Some("15m".into());
    globals.remote_disable_ssh_mux = true;
    let cfg = build_remote_config(&globals).unwrap().unwrap();
    assert_eq!(
        cfg.key_file.as_deref(),
        Some(Path::new("/home/alice/cli-key")),
        "CLI --remote-key-file must override and tilde-expand",
    );
    assert_eq!(
        cfg.xilinx_settings.as_deref(),
        Some("/from/cli/settings.sh"),
        "CLI --remote-xilinx-settings must override the file value",
    );
    assert_eq!(
        cfg.ssh_control_dir.as_deref(),
        Some(Path::new("/home/alice/cli-ctl")),
    );
    assert_eq!(cfg.ssh_control_persist, "15m");
    assert!(
        !cfg.ssh_multiplex,
        "--remote-disable-ssh-mux must clobber file ssh_multiplex=true",
    );
}

#[test]
fn parse_remote_host_spec_variants() {
    let only_host = parse_remote_host_spec("h.example").unwrap();
    assert_eq!(only_host.host, "h.example");
    assert_eq!(only_host.user, None);
    assert_eq!(only_host.port, None);

    let with_port = parse_remote_host_spec("h.example:2222").unwrap();
    assert_eq!(with_port.host, "h.example");
    assert_eq!(with_port.port, Some(2222));

    let full = parse_remote_host_spec("alice@h.example:2222").unwrap();
    assert_eq!(full.user.as_deref(), Some("alice"));
    assert_eq!(full.host, "h.example");
    assert_eq!(full.port, Some(2222));

    let bad_port =
        parse_remote_host_spec("h:notaport").expect_err("bad port must fail");
    assert!(
        bad_port.contains("invalid port"),
        "error must point at port: `{bad_port}`",
    );
}

#[test]
fn malformed_yaml_warns_and_returns_none() {
    // Codex Round 2 parity finding: Python's `load_remote_config`
    // logs a warning and continues when `~/.taparc` is unreadable or
    // malformed; Rust must do the same so a stale taparc cannot fatally
    // block `tapa version` for users without an active remote setup.
    let lock = ENV_LOCK.lock();
    let _lock = match lock {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    let mut g = EnvGuard::new();
    let td = tempfile::tempdir().unwrap();
    let p = write_taparc(&td, "remote: : :\nkey: [unterminated\n");
    g.set(TAPARC_PATH_ENV, p.to_str().unwrap());
    let cfg = build_remote_config(&make_globals())
        .expect("malformed taparc must warn-and-skip, never error");
    assert!(
        cfg.is_none(),
        "malformed taparc must yield None (Python parity)",
    );
}
