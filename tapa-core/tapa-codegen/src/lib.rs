//! RTL code generation from the TAPA topology model.
//!
//! It uses the `tapa-rtl` builder API to construct Verilog fragments and the
//! hybrid mutation API to modify existing HLS modules.

pub mod async_mmap;
pub mod children;
pub mod error;
pub mod fifos;
pub mod fsm;
pub mod instance_signals;
pub mod m_axi;
pub mod program;
pub mod rtl_state;
mod s_axi;
pub mod support_assets;

use tapa_rtl::builder::{ContinuousAssign, Expr};
use tapa_rtl::mutation::wire;
use tapa_task_graph::task::TaskLevel;

use crate::error::CodegenError;
use crate::rtl_state::TopologyWithRtl;
use tapa_protocol::{
    HANDSHAKE_DONE, HANDSHAKE_IDLE, HANDSHAKE_READY, HANDSHAKE_RST, HANDSHAKE_RST_N,
};

fn render_template_module(name: &str, ports: &[String]) -> String {
    let mut env = minijinja::Environment::new();
    env.add_template(
        "template_module",
        include_str!("templates/template_module.v.j2"),
    )
    .expect("template parses");
    env.get_template("template_module")
        .expect("template exists")
        .render(minijinja::context! { name, ports })
        .expect("render succeeds")
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
/// 7. Generate FSM pragmas
/// 8. Generate global FSM
///
/// Returns the modified modules and any generated auxiliary files.
pub fn generate_rtl(state: &mut TopologyWithRtl) -> Result<(), CodegenError> {
    let task_names: Vec<String> = state.program.tasks.keys().cloned().collect();

    for task_name in &task_names {
        let task = &state.program.tasks[task_name];
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
            .program
            .tasks
            .get(name.as_str())
            .is_some_and(|task| task.level == TaskLevel::Upper)
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
    let is_top_task = task_name == state.program.top;
    let task = &state.program.tasks[task_name];

    // Check if this is a template task (no child instances)
    let is_template = task.tasks.is_empty();

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
            // `add_comment` prepends `// pragma RS ` at emit time, so
            // callers pass raw pragma content without the `RS ` prefix.
            mm.add_comment("clk port=ap_clk".to_owned());
            mm.add_comment("rst port=ap_rst_n active=low".to_owned());

            // Collect istream/istreams port name prefixes from topology
            // For istream: peek prefix is "{name}_peek"
            // For istreams: peek prefixes are "{name}_{idx}_peek" for each channel
            let mut istream_prefixes: Vec<String> = Vec::new();
            for p in &task.ports {
                match p.cat {
                    tapa_task_graph::port::ArgCategory::Istream => {
                        istream_prefixes.push(format!("{}_peek", p.name));
                    }
                    tapa_task_graph::port::ArgCategory::Istreams => {
                        let chan_count = p.chan_count.unwrap_or(1);
                        for idx in 0..chan_count {
                            istream_prefixes.push(format!("{}_{idx}_peek", p.name));
                        }
                        // Also add the base name in case of single-channel
                        istream_prefixes.push(format!("{}_peek", p.name));
                    }
                    tapa_task_graph::port::ArgCategory::Ostream
                    | tapa_task_graph::port::ArgCategory::Ostreams
                    | tapa_task_graph::port::ArgCategory::Scalar
                    | tapa_task_graph::port::ArgCategory::Mmap
                    | tapa_task_graph::port::ArgCategory::AsyncMmap
                    | tapa_task_graph::port::ArgCategory::Immap
                    | tapa_task_graph::port::ArgCategory::Ommap => {}
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

    // Template task: emit port-declaration-only template, NO FSM module
    if is_template {
        if let Some(mm) = state.module_map.get_mut(task_name) {
            // Build port-declaration-only template (just the module shell)
            let ports: Vec<String> = mm
                .inner
                .ports
                .iter()
                .map(std::string::ToString::to_string)
                .collect();
            let template = render_template_module(&mm.inner.name, &ports);
            state
                .generated_files
                .insert(format!("{task_name}_template.v"), template);
        }
        return Ok(());
    }

    state.create_fsm_module(task_name)?;

    // Pre-compute M-AXI slave indices for crossbar-connected mmaps
    // This maps (parent_arg, child_task, inst_idx) -> slave_idx
    let mut mmap_slave_map: std::collections::BTreeMap<(String, String, usize), usize> =
        std::collections::BTreeMap::new();
    let mut mmap_channel_map: std::collections::BTreeMap<(String, String, usize), usize> =
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
        } else if conn.channel_count() > 1 {
            for (channel_idx, slave) in conn.slaves.iter().enumerate() {
                #[allow(clippy::cast_possible_truncation, reason = "index fits")]
                let idx_usize = slave.inst_idx as usize;
                mmap_channel_map.insert(
                    (conn.arg_name.clone(), slave.task.clone(), idx_usize),
                    channel_idx,
                );
            }
        }
    }

    let (is_done_signals, instance_infos) = children::generate_child_signals(
        state,
        task_name,
        &mmap_conns,
        &mmap_slave_map,
        &mmap_channel_map,
    );

    fifos::instantiate_fifos(state, task_name);

    fifos::connect_fifos(state, task_name);

    // Add M-AXI ports and crossbars (reuse pre-computed mmap connections)
    m_axi::add_m_axi_and_crossbars(state, task_name, &mmap_conns)?;

    // Add FSM pragmas
    let scalar_ports: Vec<String> = state.program.tasks[task_name]
        .ports
        .iter()
        .flat_map(|p| {
            use tapa_task_graph::port::ArgCategory;
            match p.cat {
                ArgCategory::Scalar => vec![p.name.clone()],
                ArgCategory::Mmap
                | ArgCategory::AsyncMmap
                | ArgCategory::Immap
                | ArgCategory::Ommap => {
                    let sanitized = tapa_rtl::module::sanitize_array_name(&p.name);
                    if let Some(chan_count) = p.chan_count {
                        (0..chan_count)
                            .map(|idx| format!("{sanitized}_{idx}_offset"))
                            .collect()
                    } else {
                        vec![format!("{sanitized}_offset")]
                    }
                }
                ArgCategory::Istream
                | ArgCategory::Ostream
                | ArgCategory::Istreams
                | ArgCategory::Ostreams => Vec::new(),
            }
        })
        .collect();

    if let Some(fsm_mm) = state.fsm_modules.get_mut(task_name) {
        fsm::add_rs_pragmas_to_fsm(fsm_mm, &scalar_ports, &instance_infos);
        program::apply_global_fsm(fsm_mm, &is_done_signals);
    }

    if is_top_task {
        s_axi::instantiate_top_control_s_axi(state, task_name);
    }

    Ok(())
}

#[cfg(test)]
#[path = "generate_rtl_tests.rs"]
mod tests;
