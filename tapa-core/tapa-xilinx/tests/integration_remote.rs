//! SSH control-master lifecycle integration. Requires a live remote
//! host (`VARS.local.bzl`-style `REMOTE_HOST` / `REMOTE_USER` / etc.).

mod common;

#[test]
#[ignore = "requires configured remote host"]
fn control_master_lifecycle() {
    let Some(_cfg) = common::has_remote_config() else {
        eprintln!("integration_remote: no REMOTE_HOST; skipping");
        return;
    };
    // Live establish → force-remove socket → auto-restart lands with
    // the remote-execution milestone.
    eprintln!("integration_remote: remote config present; body pending");
}
