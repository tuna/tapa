//! End-to-end Vivado integration. Same gating as `integration_hls`.

mod common;

#[test]
#[ignore = "requires real vivado or configured remote host"]
fn vivado_runs_minimal_tcl() {
    if common::should_skip_without_env() {
        eprintln!("integration_vivado: no Xilinx env; skipping");
        return;
    }
    eprintln!("integration_vivado: environment available; body pending");
}
