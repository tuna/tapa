//! End-to-end Vivado integration. Same gating as `integration_hls`.

mod common;

use std::sync::Arc;

use tapa_xilinx::{
    run_vivado, LocalToolRunner, RemoteToolRunner, SshMuxOptions, SshSession, ToolRunner,
    VivadoJob,
};

#[test]
#[ignore = "requires real vivado or configured remote host"]
fn vivado_runs_minimal_tcl() {
    if common::should_skip_without_env() {
        eprintln!("integration_vivado: no Xilinx env; skipping");
        return;
    }
    let tcl = "puts \"vivado-integration-ok\"\nexit\n";
    let job = VivadoJob::new(tcl);
    if let Some(cfg) = common::has_remote_config() {
        let session = Arc::new(SshSession::new(cfg, SshMuxOptions::default()));
        session.ensure_established().expect("ssh setup");
        let runner: Box<dyn ToolRunner> = Box::new(RemoteToolRunner::new(session));
        let out = run_vivado(runner.as_ref(), &job).expect("vivado run");
        assert!(
            out.stdout.contains("vivado-integration-ok"),
            "stdout={}",
            out.stdout
        );
    } else {
        let runner = LocalToolRunner::new();
        let out = run_vivado(&runner, &job).expect("vivado run");
        assert!(out.stdout.contains("vivado-integration-ok"));
    }
}
