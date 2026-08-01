//! HLS-artifact cleanup: the first mutation stage each upper task runs.

use tapa_protocol::{
    HANDSHAKE_DONE, HANDSHAKE_IDLE, HANDSHAKE_READY, HANDSHAKE_RST, HANDSHAKE_RST_N,
};
use tapa_rtl::builder::{ContinuousAssign, Expr};

use super::distributed_control::{self, DistributedControlPlan};
use crate::state::views::{DesignView, ModuleTable};

/// Strip HLS artifacts from `task_name`'s module: clear the cached body text,
/// demote HLS `reg`s back to wires, normalize the fabric reset, and (top task
/// only) drop the istream peek ports the HLS module declares.
pub(super) fn cleanup_hls_artifacts(
    design: DesignView<'_>,
    modules: &mut ModuleTable<'_>,
    task_name: &str,
    is_top_task: bool,
    control_plan: Option<&DistributedControlPlan>,
) {
    let task = &design.design().tasks[task_name];

    if let Some(mm) = modules.get_mut(task_name) {
        mm.cleanup_hls_artifacts();
        mm.body_text.clear();
        mm.demote_output_port_regs_to_wires();
        mm.demote_signal_regs_to_wires(&[HANDSHAKE_DONE, HANDSHAKE_IDLE, HANDSHAKE_READY]);
        let _ = mm.add_signal(distributed_control::fabric_reset_signal(
            control_plan.is_some(),
        ));
        let reset_n = control_plan
            .as_ref()
            .map_or(HANDSHAKE_RST_N, |_| distributed_control::FABRIC_RESET_N);
        mm.add_assign(ContinuousAssign::new(
            Expr::ident(HANDSHAKE_RST),
            Expr::logical_not(Expr::ident(reset_n)),
        ));

        if is_top_task {
            // Collect istream/istreams port name prefixes from topology
            // For istream: peek prefix is "{name}_peek"
            // For istreams: peek prefixes are "{name}_{idx}_peek" for each channel
            let mut istream_prefixes: Vec<String> = Vec::new();
            for p in &task.ports {
                if p.cat.is_input_stream() {
                    // Istreams (plural) gets per-channel peek prefixes;
                    // Istream gets just the base.
                    if p.cat == tapa_ir::port::ArgCategory::Istreams {
                        let chan_count = p.chan_count.unwrap_or(1);
                        for idx in 0..chan_count {
                            istream_prefixes.push(format!("{}_{idx}_peek", p.name));
                        }
                    }
                    istream_prefixes.push(format!("{}_peek", p.name));
                }
            }

            // Remove only peek ports derived from istream definitions
            let peek_ports: Vec<String> = mm
                .inner
                .ports
                .iter()
                .filter(|p| {
                    istream_prefixes
                        .iter()
                        .any(|prefix| p.name.starts_with(prefix.as_str()))
                })
                .map(|p| p.name.clone())
                .collect();
            for port_name in peek_ports {
                mm.remove_port(&port_name);
            }
        }
    }
}
