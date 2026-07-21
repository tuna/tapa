//! RTL code generation from the TAPA topology model.
//!
//! It uses the `tapa-rtl` builder API to construct Verilog fragments and the
//! hybrid mutation API to modify existing HLS modules.

pub mod async_mmap;
pub mod children;
pub mod error;
pub mod fifos;
pub mod instance_signals;
pub mod m_axi;
pub mod program;
pub mod rtl_state;
mod s_axi;
pub mod support_assets;
mod template;

use tapa_ir::task::TaskLevel;
use tapa_ir::{SynthTarget, Target};
use tapa_rtl::builder::{ContinuousAssign, Expr};
use tapa_rtl::mutation::wire;

use crate::error::CodegenError;
use crate::rtl_state::TopologyWithRtl;
use tapa_protocol::{
    HANDSHAKE_DONE, HANDSHAKE_IDLE, HANDSHAKE_READY, HANDSHAKE_RST, HANDSHAKE_RST_N,
};

/// Vendor-flow codegen policy.
///
/// This is the **single place** in `tapa-codegen` that branches on the
/// vendor flow ([`Target`]). Today only one decision differs across
/// vendors: whether the top task's external stream FIFOs need a
/// Vitis-style AXIS adapter at the module boundary. The exhaustive
/// `match` makes adding a [`Target`] variant a compile error here.
///
/// When a second vendor needs more than this one boolean, promote this
/// to a `Backend` trait implemented per vendor (the trait surface would
/// then be shaped against the real second vendor's codegen deltas, per
/// the "shape against a real vendor" principle).
#[must_use]
pub fn top_stream_needs_axis_adapter(target: Target) -> bool {
    match target {
        Target::XilinxVitis => true,
        Target::XilinxHls => false,
    }
}

/// Run the full RTL codegen orchestration pipeline.
///
/// For each upper-level task:
/// 1. Clean up HLS artifacts
/// 2. Create FSM module
/// 3. Generate instance signals for child instances
/// 4. Instantiate FIFOs
/// 5. Instantiate child tasks with FSM/port wiring
/// 6. Add M-AXI ports
/// 7. Generate global FSM
///
/// Returns the modified modules and any generated auxiliary files.
pub fn generate_rtl(state: &mut TopologyWithRtl) -> Result<(), CodegenError> {
    let task_names: Vec<String> = state.design.tasks.keys().cloned().collect();

    // Ignored tasks have no HLS result to attach. Build their authoritative
    // port-only shell from topology so parents can resolve the module while
    // the user authors the replacement RTL.
    for task_name in &task_names {
        let task = &state.design.tasks[task_name];
        if task.synth != SynthTarget::Ignore {
            continue;
        }
        let source = template::render_task_template(task_name, task);
        let module = tapa_rtl::VerilogModule::parse(&source)?;
        state.module_map.insert(
            task_name.clone(),
            tapa_rtl::mutation::MutableModule::from_parsed(module),
        );
    }

    for task_name in &task_names {
        let task = &state.design.tasks[task_name];
        if task.synth == SynthTarget::Ignore {
            let template = state.module_map[task_name].emit();
            state
                .template_files
                .insert(format!("{task_name}.v"), template);
            continue;
        }
        if task.level != TaskLevel::Upper {
            continue;
        }
        instrument_upper_task(state, task_name)?;
    }

    // Collect emitted files. Lower HLS modules were already copied from
    // their original Verilog sources by the CLI; re-emitting them from the
    // parsed model drops legal port-reg redeclarations used by HLS.
    for (name, mm) in &state.module_map {
        if state
            .design
            .tasks
            .get(name.as_str())
            .is_some_and(|task| task.level == TaskLevel::Upper || task.synth == SynthTarget::Ignore)
        {
            state.generated_files.insert(format!("{name}.v"), mm.emit());
        }
    }
    for (name, mm) in &state.fsm_modules {
        state
            .generated_files
            .insert(format!("{name}_fsm.v"), mm.emit());
    }

    Ok(())
}

/// Instrument a single upper-level task with codegen logic.
#[allow(clippy::too_many_lines, reason = "sequential orchestration logic")]
fn instrument_upper_task(state: &mut TopologyWithRtl, task_name: &str) -> Result<(), CodegenError> {
    let is_top_task = task_name == state.design.top;
    let task = &state.design.tasks[task_name];

    // Reject malformed memory geometry before mutating any RTL state.
    let mmap_conns = state.aggregate_mmap_connections(task_name)?;
    for conn in mmap_conns.values() {
        m_axi::validate_mmap_connection(conn)?;
    }

    if let Some(mm) = state.module_map.get_mut(task_name) {
        mm.cleanup_hls_artifacts();
        mm.body_text.clear();
        mm.demote_output_port_regs_to_wires();
        mm.demote_signal_regs_to_wires(&[HANDSHAKE_DONE, HANDSHAKE_IDLE, HANDSHAKE_READY]);
        let _ = mm.add_signal(wire(HANDSHAKE_RST));
        mm.add_assign(ContinuousAssign::new(
            Expr::ident(HANDSHAKE_RST),
            Expr::logical_not(Expr::ident(HANDSHAKE_RST_N)),
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

    state.create_fsm_module(task_name)?;

    // Pre-compute M-AXI slave indices for crossbar-connected mmaps
    // This maps (parent_arg, child_task, inst_idx) -> slave_idx
    let mut mmap_slave_map: std::collections::BTreeMap<(String, String, usize), usize> =
        std::collections::BTreeMap::new();
    for conn in mmap_conns.values() {
        if m_axi::needs_crossbar(conn) {
            for (slave_idx, slave) in conn.slaves.iter().enumerate() {
                #[allow(clippy::cast_possible_truncation, reason = "index fits")]
                let idx_usize = slave.inst_idx as usize;
                mmap_slave_map.insert(
                    (conn.arg_name.clone(), slave.task.clone(), idx_usize),
                    slave_idx,
                );
            }
        }
    }

    let is_done_signals =
        children::generate_child_signals(state, task_name, &mmap_conns, &mmap_slave_map);

    fifos::instantiate_fifos(state, task_name)?;

    fifos::connect_fifos(state, task_name)?;

    // Add M-AXI ports and crossbars (reuse pre-computed mmap connections)
    m_axi::add_m_axi_and_crossbars(state, task_name, &mmap_conns)?;

    if let Some(fsm_mm) = state.fsm_modules.get_mut(task_name) {
        program::apply_global_fsm(fsm_mm, &is_done_signals);
    }

    if is_top_task {
        s_axi::instantiate_top_control_s_axi(state, task_name);
    }

    Ok(())
}

/// Test-only: parse a [`tapa_ir::Design`] from fixture JSON.
#[cfg(test)]
pub(crate) fn design_from_fixture_json(value: serde_json::Value) -> tapa_ir::Design {
    serde_json::from_value(value).expect("valid design fixture JSON")
}

#[cfg(test)]
#[path = "generate_rtl_tests.rs"]
mod tests;
