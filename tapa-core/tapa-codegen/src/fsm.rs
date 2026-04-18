//! FSM pragma and control logic generation.
//!
//! Ports `tapa/task_codegen/fsm.py`: `RapidStream` pragma emission
//! for clock, reset, ap-ctrl, and per-instance ap-ctrl.

use tapa_protocol::{HANDSHAKE_CLK, HANDSHAKE_RST_N, HANDSHAKE_START};
use tapa_rtl::mutation::MutableModule;

use crate::instance_signals::InstanceSignals;

/// Add `RapidStream` pragmas to an FSM module.
///
/// Generates:
/// - Clock pragma: `clk port=ap_clk`
/// - Reset pragma: `rst port=ap_rst_n active=low`
/// - Main ap-ctrl pragma with start/ready/done/idle signal names
/// - Per-instance ap-ctrl pragmas
pub fn add_rs_pragmas_to_fsm(
    fsm_module: &mut MutableModule,
    scalar_ports: &[String],
    instances: &[(String, bool)], // (instance_name, is_autorun)
) {
    // Clock and reset pragmas
    fsm_module.add_comment(format!("clk port={HANDSHAKE_CLK}"));
    fsm_module.add_comment(format!("rst port={HANDSHAKE_RST_N} active=low"));

    // Main ap-ctrl pragma
    let handshake_ports = "start=ap_start ready=ap_ready done=ap_done idle=ap_idle";
    let scalar_str = if scalar_ports.is_empty() {
        String::new()
    } else {
        format!(" scalar=({})", scalar_ports.join("|"))
    };
    fsm_module.add_comment(format!("ap-ctrl {handshake_ports}{scalar_str}"));

    // Per-instance ap-ctrl pragmas
    for (inst_name, is_autorun) in instances {
        let sig = InstanceSignals::new(inst_name, *is_autorun);
        let port_map = if *is_autorun {
            format!("{HANDSHAKE_START}={}", sig.start_name())
        } else {
            format!(
                "{HANDSHAKE_START}={} ready={} done={} idle={}",
                sig.start_name(),
                sig.ready_name(),
                sig.done_name(),
                sig.idle_name()
            )
        };
        fsm_module.add_comment(format!("ap-ctrl {port_map}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tapa_rtl::VerilogModule;

    fn empty_fsm_module() -> MutableModule {
        let source = "module test_fsm (\n  input wire ap_clk,\n  input wire ap_rst_n\n);\nendmodule";
        MutableModule::from_parsed(VerilogModule::parse(source).unwrap())
    }

    #[test]
    fn adds_clock_reset_pragmas() {
        let mut fsm = empty_fsm_module();
        add_rs_pragmas_to_fsm(&mut fsm, &[], &[]);
        let emitted = fsm.emit();
        assert!(emitted.contains("clk port=ap_clk"), "got:\n{emitted}");
        assert!(emitted.contains("rst port=ap_rst_n active=low"), "got:\n{emitted}");
    }

    #[test]
    fn adds_main_ap_ctrl() {
        let mut fsm = empty_fsm_module();
        add_rs_pragmas_to_fsm(&mut fsm, &[], &[]);
        let emitted = fsm.emit();
        assert!(emitted.contains("ap-ctrl start=ap_start"), "got:\n{emitted}");
    }

    #[test]
    fn adds_scalar_ports_to_pragma() {
        let mut fsm = empty_fsm_module();
        add_rs_pragmas_to_fsm(&mut fsm, &["offset_a".into(), "size_b".into()], &[]);
        let emitted = fsm.emit();
        assert!(emitted.contains("scalar=(offset_a|size_b)"), "got:\n{emitted}");
    }

    #[test]
    fn adds_per_instance_pragmas() {
        let mut fsm = empty_fsm_module();
        add_rs_pragmas_to_fsm(
            &mut fsm,
            &[],
            &[
                ("child_0".into(), false),
                ("auto_1".into(), true),
            ],
        );
        let emitted = fsm.emit();
        assert!(emitted.contains("start=child_0__ap_start"), "got:\n{emitted}");
        assert!(emitted.contains("done=child_0__ap_done"), "got:\n{emitted}");
        // Autorun instance should only have start
        assert!(emitted.contains("start=auto_1__ap_start"), "got:\n{emitted}");
    }
}
