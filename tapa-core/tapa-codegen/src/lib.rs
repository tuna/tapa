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
pub mod support_assets;

use tapa_rtl::builder::{ContinuousAssign, Expr, ParamArg, PortArg};
use tapa_rtl::mutation::{wide_wire, wire};
use tapa_task_graph::task::TaskLevel;

use crate::error::CodegenError;
use crate::rtl_state::TopologyWithRtl;

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

    if let Some(mm) = state.module_map.get_mut(task_name) {
        mm.cleanup_hls_artifacts();
        mm.body_text.clear();
        mm.demote_output_port_regs_to_wires();
        mm.demote_signal_regs_to_wires(&["ap_done", "ap_idle", "ap_ready"]);
        let _ = mm.add_signal(wire("ap_rst"));
        mm.add_assign(ContinuousAssign::new(
            Expr::ident("ap_rst"),
            Expr::logical_not(Expr::ident("ap_rst_n")),
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
            use std::fmt::Write;
            let mut template = String::new();
            let _ = writeln!(template, "module {} (", mm.inner.name);
            for (i, port) in mm.inner.ports.iter().enumerate() {
                let comma = if i + 1 < mm.inner.ports.len() {
                    ","
                } else {
                    ""
                };
                let _ = writeln!(template, "  {port}{comma}");
            }
            template.push_str(");\nendmodule\n");
            state
                .generated_files
                .insert(format!("{task_name}_template.v"), template);
        }
        return Ok(());
    }

    state.create_fsm_module(task_name)?;

    // Pre-compute M-AXI slave indices for crossbar-connected mmaps
    // This maps (parent_arg, child_task, inst_idx) -> slave_idx
    let mmap_conns = state.aggregate_mmap_connections(task_name)?;
    let mut mmap_slave_map: std::collections::BTreeMap<(String, String, usize), usize> =
        std::collections::BTreeMap::new();
    let mut mmap_channel_map: std::collections::BTreeMap<(String, String, usize), usize> =
        std::collections::BTreeMap::new();
    for conn in mmap_conns.values() {
        if m_axi::needs_crossbar(conn) {
            for (slave_idx, (task, idx, _port)) in conn.args.iter().enumerate() {
                #[allow(clippy::cast_possible_truncation, reason = "index fits")]
                let idx_usize = *idx as usize;
                mmap_slave_map.insert((conn.arg_name.clone(), task.clone(), idx_usize), slave_idx);
            }
        } else if conn.chan_count > 1 {
            for (channel_idx, (task, idx, _port)) in conn.args.iter().enumerate() {
                #[allow(clippy::cast_possible_truncation, reason = "index fits")]
                let idx_usize = *idx as usize;
                mmap_channel_map.insert(
                    (conn.arg_name.clone(), task.clone(), idx_usize),
                    channel_idx,
                );
            }
        }
    }

    let (is_done_signals, instance_infos) = generate_child_signals(
        state,
        task_name,
        &mmap_conns,
        &mmap_slave_map,
        &mmap_channel_map,
    );

    instantiate_fifos(state, task_name);

    connect_fifos(state, task_name);

    // Add M-AXI ports and crossbars (reuse pre-computed mmap connections)
    add_m_axi_and_crossbars(state, task_name, &mmap_conns)?;

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
        instantiate_top_control_s_axi(state, task_name);
    }

    Ok(())
}

/// Generate child instance signals, FSM/autorun logic, and actual child module instances.
///
/// FSM/start logic goes into the FSM module (not the parent task module).
/// The FSM module is then instantiated into the parent task module.
#[allow(clippy::too_many_lines, reason = "sequential child signal generation")]
fn generate_child_signals(
    state: &mut TopologyWithRtl,
    task_name: &str,
    mmap_conns: &std::collections::BTreeMap<String, crate::rtl_state::MMapConnection>,
    mmap_slave_map: &std::collections::BTreeMap<(String, String, usize), usize>,
    mmap_channel_map: &std::collections::BTreeMap<(String, String, usize), usize>,
) -> (Vec<String>, Vec<(String, bool)>) {
    use std::collections::BTreeMap;
    use tapa_topology::instance::ArgDesign;

    type ChildEntry = (usize, Option<String>, bool, BTreeMap<String, ArgDesign>);

    let task = &state.program.tasks[task_name];
    let mut is_done_signals = Vec::new();
    let mut instance_infos = Vec::new();
    let mut fsm_portargs = Vec::new(); // portargs for FSM module instantiation

    let child_entries: Vec<(String, Vec<ChildEntry>)> = task
        .tasks
        .iter()
        .map(|(name, insts)| {
            let entries: Vec<_> = insts
                .iter()
                .enumerate()
                .map(|(idx, inst)| (idx, inst.name.clone(), inst.step < 0, inst.args.clone()))
                .collect();
            (name.clone(), entries)
        })
        .collect();

    for (child_name, entries) in child_entries {
        for (idx, explicit_name, is_autorun, args) in entries {
            let inst_name = explicit_name.map_or_else(
                || format!("{child_name}_{idx}"),
                |name| tapa_rtl::module::sanitize_identifier_name(&name),
            );
            let sig = instance_signals::InstanceSignals::new(&inst_name, is_autorun);

            // Parent task module: only declare handshake WIRES (not state regs)
            // The parent sees start/done/idle/ready/is_done as wires connected to FSM ports
            if let Some(mm) = state.module_map.get_mut(task_name) {
                if is_autorun {
                    let _ = mm.add_signal(tapa_rtl::mutation::wire(sig.start_name()));
                } else {
                    for signal in sig.all_signals() {
                        let _ = mm.add_signal(signal);
                    }
                }
            }

            // FSM module interface: add handshake ports
            if let Some(fsm_mm) = state.fsm_modules.get_mut(task_name) {
                for port in sig.fsm_ports() {
                    let _ = fsm_mm.add_port(port);
                }
            }

            // Build FSM module instantiation portargs using child-specific port names
            // FSM module ports are child-specific (e.g., child_0__ap_start),
            // parent wires have the same names — so both sides match
            for port in sig.fsm_ports() {
                fsm_portargs.push(tapa_rtl::builder::PortArg::new(
                    &port.name,
                    Expr::ident(&port.name),
                ));
            }

            if !is_autorun {
                is_done_signals.push(sig.is_done_name());

                // Add is_done port to FSM module interface
                if let Some(fsm_mm) = state.fsm_modules.get_mut(task_name) {
                    let _ = fsm_mm.add_port(tapa_rtl::mutation::simple_port(
                        sig.is_done_name(),
                        tapa_rtl::port::Direction::Output,
                    ));
                }
                // Add is_done portarg for FSM instantiation
                fsm_portargs.push(tapa_rtl::builder::PortArg::new(
                    sig.is_done_name(),
                    Expr::ident(sig.is_done_name()),
                ));
            }

            // FSM/autorun logic goes into the FSM MODULE
            if let Some(fsm_mm) = state.fsm_modules.get_mut(task_name) {
                if is_autorun {
                    for signal in sig.all_signals() {
                        let _ = fsm_mm.add_signal(signal);
                    }
                    fsm_mm.ensure_signal_kind(&sig.start_name(), tapa_rtl::signal::SignalKind::Reg);
                    fsm_mm.add_always(children::generate_autorun_start(&sig));
                } else {
                    // State register and FSM logic owned by FSM module
                    for signal in sig.all_signals() {
                        let _ = fsm_mm.add_signal(signal);
                    }
                    // Use pipelined start_q/done_q from global FSM
                    let start_input = Expr::ident(program::START_Q);
                    let done_release = Expr::ident(program::DONE_Q);
                    fsm_mm.add_always(children::generate_child_fsm(
                        &sig,
                        start_input,
                        done_release,
                    ));
                    // ap_start output: combinationally driven from state
                    fsm_mm.add_assign(children::generate_start_assign(&sig));
                    // is_done: driven from state inside FSM module
                    fsm_mm.add_assign(children::generate_is_done_assign(&sig));
                }
            }

            // Declare per-instance pipeline signals for scalar/mmap args
            declare_instance_pipeline_signals(state, task_name, &child_name, &inst_name, &args);

            // Add pipeline portargs to FSM instantiation
            for (port_name, arg) in &args {
                match arg.cat {
                    tapa_task_graph::port::ArgCategory::Scalar => {
                        let pipeline_out = format!("{inst_name}__{port_name}");
                        let fsm_in_port = format!("{inst_name}__{port_name}_in");
                        fsm_portargs.push(tapa_rtl::builder::PortArg::new(
                            &fsm_in_port,
                            Expr::ident(tapa_rtl::module::sanitize_array_name(&arg.arg)),
                        ));
                        fsm_portargs.push(tapa_rtl::builder::PortArg::new(
                            &pipeline_out,
                            Expr::ident(&pipeline_out),
                        ));
                    }
                    tapa_task_graph::port::ArgCategory::Mmap
                    | tapa_task_graph::port::ArgCategory::AsyncMmap => {
                        let pipeline_out = format!("{inst_name}__{port_name}_offset");
                        let fsm_in_port = format!("{inst_name}__{port_name}_offset_in");
                        let arg_name = tapa_rtl::module::sanitize_array_name(&arg.arg);
                        let offset_source = mmap_conns.get(&arg.arg).map_or_else(
                            || Expr::ident(format!("{arg_name}_offset")),
                            |conn| {
                                if conn.chan_count > 1 && m_axi::needs_crossbar(conn) {
                                    Expr::lit("64'd0")
                                } else if conn.chan_count > 1 {
                                    Expr::ident(format!("{arg_name}_0_offset"))
                                } else {
                                    Expr::ident(format!("{arg_name}_offset"))
                                }
                            },
                        );
                        fsm_portargs
                            .push(tapa_rtl::builder::PortArg::new(&fsm_in_port, offset_source));
                        fsm_portargs.push(tapa_rtl::builder::PortArg::new(
                            &pipeline_out,
                            Expr::ident(&pipeline_out),
                        ));
                    }
                    tapa_task_graph::port::ArgCategory::Istream
                    | tapa_task_graph::port::ArgCategory::Ostream
                    | tapa_task_graph::port::ArgCategory::Istreams
                    | tapa_task_graph::port::ArgCategory::Ostreams
                    | tapa_task_graph::port::ArgCategory::Immap
                    | tapa_task_graph::port::ArgCategory::Ommap => {}
                }
            }

            // Build per-instance mmap slave index map for crossbar routing
            let mut mmap_bindings = children::ChildMmapBindings::default();
            let child_rtl = state
                .module_map
                .get(&child_name)
                .map(|module| module.inner.clone());
            for (child_port, arg) in &args {
                if matches!(
                    arg.cat,
                    tapa_task_graph::port::ArgCategory::Mmap
                        | tapa_task_graph::port::ArgCategory::AsyncMmap
                ) {
                    if let Some(&slave_idx) =
                        mmap_slave_map.get(&(arg.arg.clone(), child_name.clone(), idx))
                    {
                        mmap_bindings
                            .slave_indices
                            .insert(arg.arg.clone(), slave_idx);
                        if let Some(conn) = mmap_conns.get(&arg.arg) {
                            mmap_bindings
                                .wire_id_widths
                                .insert(arg.arg.clone(), m_axi::crossbar_slave_id_width(conn));
                        }
                        mmap_bindings.child_id_widths.insert(
                            arg.arg.clone(),
                            state.child_mmap_id_width(&child_name, child_port),
                        );
                    }
                    if let Some(&channel_idx) =
                        mmap_channel_map.get(&(arg.arg.clone(), child_name.clone(), idx))
                    {
                        mmap_bindings
                            .channel_indices
                            .insert(arg.arg.clone(), channel_idx);
                    }
                }
            }

            // Build and add the actual child module instance to parent
            if let Some(child_rtl) = child_rtl.as_ref() {
                for (child_port, arg) in &args {
                    if !matches!(arg.cat, tapa_task_graph::port::ArgCategory::AsyncMmap) {
                        continue;
                    }
                    let active_tags = async_mmap::active_tags(child_rtl, child_port);
                    if active_tags.is_empty() {
                        continue;
                    }
                    let m_axi_wire_prefix = mmap_bindings.wire_prefix(&arg.arg);
                    let bridge_base = async_mmap::bridge_base_from_m_axi_prefix(&m_axi_wire_prefix);
                    let data_width = resolve_mmap_data_width(
                        state,
                        task_name,
                        &child_name,
                        &arg.arg,
                        child_port,
                    );
                    let connect_optional_axi_ports =
                        !mmap_bindings.slave_indices.contains_key(&arg.arg);
                    if let Some(mm) = state.module_map.get_mut(task_name) {
                        async_mmap::add_bridge_signals(mm, &bridge_base, &active_tags, data_width);
                        mm.add_instance(async_mmap::build_bridge_instance(
                            &bridge_base,
                            &m_axi_wire_prefix,
                            &active_tags,
                            data_width,
                            connect_optional_axi_ports,
                        ));
                    }
                }
            }
            let child_inst = children::build_child_instance(
                &child_name,
                &inst_name,
                &sig,
                &args,
                &mmap_bindings,
                child_rtl.as_ref(),
            );
            if let Some(mm) = state.module_map.get_mut(task_name) {
                mm.add_instance(child_inst);
            }
            if child_rtl.is_some() {
                add_crossbar_slave_id_padding(state, task_name, &args, &mmap_bindings);
            }

            instance_infos.push((inst_name, is_autorun));
        }
    }

    // Instantiate FSM module into parent task module
    let fsm_module_name = format!("{task_name}_fsm");
    let mut fsm_inst_ports = vec![
        tapa_rtl::builder::PortArg::new("ap_clk", Expr::ident("ap_clk")),
        tapa_rtl::builder::PortArg::new("ap_rst_n", Expr::ident("ap_rst_n")),
        // Top-level handshake ports
        tapa_rtl::builder::PortArg::new("ap_start", Expr::ident("ap_start")),
        tapa_rtl::builder::PortArg::new("ap_done", Expr::ident("ap_done")),
        tapa_rtl::builder::PortArg::new("ap_idle", Expr::ident("ap_idle")),
        tapa_rtl::builder::PortArg::new("ap_ready", Expr::ident("ap_ready")),
    ];
    // Add child-specific handshake portargs (child_0__ap_start, etc.)
    fsm_inst_ports.extend(fsm_portargs);
    let fsm_inst = tapa_rtl::builder::ModuleInstance::new(&fsm_module_name, "__tapa_fsm_unit")
        .with_ports(fsm_inst_ports);
    if let Some(mm) = state.module_map.get_mut(task_name) {
        mm.add_instance(fsm_inst);
    }

    (is_done_signals, instance_infos)
}

fn resolve_mmap_data_width(
    state: &TopologyWithRtl,
    parent_task_name: &str,
    child_task_name: &str,
    parent_arg_name: &str,
    child_port_name: &str,
) -> u32 {
    state
        .program
        .tasks
        .get(parent_task_name)
        .and_then(|task| task.ports.iter().find(|p| p.name == parent_arg_name))
        .or_else(|| {
            state
                .program
                .tasks
                .get(child_task_name)
                .and_then(|task| task.ports.iter().find(|p| p.name == child_port_name))
        })
        .map_or(64, |p| p.width)
}

fn add_crossbar_slave_id_padding(
    state: &mut TopologyWithRtl,
    task_name: &str,
    args: &std::collections::BTreeMap<String, tapa_topology::instance::ArgDesign>,
    mmap_bindings: &children::ChildMmapBindings,
) {
    let mut assigns = Vec::new();
    for arg in args.values() {
        if !matches!(
            arg.cat,
            tapa_task_graph::port::ArgCategory::Mmap
                | tapa_task_graph::port::ArgCategory::AsyncMmap
        ) {
            continue;
        }
        let Some(&slave_idx) = mmap_bindings.slave_indices.get(&arg.arg) else {
            continue;
        };
        let Some(target_width) = mmap_bindings.wire_id_width(&arg.arg) else {
            continue;
        };
        let Some(child_width) = mmap_bindings.child_id_width(&arg.arg) else {
            continue;
        };
        if child_width >= target_width {
            continue;
        }
        let wire_prefix = m_axi::crossbar_slave_prefix(&arg.arg, slave_idx);
        for suffix in ["_ARID", "_AWID"] {
            let wire_name = format!("{wire_prefix}{suffix}");
            assigns.push(ContinuousAssign::new(
                Expr::range(
                    Expr::ident(wire_name),
                    Expr::int(u64::from(target_width - 1)),
                    Expr::int(u64::from(child_width)),
                ),
                Expr::int_const(target_width - child_width, 0),
            ));
        }
    }
    if let Some(mm) = state.module_map.get_mut(task_name) {
        for assign in assigns {
            mm.add_assign(assign);
        }
    }
}

const S_AXI_CTRL_PORTS: &[&str] = &[
    "AWVALID", "AWREADY", "AWADDR", "WVALID", "WREADY", "WDATA", "WSTRB", "ARVALID", "ARREADY",
    "ARADDR", "RVALID", "RREADY", "RDATA", "RRESP", "BVALID", "BREADY", "BRESP",
];

fn instantiate_top_control_s_axi(state: &mut TopologyWithRtl, task_name: &str) {
    let Some(task) = state.program.tasks.get(task_name) else {
        return;
    };
    let Some(mm) = state.module_map.get_mut(task_name) else {
        return;
    };
    if !mm
        .inner
        .ports
        .iter()
        .any(|p| p.name == "s_axi_control_AWVALID")
    {
        return;
    }

    let mut ports = vec![
        PortArg::new("ACLK", Expr::ident("ap_clk")),
        PortArg::new("ARESET", Expr::ident("ap_rst")),
        PortArg::new("ACLK_EN", Expr::lit("1'b1")),
    ];
    for &axi_port in S_AXI_CTRL_PORTS {
        ports.push(PortArg::new(
            axi_port,
            Expr::ident(format!("s_axi_control_{axi_port}")),
        ));
    }
    for &sig in &["ap_start", "ap_done", "ap_idle", "ap_ready", "interrupt"] {
        ports.push(PortArg::new(sig, Expr::ident(sig)));
    }

    for port in &task.ports {
        use tapa_task_graph::port::ArgCategory;
        let sanitized = tapa_rtl::module::sanitize_array_name(&port.name);
        let ctrl_port_names = match port.cat {
            ArgCategory::Scalar => {
                let width = port.width.max(1);
                if width == 1 {
                    let _ = mm.add_signal(wire(&sanitized));
                } else {
                    let _ = mm.add_signal(wide_wire(&sanitized, &(width - 1).to_string(), "0"));
                }
                vec![sanitized]
            }
            ArgCategory::Mmap
            | ArgCategory::AsyncMmap
            | ArgCategory::Immap
            | ArgCategory::Ommap => {
                if let Some(chan_count) = port.chan_count {
                    (0..chan_count)
                        .map(|idx| {
                            let name = format!("{sanitized}_{idx}_offset");
                            let _ = mm.add_signal(wide_wire(&name, "63", "0"));
                            name
                        })
                        .collect()
                } else {
                    let name = format!("{sanitized}_offset");
                    let _ = mm.add_signal(wide_wire(&name, "63", "0"));
                    vec![name]
                }
            }
            ArgCategory::Istream
            | ArgCategory::Ostream
            | ArgCategory::Istreams
            | ArgCategory::Ostreams => Vec::new(),
        };
        for ctrl_port_name in ctrl_port_names {
            ports.push(PortArg::new(
                ctrl_port_name.clone(),
                Expr::ident(ctrl_port_name),
            ));
        }
    }

    let inst = tapa_rtl::builder::ModuleInstance::new(
        format!("{task_name}_control_s_axi"),
        "control_s_axi_U",
    )
    .with_params(vec![
        ParamArg::new(
            "C_S_AXI_ADDR_WIDTH",
            Expr::ident("C_S_AXI_CONTROL_ADDR_WIDTH"),
        ),
        ParamArg::new(
            "C_S_AXI_DATA_WIDTH",
            Expr::ident("C_S_AXI_CONTROL_DATA_WIDTH"),
        ),
    ])
    .with_ports(ports);
    mm.add_instance(inst);
}

/// Declare per-instance pipeline signals for scalar and mmap arguments.
///
/// Creates FSM-owned pipeline ports and parent-side wires:
/// - FSM module gets an input port for the parent arg and an output port for the pipeline signal
/// - Parent module gets a wire for the pipeline output
/// - The FSM module uses a registered pipeline (always @(posedge clk)) to
///   delay the signal by one cycle, matching `add_pipeline` behavior
fn declare_instance_pipeline_signals(
    state: &mut TopologyWithRtl,
    task_name: &str,
    child_name: &str,
    inst_name: &str,
    args: &std::collections::BTreeMap<String, tapa_topology::instance::ArgDesign>,
) {
    for (port_name, arg) in args {
        let (pipeline_out, fsm_in, width) = match arg.cat {
            tapa_task_graph::port::ArgCategory::Scalar => (
                format!("{inst_name}__{port_name}"),
                format!("{inst_name}__{port_name}_in"),
                resolve_child_scalar_width(state, child_name, port_name),
            ),
            tapa_task_graph::port::ArgCategory::Mmap
            | tapa_task_graph::port::ArgCategory::AsyncMmap => (
                format!("{inst_name}__{port_name}_offset"),
                format!("{inst_name}__{port_name}_offset_in"),
                Some(("63".to_string(), "0".to_string())), // 64-bit
            ),
            tapa_task_graph::port::ArgCategory::Istream
            | tapa_task_graph::port::ArgCategory::Ostream
            | tapa_task_graph::port::ArgCategory::Istreams
            | tapa_task_graph::port::ArgCategory::Ostreams
            | tapa_task_graph::port::ArgCategory::Immap
            | tapa_task_graph::port::ArgCategory::Ommap => continue,
        };
        add_pipeline_stage(state, task_name, &pipeline_out, &fsm_in, width);
    }
}

/// Add a registered pipeline stage: parent wire + FSM input/output ports + internal reg.
fn add_pipeline_stage(
    state: &mut TopologyWithRtl,
    task_name: &str,
    pipeline_out: &str,
    fsm_in_port: &str,
    width: Option<(String, String)>, // None = 1-bit, Some((msb, lsb))
) {
    // Parent: wire for pipeline output
    if let Some(mm) = state.module_map.get_mut(task_name) {
        let sig = match width.as_ref() {
            Some((msb, lsb)) => tapa_rtl::mutation::wide_wire(pipeline_out, msb, lsb),
            None => tapa_rtl::mutation::wire(pipeline_out),
        };
        let _ = mm.add_signal(sig);
    }

    // FSM module: input port + output port + internal _reg + registered always block
    if let Some(fsm_mm) = state.fsm_modules.get_mut(task_name) {
        let reg_name = format!("{pipeline_out}_reg");
        let (in_port, out_port, reg_sig) = match width.as_ref() {
            Some((msb, lsb)) => (
                tapa_rtl::mutation::wide_port(
                    fsm_in_port,
                    tapa_rtl::port::Direction::Input,
                    msb,
                    lsb,
                ),
                tapa_rtl::mutation::wide_port(
                    pipeline_out,
                    tapa_rtl::port::Direction::Output,
                    msb,
                    lsb,
                ),
                tapa_rtl::mutation::wide_reg(&reg_name, msb, lsb),
            ),
            None => (
                tapa_rtl::mutation::simple_port(fsm_in_port, tapa_rtl::port::Direction::Input),
                tapa_rtl::mutation::simple_port(pipeline_out, tapa_rtl::port::Direction::Output),
                tapa_rtl::mutation::reg(&reg_name),
            ),
        };
        let _ = fsm_mm.add_port(in_port);
        let _ = fsm_mm.add_port(out_port);
        let _ = fsm_mm.add_signal(reg_sig);
        fsm_mm.add_always(tapa_rtl::builder::AlwaysBlock::posedge(
            "ap_clk",
            vec![tapa_rtl::builder::Statement::NonblockingAssign {
                lhs: Expr::ident(&reg_name),
                rhs: Expr::ident(fsm_in_port),
            }],
        ));
        fsm_mm.add_assign(ContinuousAssign::new(
            Expr::ident(pipeline_out),
            Expr::ident(&reg_name),
        ));
    }
}

fn resolve_child_scalar_width(
    state: &TopologyWithRtl,
    child_name: &str,
    port_name: &str,
) -> Option<(String, String)> {
    if let Some(module) = state.module_map.get(child_name) {
        if let Some(port) = module.inner.find_port(port_name) {
            return port.width.as_ref().map(|width| {
                (
                    rtl_expr_to_string(&width.msb),
                    rtl_expr_to_string(&width.lsb),
                )
            });
        }
    }

    state
        .program
        .tasks
        .get(child_name)?
        .ports
        .iter()
        .find(|port| {
            port.name == port_name && matches!(port.cat, tapa_task_graph::port::ArgCategory::Scalar)
        })
        .and_then(|port| {
            if port.width > 1 {
                Some(((port.width - 1).to_string(), "0".to_string()))
            } else {
                None
            }
        })
}

fn rtl_expr_to_string(expr: &tapa_rtl::expression::Expression) -> String {
    expr.iter()
        .map(|token| token.repr.as_str())
        .collect::<Vec<_>>()
        .join("")
}

/// Producer endpoint for a FIFO in the parent task.
#[derive(Clone, Debug)]
struct FifoProducer {
    task_name: String,
    port_name: Option<String>,
}

/// FIFO entry: (name, depth, `is_consumed`, `producer_endpoint`).
type FifoEntry = (String, Option<u32>, bool, Option<FifoProducer>);
/// FIFO connection entry: (name, depth, `has_consumer`, `has_producer`, `producer_endpoint`).
type FifoConnEntry = (String, Option<u32>, bool, bool, Option<FifoProducer>);

/// Instantiate FIFOs for a task.
///
/// Internal FIFOs (with depth) get a `fifo` module instance.
/// External FIFOs (no depth) get wire assignments connecting to external ports.
/// FIFO width is resolved from the producer child's attached RTL module ports.
fn instantiate_fifos(state: &mut TopologyWithRtl, task_name: &str) {
    let task = &state.program.tasks[task_name];

    // Collect FIFO info before mutating
    let fifo_entries: Vec<FifoEntry> = task
        .fifos
        .iter()
        .map(|(name, fifo)| {
            let is_consumed = fifo.consumed_by.is_some();
            let producer = fifo_producer_for(task, name, fifo.produced_by.as_ref());
            (name.clone(), fifo.depth, is_consumed, producer)
        })
        .collect();

    for (fifo_name, depth, is_consumed, producer) in fifo_entries {
        if let Some(depth) = depth {
            // Resolve FIFO width from producer child's attached RTL port
            let width = resolve_fifo_width(state, producer.as_ref());
            let fifo_inst = fifos::build_fifo_instance(
                &fifo_name,
                Expr::ident("ap_rst"),
                Expr::int(u64::from(width)),
                depth,
            );
            if let Some(mm) = state.module_map.get_mut(task_name) {
                mm.add_instance(fifo_inst);
            }
        } else {
            // External FIFO: wire assigns if internal/external names differ
            let assigns = fifos::build_external_fifo_assigns(&fifo_name, &fifo_name, is_consumed);
            if let Some(mm) = state.module_map.get_mut(task_name) {
                for assign in assigns {
                    mm.add_assign(assign);
                }
            }
        }
    }
}

fn fifo_producer_for(
    task: &tapa_topology::task::TaskDesign,
    fifo_name: &str,
    endpoint: Option<&tapa_task_graph::interconnect::EndpointRef>,
) -> Option<FifoProducer> {
    endpoint.map(|ep| {
        let port_name = task
            .tasks
            .get(&ep.0)
            .and_then(|instances| instances.get(ep.1 as usize))
            .and_then(|instance| {
                instance
                    .args
                    .iter()
                    .find(|(_, arg)| arg.arg == fifo_name)
                    .map(|(port_name, _)| port_name.clone())
            });
        FifoProducer {
            task_name: ep.0.clone(),
            port_name,
        }
    })
}

/// Resolve FIFO width from the producer child's attached RTL module.
///
/// Looks for the producer's bound stream port on the parsed child RTL
/// and uses its width. Falls back to topology port width, then 32.
fn resolve_fifo_width(state: &TopologyWithRtl, producer: Option<&FifoProducer>) -> u32 {
    if let Some(producer) = producer {
        // Check attached RTL module for producer port width
        if let Some(mm) = state.module_map.get(producer.task_name.as_str()) {
            if let Some(port_name) = producer.port_name.as_deref() {
                for suffix in ["_din", "_dout"] {
                    if let Some(port) = mm.inner.get_port_of(port_name, suffix) {
                        if let Some(width) = verilog_port_width(port) {
                            return width;
                        }
                    }
                }
            } else {
                // Keep the old best-effort behavior for incomplete topology data.
                for port in &mm.inner.ports {
                    if port.name.ends_with("_dout") || port.name.ends_with("_din") {
                        if let Some(width) = verilog_port_width(port) {
                            return width;
                        }
                    }
                }
            }
        }
        // Fallback: check topology port definitions for the producer task
        if let Some(task) = state.program.tasks.get(producer.task_name.as_str()) {
            if let Some(port_name) = producer.port_name.as_deref() {
                if let Some(port) = task.ports.iter().find(|port| {
                    port.name == port_name
                        && matches!(
                            port.cat,
                            tapa_task_graph::port::ArgCategory::Ostream
                                | tapa_task_graph::port::ArgCategory::Ostreams
                        )
                }) {
                    return port.width;
                }
            }
            for port in &task.ports {
                if matches!(
                    port.cat,
                    tapa_task_graph::port::ArgCategory::Ostream
                        | tapa_task_graph::port::ArgCategory::Ostreams
                ) {
                    return port.width;
                }
            }
        }
    }
    32 // Ultimate fallback
}

fn verilog_port_width(port: &tapa_rtl::port::Port) -> Option<u32> {
    let Some(width) = port.width.as_ref() else {
        return Some(1);
    };
    let msb = parse_verilog_u32(&width.msb)?;
    let lsb = parse_verilog_u32(&width.lsb)?;
    Some(msb.saturating_sub(lsb) + 1)
}

fn parse_verilog_u32(expr: &[tapa_rtl::expression::Token]) -> Option<u32> {
    if expr.len() == 1 {
        expr[0].repr.parse::<u32>().ok()
    } else {
        None
    }
}

/// Connect FIFOs: declare inter-task wires and connect external FIFOs.
///
/// For internal FIFOs (both endpoints in this task): declare wires with
/// proper width using stream suffixes so child instances can connect.
/// For external FIFOs: connect to parent module ports, potentially
/// through AXIS adapters.
fn connect_fifos(state: &mut TopologyWithRtl, task_name: &str) {
    use tapa_protocol::{ISTREAM_SUFFIXES, OSTREAM_SUFFIXES, STREAM_PORT_DIRECTION};
    use tapa_rtl::signal::{Signal, SignalKind};

    let task = &state.program.tasks[task_name];

    // Collect FIFO connection info with producer endpoint for width resolution
    let fifo_entries: Vec<FifoConnEntry> = task
        .fifos
        .iter()
        .map(|(name, fifo)| {
            let has_consumer = fifo.consumed_by.is_some();
            let has_producer = fifo.produced_by.is_some();
            let producer = fifo_producer_for(task, name, fifo.produced_by.as_ref());
            (
                name.clone(),
                fifo.depth,
                has_consumer,
                has_producer,
                producer,
            )
        })
        .collect();

    for (fifo_name, depth, has_consumer, has_producer, producer) in &fifo_entries {
        let sanitized_fifo_name = tapa_rtl::module::sanitize_array_name(fifo_name);
        // Resolve width from producer child's attached RTL
        let width = resolve_fifo_width(state, producer.as_ref());

        if depth.is_some() && *has_consumer && *has_producer {
            // Internal FIFO: declare wires for both read and write sides
            if let Some(mm) = state.module_map.get_mut(task_name) {
                // Declare wires for each FIFO suffix (read side)
                for suffix in ISTREAM_SUFFIXES {
                    let wire_name = format!("{sanitized_fifo_name}{suffix}");
                    let sig = if suffix.contains("dout") {
                        tapa_rtl::mutation::wide_wire(&wire_name, &(width - 1).to_string(), "0")
                    } else {
                        tapa_rtl::mutation::wire(&wire_name)
                    };
                    let _ = mm.add_signal(sig);
                }
                // Declare wires for write side
                for suffix in OSTREAM_SUFFIXES {
                    let wire_name = format!("{sanitized_fifo_name}{suffix}");
                    let sig = if suffix.contains("din") {
                        tapa_rtl::mutation::wide_wire(&wire_name, &(width - 1).to_string(), "0")
                    } else {
                        tapa_rtl::mutation::wire(&wire_name)
                    };
                    let _ = mm.add_signal(sig);
                }
            }
        } else if depth.is_none() {
            // External FIFO: parent module ports exist, just need to ensure
            // wires exist for child instance connections.
            let stream_width = task
                .ports
                .iter()
                .find(|p| p.name == *fifo_name)
                .map_or(32, |p| p.width);
            let is_vitis_top_axis =
                task_name == state.program.top && state.program.target == "xilinx-vitis";
            if let Some(mm) = state.module_map.get_mut(task_name) {
                let suffixes: &[&str] = if *has_consumer {
                    ISTREAM_SUFFIXES
                } else {
                    OSTREAM_SUFFIXES
                };
                let canonical_base = tapa_rtl::module::sanitize_array_name(fifo_name);
                for suffix in suffixes {
                    let canonical = format!("{canonical_base}{suffix}");
                    let signal_width = if suffix.ends_with("dout") || suffix.ends_with("din") {
                        Some(tapa_rtl::port::Width {
                            msb: tapa_rtl::expression::tokenize_expression(
                                &stream_width.to_string(),
                            ),
                            lsb: tapa_rtl::expression::tokenize_expression("0"),
                        })
                    } else {
                        None
                    };
                    let _ = mm.add_signal(Signal {
                        name: canonical.clone(),
                        kind: SignalKind::Wire,
                        width: signal_width,
                    });
                    let Some(parent_port) = mm.inner.get_port_of(fifo_name, suffix).cloned() else {
                        continue;
                    };
                    if parent_port.name == canonical {
                        continue;
                    }
                    let parent = Expr::ident(parent_port.name);
                    let internal = Expr::ident(canonical);
                    let is_input_dir = STREAM_PORT_DIRECTION
                        .get(suffix)
                        .is_some_and(|&d| d == "input");
                    if is_input_dir {
                        mm.add_assign(ContinuousAssign::new(internal, parent));
                    } else {
                        mm.add_assign(ContinuousAssign::new(parent, internal));
                    }
                }
            }

            // Check if this should be an AXIS adapter
            if is_vitis_top_axis {
                // Instantiate AXIS adapter
                let is_input = *has_consumer;
                let adapter = fifos::build_axis_adapter(fifo_name, stream_width, is_input);
                if let Some(mm) = state.module_map.get_mut(task_name) {
                    mm.add_instance(adapter);
                    if !is_input {
                        let canonical_base = tapa_rtl::module::sanitize_array_name(fifo_name);
                        if mm
                            .inner
                            .find_port(&format!("{canonical_base}_TKEEP"))
                            .is_some()
                        {
                            let keep_width = stream_width.div_ceil(8).max(1);
                            mm.add_assign(ContinuousAssign::new(
                                Expr::ident(format!("{canonical_base}_TKEEP")),
                                Expr::lit(format!(
                                    "{keep_width}'b{}",
                                    "1".repeat(keep_width as usize)
                                )),
                            ));
                        }
                    }
                }
            }
        }
    }
}

/// Add M-AXI ports, crossbar instances, and emit crossbar aux files.
fn add_m_axi_and_crossbars(
    state: &mut TopologyWithRtl,
    task_name: &str,
    mmap_conns: &std::collections::BTreeMap<String, crate::rtl_state::MMapConnection>,
) -> Result<(), CodegenError> {
    for conn in mmap_conns.values() {
        // Validate before generating
        m_axi::validate_mmap_connection(conn)?;

        if let Some(mm) = state.module_map.get_mut(task_name) {
            if conn.chan_count > 1 {
                for channel_idx in 0..conn.chan_count {
                    m_axi::add_m_axi_ports_with_id_width(
                        mm,
                        &format!("{}_{}", conn.arg_name, channel_idx),
                        conn.data_width,
                        64,
                        conn.id_width,
                    );
                }
            } else if m_axi::needs_crossbar(conn) {
                m_axi::add_m_axi_ports_with_id_width(
                    mm,
                    &conn.arg_name,
                    conn.data_width,
                    64,
                    conn.id_width,
                );
            } else if conn.id_width > 1 {
                m_axi::add_m_axi_ports_with_id_width(
                    mm,
                    &conn.arg_name,
                    conn.data_width,
                    64,
                    conn.id_width,
                );
            } else {
                m_axi::add_m_axi_ports(mm, &conn.arg_name, conn.data_width, 64);
            }
        }
        if m_axi::needs_crossbar(conn) {
            // Declare downstream m_axi_{arg}_{idx}_* wires in parent
            // Size each wire using protocol metadata for correct widths
            if let Some(mm) = state.module_map.get_mut(task_name) {
                if conn.chan_count > 1 {
                    let addr_width = m_axi::get_addr_width(conn.chan_size, conn.data_width);
                    for channel_idx in 0..conn.chan_count {
                        let channel_prefix = format!(
                            "m_axi_{}_{}",
                            tapa_rtl::module::sanitize_array_name(&conn.arg_name),
                            channel_idx
                        );
                        let offset_name = format!(
                            "{}_{}_offset",
                            tapa_rtl::module::sanitize_array_name(&conn.arg_name),
                            channel_idx
                        );
                        for suffix in ["_ARADDR", "_AWADDR"] {
                            let raw = m_axi::crossbar_master_addr_raw(
                                &conn.arg_name,
                                channel_idx,
                                suffix,
                            );
                            let _ = mm.add_signal(tapa_rtl::mutation::wide_wire(&raw, "63", "0"));
                            let local_addr = if addr_width >= 64 {
                                Expr::ident(&raw)
                            } else {
                                Expr::range(
                                    Expr::ident(&raw),
                                    Expr::int(u64::from(addr_width - 1)),
                                    Expr::int(0),
                                )
                            };
                            let rhs = Expr::plus(Expr::ident(&offset_name), local_addr);
                            mm.add_assign(ContinuousAssign::new(
                                Expr::ident(format!("{channel_prefix}{suffix}")),
                                rhs,
                            ));
                        }
                    }
                }

                for (slave_idx, _) in conn.args.iter().enumerate() {
                    let wire_prefix = m_axi::crossbar_slave_prefix(&conn.arg_name, slave_idx);
                    for suffix in tapa_protocol::M_AXI_SUFFIXES_COMPACT {
                        let wire_name = format!("{wire_prefix}{suffix}");
                        // Resolve width from suffix name using protocol constants
                        let width = m_axi::crossbar_slave_suffix_width(conn, suffix);
                        let sig = if width > 1 {
                            tapa_rtl::mutation::wide_wire(&wire_name, &(width - 1).to_string(), "0")
                        } else {
                            tapa_rtl::mutation::wire(&wire_name)
                        };
                        let _ = mm.add_signal(sig);
                    }
                }
            }

            let crossbar_inst = m_axi::build_crossbar_instance(conn);
            if let Some(mm) = state.module_map.get_mut(task_name) {
                mm.add_instance(crossbar_inst);
            }
            let crossbar_rtl = m_axi::generate_crossbar_rtl(conn);
            let file_name = format!("{}.v", m_axi::crossbar_module_name(conn));
            state.generated_files.insert(file_name, crossbar_rtl);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rtl_state::TopologyWithRtl;
    use tapa_rtl::VerilogModule;
    use tapa_topology::program::Program;

    /// Helper: build a minimal topology Program from a JSON value.
    fn program_from_json(json: serde_json::Value) -> Program {
        serde_json::from_value(json).expect("valid program JSON")
    }

    /// Helper: parse a minimal Verilog module source.
    fn parse_module(src: &str) -> VerilogModule {
        VerilogModule::parse(src).expect("valid Verilog")
    }

    // ------------------------------------------------------------------
    // 1. Simple design: one upper task + one lower child
    // ------------------------------------------------------------------

    #[test]
    fn test_generate_rtl_simple_design() {
        let prog = program_from_json(serde_json::json!({
            "top": "top",
            "target": "xilinx-hls",
            "tasks": {
                "top": {
                    "level": "upper",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [],
                    "tasks": {
                        "child": [{"args": {}}]
                    },
                    "fifos": {}
                },
                "child": {
                    "level": "lower",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [],
                    "tasks": {},
                    "fifos": {}
                }
            }
        }));

        let mut state = TopologyWithRtl::new(prog);

        // Attach Verilog modules for both tasks
        state
            .attach_module(
                "top",
                parse_module(
                    "module top(\n  input wire ap_clk,\n  input wire ap_rst_n\n);\nendmodule",
                ),
            )
            .unwrap();
        state
            .attach_module(
                "child",
                parse_module(
                    "module child(\n  input wire ap_clk,\n  input wire ap_rst_n\n);\nendmodule",
                ),
            )
            .unwrap();

        generate_rtl(&mut state).unwrap();

        // generated_files should contain the parent .v and an FSM .v
        assert!(
            state.generated_files.contains_key("top.v"),
            "should emit top.v, got keys: {:?}",
            state.generated_files.keys().collect::<Vec<_>>()
        );
        assert!(
            state.generated_files.contains_key("top_fsm.v"),
            "should emit top_fsm.v, got keys: {:?}",
            state.generated_files.keys().collect::<Vec<_>>()
        );

        // The emitted parent module should contain the child instance
        let parent_v = &state.generated_files["top.v"];
        assert!(
            parent_v.contains("child child_0"),
            "parent should instantiate child as child_0, got:\n{parent_v}"
        );

        // The FSM module should contain __tapa_state and pipeline signals
        let fsm_v = &state.generated_files["top_fsm.v"];
        assert!(
            fsm_v.contains("__tapa_state"),
            "FSM should contain __tapa_state, got:\n{fsm_v}"
        );
        assert!(
            fsm_v.contains("__tapa_start_q"),
            "FSM should contain __tapa_start_q pipeline signal, got:\n{fsm_v}"
        );
        assert!(
            fsm_v.contains("__tapa_done_q"),
            "FSM should contain __tapa_done_q pipeline signal, got:\n{fsm_v}"
        );
    }

    #[test]
    fn test_generate_rtl_autorun_fsm_start_is_reg_output() {
        let prog = program_from_json(serde_json::json!({
            "top": "top",
            "target": "xilinx-hls",
            "tasks": {
                "top": {
                    "level": "upper",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [],
                    "tasks": {
                        "child": [{"step": -1, "args": {}}]
                    },
                    "fifos": {}
                },
                "child": {
                    "level": "lower",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [],
                    "tasks": {},
                    "fifos": {}
                }
            }
        }));

        let mut state = TopologyWithRtl::new(prog);
        state
            .attach_module(
                "top",
                parse_module(
                    "module top(\n  input wire ap_clk,\n  input wire ap_rst_n\n);\nendmodule",
                ),
            )
            .unwrap();
        state
            .attach_module(
                "child",
                parse_module(
                    "module child(\n  input wire ap_clk,\n  input wire ap_rst_n\n);\nendmodule",
                ),
            )
            .unwrap();

        generate_rtl(&mut state).unwrap();

        let parent_v = &state.generated_files["top.v"];
        let fsm_v = &state.generated_files["top_fsm.v"];
        assert!(
            parent_v.contains("wire child_0__ap_start;"),
            "parent-side autorun start is driven by the FSM instance and must be a net:\n{parent_v}"
        );
        assert!(
            !parent_v.contains("reg child_0__ap_start;"),
            "parent-side autorun start should not be a reg:\n{parent_v}"
        );
        assert!(
            fsm_v.contains("output reg child_0__ap_start"),
            "autorun start is assigned procedurally and must be an output reg:\n{fsm_v}"
        );
        assert!(
            !fsm_v.contains("\nreg child_0__ap_start;"),
            "reg port should not be redeclared:\n{fsm_v}"
        );
    }

    #[test]
    fn test_generate_rtl_fsm_uses_explicit_instance_names() {
        let prog = program_from_json(serde_json::json!({
            "top": "top",
            "target": "xilinx-hls",
            "tasks": {
                "top": {
                    "level": "upper",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [],
                    "tasks": {
                        "child": [{"name": "child_7", "step": -1, "args": {}}]
                    },
                    "fifos": {}
                },
                "child": {
                    "level": "lower",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [],
                    "tasks": {},
                    "fifos": {}
                }
            }
        }));

        let mut state = TopologyWithRtl::new(prog);
        state
            .attach_module(
                "top",
                parse_module(
                    "module top(\n  input wire ap_clk,\n  input wire ap_rst_n\n);\nendmodule",
                ),
            )
            .unwrap();
        state
            .attach_module(
                "child",
                parse_module(
                    "module child(\n  input wire ap_clk,\n  input wire ap_rst_n\n);\nendmodule",
                ),
            )
            .unwrap();

        generate_rtl(&mut state).unwrap();

        let parent_v = &state.generated_files["top.v"];
        let fsm_v = &state.generated_files["top_fsm.v"];
        assert!(
            parent_v.contains("child child_7"),
            "parent instance should preserve explicit instance name:\n{parent_v}"
        );
        assert!(
            fsm_v.contains("output reg child_7__ap_start"),
            "FSM ports must use the explicit child instance name:\n{fsm_v}"
        );
        assert!(
            !fsm_v.contains("child_0__ap_start"),
            "FSM must not use local index names when an explicit instance name exists:\n{fsm_v}"
        );
    }

    #[test]
    fn test_generate_rtl_sanitizes_explicit_instance_names() {
        let prog = program_from_json(serde_json::json!({
            "top": "top",
            "target": "xilinx-hls",
            "tasks": {
                "top": {
                    "level": "upper",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [],
                    "tasks": {
                        "child": [{"name": "Module1Func#1", "args": {}}]
                    },
                    "fifos": {}
                },
                "child": {
                    "level": "lower",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [],
                    "tasks": {},
                    "fifos": {}
                }
            }
        }));

        let mut state = TopologyWithRtl::new(prog);
        state
            .attach_module(
                "top",
                parse_module(
                    "module top(\n  input wire ap_clk,\n  input wire ap_rst_n\n);\nendmodule",
                ),
            )
            .unwrap();
        state
            .attach_module(
                "child",
                parse_module(
                    "module child(\n  input wire ap_clk,\n  input wire ap_rst_n,\n  input wire ap_start,\n  output wire ap_done,\n  output wire ap_idle,\n  output wire ap_ready\n);\nendmodule",
                ),
            )
            .unwrap();

        generate_rtl(&mut state).unwrap();

        let parent_v = &state.generated_files["top.v"];
        let fsm_v = &state.generated_files["top_fsm.v"];
        assert!(
            parent_v.contains("child Module1Func_1"),
            "parent instance name must be a Verilog identifier:\n{parent_v}"
        );
        assert!(
            fsm_v.contains("reg [1:0] Module1Func_1__state;"),
            "FSM state signal must be a Verilog identifier:\n{fsm_v}"
        );
        assert!(
            !parent_v.contains("Module1Func#1") && !fsm_v.contains("Module1Func#1"),
            "generated RTL must not contain unsanitized frontend instance labels"
        );
    }

    #[test]
    fn test_generate_rtl_child_scalar_pipeline_preserves_width() {
        let prog = program_from_json(serde_json::json!({
            "top": "top",
            "target": "xilinx-hls",
            "tasks": {
                "top": {
                    "level": "upper",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [],
                    "tasks": {
                        "child": [{"args": {"pe_id": {"arg": "1", "cat": "scalar"}}}]
                    },
                    "fifos": {}
                },
                "child": {
                    "level": "lower",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [
                        {"cat": "scalar", "name": "pe_id", "type": "uint32_t", "width": 32}
                    ],
                    "tasks": {},
                    "fifos": {}
                }
            }
        }));

        let mut state = TopologyWithRtl::new(prog);
        state
            .attach_module(
                "top",
                parse_module(
                    "module top(\n  input wire ap_clk,\n  input wire ap_rst_n\n);\nendmodule",
                ),
            )
            .unwrap();
        state
            .attach_module(
                "child",
                parse_module(
                    "module child(\n\
                     input wire ap_clk,\n\
                     input wire ap_rst_n,\n\
                     input wire [31:0] pe_id\n\
                     );\nendmodule",
                ),
            )
            .unwrap();

        generate_rtl(&mut state).unwrap();

        let top_v = &state.generated_files["top.v"];
        let fsm_v = &state.generated_files["top_fsm.v"];
        assert!(
            top_v.contains("wire [31:0] child_0__pe_id;"),
            "parent scalar pipeline wire should match child port width, got:\n{top_v}"
        );
        assert!(
            fsm_v.contains("input wire [31:0] child_0__pe_id_in"),
            "FSM scalar input should match child port width, got:\n{fsm_v}"
        );
        assert!(
            fsm_v.contains("output wire [31:0] child_0__pe_id"),
            "FSM scalar output should match child port width, got:\n{fsm_v}"
        );
        assert!(
            fsm_v.contains("reg [31:0] child_0__pe_id_reg;"),
            "FSM scalar pipeline register should match child port width, got:\n{fsm_v}"
        );
    }

    #[test]
    fn test_generate_rtl_upper_output_regs_become_nets() {
        let prog = program_from_json(serde_json::json!({
            "top": "top",
            "target": "xilinx-hls",
            "tasks": {
                "top": {
                    "level": "upper",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [
                        {"cat": "ostream", "name": "out_q", "type": "int", "width": 32}
                    ],
                    "tasks": {
                        "child": [{"args": {"out_q": {"arg": "out_q", "cat": "ostream"}}}]
                    },
                    "fifos": {}
                },
                "child": {
                    "level": "lower",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [
                        {"cat": "ostream", "name": "out_q", "type": "int", "width": 32}
                    ],
                    "tasks": {},
                    "fifos": {}
                }
            }
        }));

        let mut state = TopologyWithRtl::new(prog);
        state
            .attach_module(
                "top",
                parse_module(
                    "module top(\n\
                     input wire ap_clk,\n\
                     input wire ap_rst_n,\n\
                     output wire [31:0] out_q_din,\n\
                     input wire out_q_full_n,\n\
                     output reg out_q_write\n\
                     );\nendmodule",
                ),
            )
            .unwrap();
        state
            .attach_module(
                "child",
                parse_module(
                    "module child(\n\
                     input wire ap_clk,\n\
                     input wire ap_rst_n,\n\
                     output wire [31:0] out_q_din,\n\
                     input wire out_q_full_n,\n\
                     output wire out_q_write\n\
                     );\nendmodule",
                ),
            )
            .unwrap();

        generate_rtl(&mut state).unwrap();

        let top_v = &state.generated_files["top.v"];
        assert!(
            top_v.contains("output wire out_q_write"),
            "upper output driven by child instance should be a net, got:\n{top_v}"
        );
        assert!(
            !top_v.contains("output reg out_q_write"),
            "stale HLS output reg should not remain, got:\n{top_v}"
        );
    }

    // ------------------------------------------------------------------
    // 2. Template task: upper task with no children (template)
    // ------------------------------------------------------------------

    #[test]
    fn test_generate_rtl_template_task() {
        let prog = program_from_json(serde_json::json!({
            "top": "shell",
            "target": "xilinx-hls",
            "tasks": {
                "shell": {
                    "level": "upper",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [
                        {"cat": "scalar", "name": "n", "type": "int", "width": 32}
                    ],
                    "tasks": {},
                    "fifos": {}
                }
            }
        }));

        let mut state = TopologyWithRtl::new(prog);
        state
            .attach_module(
                "shell",
                parse_module(
                    "module shell(\n\
                     input wire ap_clk,\n\
                     input wire ap_rst_n,\n\
                     input wire [31:0] n\n\
                     );\nendmodule",
                ),
            )
            .unwrap();

        generate_rtl(&mut state).unwrap();

        // Template task generates a _template.v file
        assert!(
            state.generated_files.contains_key("shell_template.v"),
            "should emit shell_template.v, got keys: {:?}",
            state.generated_files.keys().collect::<Vec<_>>()
        );
        let template_v = &state.generated_files["shell_template.v"];
        assert!(
            template_v.contains("module shell"),
            "template should contain module declaration, got:\n{template_v}"
        );
        assert!(
            template_v.contains("endmodule"),
            "template should end with endmodule, got:\n{template_v}"
        );

        // NO FSM module should be generated for a template task
        assert!(
            !state.generated_files.contains_key("shell_fsm.v"),
            "template task should not have an FSM module, got keys: {:?}",
            state.generated_files.keys().collect::<Vec<_>>()
        );
        assert!(
            !state.fsm_modules.contains_key("shell"),
            "template task should not have fsm_modules entry"
        );
    }

    // ------------------------------------------------------------------
    // 3. Top task removes peek ports from istream
    // ------------------------------------------------------------------

    #[test]
    fn test_generate_rtl_top_task_removes_peek_ports() {
        let prog = program_from_json(serde_json::json!({
            "top": "top",
            "target": "xilinx-hls",
            "tasks": {
                "top": {
                    "level": "upper",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [
                        {"cat": "istream", "name": "data_in", "type": "float", "width": 32}
                    ],
                    "tasks": {
                        "reader": [{"args": {"input": {"arg": "data_in", "cat": "istream"}}}]
                    },
                    "fifos": {}
                },
                "reader": {
                    "level": "lower",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [
                        {"cat": "istream", "name": "input", "type": "float", "width": 32}
                    ],
                    "tasks": {},
                    "fifos": {}
                }
            }
        }));

        let mut state = TopologyWithRtl::new(prog);

        // The top module has istream_peek_* ports that should be removed
        state
            .attach_module(
                "top",
                parse_module(
                    "module top(\n\
                     input wire ap_clk,\n\
                     input wire ap_rst_n,\n\
                     input wire [31:0] data_in_dout,\n\
                     input wire data_in_empty_n,\n\
                     output wire data_in_read,\n\
                     input wire [31:0] data_in_peek_dout,\n\
                     input wire data_in_peek_empty_n,\n\
                     output wire data_in_peek_read\n\
                     );\nendmodule",
                ),
            )
            .unwrap();
        state
            .attach_module(
                "reader",
                parse_module(
                    "module reader(\n  input wire ap_clk,\n  input wire ap_rst_n\n);\nendmodule",
                ),
            )
            .unwrap();

        generate_rtl(&mut state).unwrap();

        let top_v = &state.generated_files["top.v"];

        // Peek ports should be removed from the emitted module declaration
        let decl_section = top_v.split(");").next().unwrap_or("");
        assert!(
            !decl_section.contains("data_in_peek_dout"),
            "peek dout port should be removed from declaration, got:\n{decl_section}"
        );
        assert!(
            !decl_section.contains("data_in_peek_empty_n"),
            "peek empty_n port should be removed from declaration, got:\n{decl_section}"
        );
        assert!(
            !decl_section.contains("data_in_peek_read"),
            "peek read port should be removed from declaration, got:\n{decl_section}"
        );

        // Regular istream ports should still be present
        assert!(
            decl_section.contains("data_in_dout"),
            "regular data_in_dout should remain, got:\n{decl_section}"
        );
    }

    #[test]
    fn test_generate_rtl_external_istream_aliases_hls_s_ports() {
        let prog = program_from_json(serde_json::json!({
            "top": "top",
            "target": "xilinx-hls",
            "tasks": {
                "top": {
                    "level": "upper",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [
                        {"cat": "istream", "name": "data_stream", "type": "uint64_t", "width": 64}
                    ],
                    "tasks": {
                        "consumer": [{"args": {"data_stream": {"arg": "data_stream", "cat": "istream"}}}]
                    },
                    "fifos": {
                        "data_stream": {"consumed_by": ["consumer", 0]}
                    }
                },
                "consumer": {
                    "level": "lower",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [
                        {"cat": "istream", "name": "data_stream", "type": "uint64_t", "width": 64}
                    ],
                    "tasks": {},
                    "fifos": {}
                }
            }
        }));

        let mut state = TopologyWithRtl::new(prog);
        state
            .attach_module(
                "top",
                parse_module(
                    "module top(\n\
                     input wire ap_clk,\n\
                     input wire ap_rst_n,\n\
                     input wire [64:0] data_stream_s_dout,\n\
                     input wire data_stream_s_empty_n,\n\
                     output wire data_stream_s_read\n\
                     );\nendmodule",
                ),
            )
            .unwrap();
        state
            .attach_module(
                "consumer",
                parse_module(
                    "module consumer(\n\
                     input wire ap_clk,\n\
                     input wire ap_rst_n,\n\
                     input wire [64:0] data_stream_s_dout,\n\
                     input wire data_stream_s_empty_n,\n\
                     output wire data_stream_s_read\n\
                     );\nendmodule",
                ),
            )
            .unwrap();

        generate_rtl(&mut state).unwrap();
        let top_v = &state.generated_files["top.v"];

        assert!(
            top_v.contains("wire [64:0] data_stream_dout;"),
            "canonical child-facing data wire should be declared:\n{top_v}"
        );
        assert!(
            top_v.contains("assign data_stream_dout = data_stream_s_dout;"),
            "external HLS input port should drive canonical child-facing wire:\n{top_v}"
        );
        assert!(
            top_v.contains("assign data_stream_s_read = data_stream_read;"),
            "canonical child read should drive external HLS read port:\n{top_v}"
        );
        assert!(
            top_v.contains(".data_stream_s_dout(data_stream_dout)"),
            "child HLS stream port should connect through the canonical alias:\n{top_v}"
        );
    }

    #[test]
    fn test_generate_rtl_vitis_top_streams_use_axis_adapters() {
        let prog = program_from_json(serde_json::json!({
            "top": "top",
            "target": "xilinx-vitis",
            "tasks": {
                "top": {
                    "level": "upper",
                    "code": "",
                    "target": "xilinx-vitis",
                    "ports": [
                        {"cat": "istream", "name": "a", "type": "uint48_t", "width": 48},
                        {"cat": "ostream", "name": "c", "type": "uint64_t", "width": 64}
                    ],
                    "tasks": {
                        "worker": [{"args": {
                            "a": {"arg": "a", "cat": "istream"},
                            "c": {"arg": "c", "cat": "ostream"}
                        }}]
                    },
                    "fifos": {
                        "a": {"consumed_by": ["worker", 0]},
                        "c": {"produced_by": ["worker", 0]}
                    }
                },
                "worker": {
                    "level": "lower",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [
                        {"cat": "istream", "name": "a", "type": "uint48_t", "width": 48},
                        {"cat": "ostream", "name": "c", "type": "uint64_t", "width": 64}
                    ],
                    "tasks": {},
                    "fifos": {}
                }
            }
        }));

        let mut state = TopologyWithRtl::new(prog);
        state
            .attach_module(
                "top",
                parse_module(
                    "module top(\n\
                     input wire ap_clk,\n\
                     input wire ap_rst_n,\n\
                     input wire [47:0] a_TDATA,\n\
                     input wire a_TVALID,\n\
                     output wire a_TREADY,\n\
                     input wire [0:0] a_TLAST,\n\
                     output wire [63:0] c_TDATA,\n\
                     output wire c_TVALID,\n\
                     input wire c_TREADY,\n\
                     output wire [0:0] c_TLAST,\n\
                     output wire [7:0] c_TKEEP\n\
                     );\n\
                     reg ap_done;\n\
                     reg ap_idle;\n\
                     reg ap_ready;\n\
                     endmodule",
                ),
            )
            .unwrap();
        state
            .attach_module(
                "worker",
                parse_module(
                    "module worker(\n\
                     input wire ap_clk,\n\
                     input wire ap_rst_n,\n\
                     input wire [48:0] a_s_dout,\n\
                     input wire a_s_empty_n,\n\
                     output wire a_s_read,\n\
                     output wire [64:0] c_s_din,\n\
                     input wire c_s_full_n,\n\
                     output wire c_s_write\n\
                     );\nendmodule",
                ),
            )
            .unwrap();

        generate_rtl(&mut state).unwrap();
        let top_v = &state.generated_files["top.v"];

        assert!(
            top_v.contains("wire [48:0] a_dout;"),
            "input AXIS adapter should drive canonical stream data:\n{top_v}"
        );
        assert!(
            top_v.contains("axis_to_stream_adapter #(")
                && top_v.contains(".DATA_WIDTH(48)")
                && top_v.contains(".s_axis_tdata(a_TDATA)")
                && top_v.contains(".m_stream_dout(a_dout)"),
            "input AXIS adapter should be instantiated with compatible ports:\n{top_v}"
        );
        assert!(
            top_v.contains("stream_to_axis_adapter #(")
                && top_v.contains(".DATA_WIDTH(64)")
                && top_v.contains(".s_stream_din(c_din)")
                && top_v.contains(".m_axis_tlast(c_TLAST)"),
            "output AXIS adapter should be instantiated with compatible ports:\n{top_v}"
        );
        assert!(
            top_v.contains("assign c_TKEEP = 8'b11111111;"),
            "output AXIS TKEEP should be tied high:\n{top_v}"
        );
        assert!(
            top_v.contains("wire ap_done;")
                && top_v.contains("wire ap_idle;")
                && top_v.contains("wire ap_ready;"),
            "generated submodule outputs should drive nets, not regs:\n{top_v}"
        );
    }

    // ------------------------------------------------------------------
    // 4. Upper task with a FIFO between producer and consumer children
    // ------------------------------------------------------------------

    #[test]
    fn test_generate_rtl_with_fifo() {
        let prog = program_from_json(serde_json::json!({
            "top": "top",
            "target": "xilinx-hls",
            "tasks": {
                "top": {
                    "level": "upper",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [],
                    "tasks": {
                        "producer": [{"args": {"out_data": {"arg": "fifo_0", "cat": "ostream"}}}],
                        "consumer": [{"args": {"in_data": {"arg": "fifo_0", "cat": "istream"}}}]
                    },
                    "fifos": {
                        "fifo_0": {
                            "depth": 16,
                            "produced_by": ["producer", 0],
                            "consumed_by": ["consumer", 0]
                        }
                    }
                },
                "producer": {
                    "level": "lower",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [
                        {"cat": "ostream", "name": "out_data", "type": "float", "width": 32}
                    ],
                    "tasks": {},
                    "fifos": {}
                },
                "consumer": {
                    "level": "lower",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [
                        {"cat": "istream", "name": "in_data", "type": "float", "width": 32}
                    ],
                    "tasks": {},
                    "fifos": {}
                }
            }
        }));

        let mut state = TopologyWithRtl::new(prog);
        state
            .attach_module(
                "top",
                parse_module(
                    "module top(\n  input wire ap_clk,\n  input wire ap_rst_n\n);\nendmodule",
                ),
            )
            .unwrap();
        // Producer with a _din port so width resolution finds 32 bits
        state
            .attach_module(
                "producer",
                parse_module(
                    "module producer(\n\
                     input wire ap_clk,\n\
                     input wire ap_rst_n,\n\
                     output wire [31:0] out_data_din,\n\
                     output wire out_data_write,\n\
                     input wire out_data_full_n\n\
                     );\nendmodule",
                ),
            )
            .unwrap();
        state
            .attach_module(
                "consumer",
                parse_module(
                    "module consumer(\n\
                     input wire ap_clk,\n\
                     input wire ap_rst_n,\n\
                     input wire [31:0] in_data_dout,\n\
                     input wire in_data_empty_n,\n\
                     output wire in_data_read\n\
                     );\nendmodule",
                ),
            )
            .unwrap();

        generate_rtl(&mut state).unwrap();

        let top_v = &state.generated_files["top.v"];

        // Should contain a FIFO instance (parameterized: "fifo #(...) fifo_0_fifo")
        assert!(
            top_v.contains("fifo_0_fifo"),
            "parent should contain FIFO instance, got:\n{top_v}"
        );

        // Should contain wire declarations for the FIFO
        assert!(
            top_v.contains("fifo_0_dout") || top_v.contains("fifo_0_din"),
            "parent should contain FIFO wire declarations, got:\n{top_v}"
        );
    }

    #[test]
    fn test_generate_rtl_does_not_reemit_lower_modules() {
        let prog = program_from_json(serde_json::json!({
            "top": "top",
            "target": "xilinx-hls",
            "tasks": {
                "top": {
                    "level": "upper",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [],
                    "tasks": {
                        "child": [{"args": {}}]
                    },
                    "fifos": {}
                },
                "child": {
                    "level": "lower",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [],
                    "tasks": {},
                    "fifos": {}
                }
            }
        }));

        let mut state = TopologyWithRtl::new(prog);
        state
            .attach_module(
                "top",
                parse_module(
                    "module top(\n  input wire ap_clk,\n  input wire ap_rst_n\n);\nendmodule",
                ),
            )
            .unwrap();
        state
            .attach_module(
                "child",
                parse_module(
                    "module child(\n  input wire ap_clk,\n  output wire ap_done\n);\n\
                     reg ap_done;\n\
                     always @(*) begin ap_done = 1'b1; end\n\
                     endmodule",
                ),
            )
            .unwrap();

        generate_rtl(&mut state).unwrap();

        assert!(
            !state.generated_files.contains_key("child.v"),
            "lower HLS modules are copied from the original files; re-emitting \
             them drops legal port-reg redeclarations"
        );
    }

    #[test]
    fn test_generate_rtl_fifo_width_uses_bound_producer_port() {
        let prog = program_from_json(serde_json::json!({
            "top": "top",
            "target": "xilinx-hls",
            "tasks": {
                "top": {
                    "level": "upper",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [],
                    "tasks": {
                        "producer": [{
                            "args": {
                                "small": {"arg": "small_fifo", "cat": "ostream"},
                                "wide": {"arg": "wide_fifo", "cat": "ostream"}
                            }
                        }],
                        "consumer": [{
                            "args": {
                                "small_in": {"arg": "small_fifo", "cat": "istream"},
                                "wide_in": {"arg": "wide_fifo", "cat": "istream"}
                            }
                        }]
                    },
                    "fifos": {
                        "small_fifo": {
                            "depth": 2,
                            "produced_by": ["producer", 0],
                            "consumed_by": ["consumer", 0]
                        },
                        "wide_fifo": {
                            "depth": 2,
                            "produced_by": ["producer", 0],
                            "consumed_by": ["consumer", 0]
                        }
                    }
                },
                "producer": {
                    "level": "lower",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [
                        {"cat": "ostream", "name": "small", "type": "uint8_t", "width": 8},
                        {"cat": "ostream", "name": "wide", "type": "uint32_t", "width": 32}
                    ],
                    "tasks": {},
                    "fifos": {}
                },
                "consumer": {
                    "level": "lower",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [
                        {"cat": "istream", "name": "small_in", "type": "uint8_t", "width": 8},
                        {"cat": "istream", "name": "wide_in", "type": "uint32_t", "width": 32}
                    ],
                    "tasks": {},
                    "fifos": {}
                }
            }
        }));

        let mut state = TopologyWithRtl::new(prog);
        state
            .attach_module(
                "top",
                parse_module(
                    "module top(\n  input wire ap_clk,\n  input wire ap_rst_n\n);\nendmodule",
                ),
            )
            .unwrap();
        state
            .attach_module(
                "producer",
                parse_module(
                    "module producer(\n\
                     input wire ap_clk,\n\
                     input wire ap_rst_n,\n\
                     output wire [8:0] small_s_din,\n\
                     output wire small_s_write,\n\
                     input wire small_s_full_n,\n\
                     output wire [32:0] wide_s_din,\n\
                     output wire wide_s_write,\n\
                     input wire wide_s_full_n\n\
                     );\nendmodule",
                ),
            )
            .unwrap();
        state
            .attach_module(
                "consumer",
                parse_module(
                    "module consumer(\n\
                     input wire ap_clk,\n\
                     input wire ap_rst_n,\n\
                     input wire [8:0] small_in_s_dout,\n\
                     input wire small_in_s_empty_n,\n\
                     output wire small_in_s_read,\n\
                     input wire [32:0] wide_in_s_dout,\n\
                     input wire wide_in_s_empty_n,\n\
                     output wire wide_in_s_read\n\
                     );\nendmodule",
                ),
            )
            .unwrap();

        generate_rtl(&mut state).unwrap();

        let top_v = &state.generated_files["top.v"];
        assert!(
            top_v.contains("small_fifo_fifo")
                && top_v.contains(".DATA_WIDTH(9)")
                && top_v.contains("wire [8:0] small_fifo_dout;")
                && top_v.contains("wire [8:0] small_fifo_din;"),
            "small_fifo should use the producer's small port width:\n{top_v}"
        );
        assert!(
            top_v.contains("wide_fifo_fifo")
                && top_v.contains(".DATA_WIDTH(33)")
                && top_v.contains("wire [32:0] wide_fifo_dout;")
                && top_v.contains("wire [32:0] wide_fifo_din;"),
            "wide_fifo should use the producer's wide port width:\n{top_v}"
        );
    }

    // ------------------------------------------------------------------
    // 5. Multi-thread mmap: two children sharing an mmap arg
    // ------------------------------------------------------------------

    #[test]
    fn test_generate_rtl_multithread_mmap() {
        let prog = program_from_json(serde_json::json!({
            "top": "top",
            "target": "xilinx-hls",
            "tasks": {
                "top": {
                    "level": "upper",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [
                        {"cat": "mmap", "name": "mem", "type": "float*", "width": 32}
                    ],
                    "tasks": {
                        "worker": [
                            {"args": {"data": {"arg": "mem", "cat": "mmap"}}},
                            {"args": {"data": {"arg": "mem", "cat": "mmap"}}}
                        ]
                    },
                    "fifos": {}
                },
                "worker": {
                    "level": "lower",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [
                        {"cat": "mmap", "name": "data", "type": "float*", "width": 32}
                    ],
                    "tasks": {},
                    "fifos": {}
                }
            }
        }));

        let mut state = TopologyWithRtl::new(prog);
        state
            .attach_module(
                "top",
                parse_module(
                    "module top(\n  input wire ap_clk,\n  input wire ap_rst_n\n);\nendmodule",
                ),
            )
            .unwrap();
        state
            .attach_module(
                "worker",
                parse_module(
                    "module worker(\n  input wire ap_clk,\n  input wire ap_rst_n\n);\nendmodule",
                ),
            )
            .unwrap();

        generate_rtl(&mut state).unwrap();

        let top_v = &state.generated_files["top.v"];

        // Crossbar instance should appear (2 threads sharing 'mem')
        assert!(
            top_v.contains("axi_crossbar"),
            "parent should contain crossbar instance, got:\n{top_v}"
        );

        // Downstream wires m_axi_mem_s0_* and m_axi_mem_s1_* should be declared
        assert!(
            top_v.contains("m_axi_mem_s0_"),
            "parent should have m_axi_mem_s0_* wires, got:\n{top_v}"
        );
        assert!(
            top_v.contains("m_axi_mem_s1_"),
            "parent should have m_axi_mem_s1_* wires, got:\n{top_v}"
        );

        // Crossbar auxiliary RTL file should be generated
        assert!(
            state
                .generated_files
                .keys()
                .any(|k| k.contains("axi_crossbar")),
            "should emit crossbar RTL file, got keys: {:?}",
            state.generated_files.keys().collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_generate_rtl_single_child_mmap_preserves_child_id_width() {
        let prog = program_from_json(serde_json::json!({
            "top": "top",
            "target": "xilinx-hls",
            "tasks": {
                "mid": {
                    "level": "upper",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [
                        {"cat": "mmap", "name": "data", "type": "float*", "width": 32}
                    ],
                    "tasks": {},
                    "fifos": {}
                },
                "top": {
                    "level": "upper",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [
                        {"cat": "mmap", "name": "elems", "type": "float*", "width": 32}
                    ],
                    "tasks": {
                        "mid": [{"args": {"data": {"arg": "elems", "cat": "mmap"}}}]
                    },
                    "fifos": {}
                }
            }
        }));

        let mut state = TopologyWithRtl::new(prog);
        state
            .attach_module(
                "mid",
                parse_module(
                    "module mid(\n\
                     input wire ap_clk,\n\
                     input wire ap_rst_n,\n\
                     output wire [1:0] m_axi_data_ARID,\n\
                     output wire [1:0] m_axi_data_AWID,\n\
                     input wire [1:0] m_axi_data_BID,\n\
                     input wire [1:0] m_axi_data_RID\n\
                     ); endmodule",
                ),
            )
            .unwrap();
        state
            .attach_module(
                "top",
                parse_module("module top(input wire ap_clk, input wire ap_rst_n); endmodule"),
            )
            .unwrap();

        generate_rtl(&mut state).unwrap();

        let top_v = &state.generated_files["top.v"];
        assert!(
            top_v.contains("output wire [1:0] m_axi_elems_ARID"),
            "top mmap ID ports must preserve a wider child AXI ID even without a parent crossbar:\n{top_v}"
        );
        assert!(
            top_v.contains(".m_axi_data_ARID(m_axi_elems_ARID)"),
            "child should bind directly to the widened parent ID port:\n{top_v}"
        );
    }

    #[test]
    fn test_generate_rtl_parent_crossbar_zero_extends_narrow_child_ids() {
        let prog = program_from_json(serde_json::json!({
            "top": "top",
            "target": "xilinx-hls",
            "tasks": {
                "leaf": {
                    "level": "lower",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [
                        {"cat": "mmap", "name": "mmap", "type": "float*", "width": 32}
                    ],
                    "tasks": {},
                    "fifos": {}
                },
                "mid": {
                    "level": "upper",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [
                        {"cat": "mmap", "name": "data", "type": "float*", "width": 32}
                    ],
                    "tasks": {},
                    "fifos": {}
                },
                "top": {
                    "level": "upper",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [
                        {"cat": "mmap", "name": "elems", "type": "float*", "width": 32}
                    ],
                    "tasks": {
                        "leaf": [{"args": {"mmap": {"arg": "elems", "cat": "mmap"}}}],
                        "mid": [{"args": {"data": {"arg": "elems", "cat": "mmap"}}}]
                    },
                    "fifos": {}
                }
            }
        }));

        let mut state = TopologyWithRtl::new(prog);
        state
            .attach_module(
                "leaf",
                parse_module(
                    "module leaf(\n\
                     input wire ap_clk,\n\
                     input wire ap_rst_n,\n\
                     output wire m_axi_mmap_ARID,\n\
                     output wire m_axi_mmap_AWID,\n\
                     input wire m_axi_mmap_BID,\n\
                     input wire m_axi_mmap_RID\n\
                     ); endmodule",
                ),
            )
            .unwrap();
        state
            .attach_module(
                "mid",
                parse_module(
                    "module mid(\n\
                     input wire ap_clk,\n\
                     input wire ap_rst_n,\n\
                     output wire [1:0] m_axi_data_ARID,\n\
                     output wire [1:0] m_axi_data_AWID,\n\
                     input wire [1:0] m_axi_data_BID,\n\
                     input wire [1:0] m_axi_data_RID\n\
                     ); endmodule",
                ),
            )
            .unwrap();
        state
            .attach_module(
                "top",
                parse_module("module top(input wire ap_clk, input wire ap_rst_n); endmodule"),
            )
            .unwrap();

        generate_rtl(&mut state).unwrap();

        let top_v = &state.generated_files["top.v"];
        assert!(
            top_v.contains("wire [1:0] m_axi_elems_s0_ARID"),
            "parent crossbar slave wires should use the widest child ID:\n{top_v}"
        );
        assert!(
            top_v.contains("assign m_axi_elems_s0_ARID[1:1] = 1'd0"),
            "narrow child read IDs should be zero-extended into the parent crossbar:\n{top_v}"
        );
        assert!(
            top_v.contains("assign m_axi_elems_s0_AWID[1:1] = 1'd0"),
            "narrow child write IDs should be zero-extended into the parent crossbar:\n{top_v}"
        );
        assert!(
            top_v.contains(".m_axi_mmap_ARID(m_axi_elems_s0_ARID[0:0])"),
            "narrow child read ID ports should connect only to the low crossbar ID bit:\n{top_v}"
        );
        assert!(
            top_v.contains(".m_axi_mmap_BID(m_axi_elems_s0_BID[0:0])"),
            "narrow child response ID ports should consume only the low crossbar ID bit:\n{top_v}"
        );
    }

    #[test]
    fn test_generate_rtl_parent_crossbar_slices_generated_narrow_upper_child_ids() {
        let prog = program_from_json(serde_json::json!({
            "top": "VecTop",
            "target": "xilinx-hls",
            "tasks": {
                "Awide": {
                    "level": "lower",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [
                        {"cat": "mmap", "name": "data", "type": "float*", "width": 32}
                    ],
                    "tasks": {},
                    "fifos": {}
                },
                "Leaf": {
                    "level": "lower",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [
                        {"cat": "mmap", "name": "mmap", "type": "float*", "width": 32}
                    ],
                    "tasks": {},
                    "fifos": {}
                },
                "Store": {
                    "level": "upper",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [
                        {"cat": "mmap", "name": "mmap", "type": "float*", "width": 32}
                    ],
                    "tasks": {
                        "Leaf": [{"args": {"mmap": {"arg": "mmap", "cat": "mmap"}}}]
                    },
                    "fifos": {}
                },
                "VecTop": {
                    "level": "upper",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [
                        {"cat": "mmap", "name": "elems", "type": "float*", "width": 32}
                    ],
                    "tasks": {
                        "Awide": [{"args": {"data": {"arg": "elems", "cat": "mmap"}}}],
                        "Store": [{"args": {"mmap": {"arg": "elems", "cat": "mmap"}}}]
                    },
                    "fifos": {}
                }
            }
        }));

        let mut state = TopologyWithRtl::new(prog);
        state
            .attach_module(
                "Awide",
                parse_module(
                    "module Awide(\n\
                     input wire ap_clk,\n\
                     input wire ap_rst_n,\n\
                     output wire [1:0] m_axi_data_ARID,\n\
                     output wire [1:0] m_axi_data_AWID,\n\
                     input wire [1:0] m_axi_data_BID,\n\
                     input wire [1:0] m_axi_data_RID\n\
                     ); endmodule",
                ),
            )
            .unwrap();
        state
            .attach_module(
                "Leaf",
                parse_module(
                    "module Leaf(\n\
                     input wire ap_clk,\n\
                     input wire ap_rst_n,\n\
                     output wire m_axi_mmap_ARID,\n\
                     output wire m_axi_mmap_AWID,\n\
                     input wire m_axi_mmap_BID,\n\
                     input wire m_axi_mmap_RID\n\
                     ); endmodule",
                ),
            )
            .unwrap();
        state
            .attach_module(
                "Store",
                parse_module("module Store(input wire ap_clk, input wire ap_rst_n); endmodule"),
            )
            .unwrap();
        state
            .attach_module(
                "VecTop",
                parse_module("module VecTop(input wire ap_clk, input wire ap_rst_n); endmodule"),
            )
            .unwrap();

        generate_rtl(&mut state).unwrap();

        let top_v = &state.generated_files["VecTop.v"];
        assert!(
            top_v.contains("wire [1:0] m_axi_elems_s1_ARID"),
            "second crossbar slave should inherit the widest child ID width:\n{top_v}"
        );
        assert!(
            top_v.contains(".m_axi_data_ARID(m_axi_elems_s0_ARID)"),
            "wide sibling child read ID port should keep the full crossbar slave ID:\n{top_v}"
        );
        assert!(
            !top_v.contains("assign m_axi_elems_s0_ARID[1:1]"),
            "wide sibling child IDs should not be zero-extended as if they were narrow:\n{top_v}"
        );
        assert!(
            top_v.contains("assign m_axi_elems_s1_ARID[1:1] = 1'd0"),
            "generated narrow upper child read IDs should be zero-extended:\n{top_v}"
        );
        assert!(
            top_v.contains(".m_axi_mmap_ARID(m_axi_elems_s1_ARID[0:0])"),
            "generated narrow upper child read ID port should connect only to the low bit:\n{top_v}"
        );
        assert!(
            top_v.contains(".m_axi_mmap_BID(m_axi_elems_s1_BID[0:0])"),
            "generated narrow upper child response ID port should consume only the low bit:\n{top_v}"
        );
    }

    #[test]
    fn test_generate_rtl_hmap_uses_parent_channels() {
        let prog = program_from_json(serde_json::json!({
            "top": "top",
            "target": "xilinx-hls",
            "tasks": {
                "top": {
                    "level": "upper",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [
                        {
                            "cat": "mmap",
                            "name": "mem",
                            "type": "float*",
                            "width": 32,
                            "chan_count": 2,
                            "chan_size": 1024
                        }
                    ],
                    "tasks": {
                        "worker": [
                            {"args": {"data": {"arg": "mem", "cat": "mmap"}}},
                            {"args": {"data": {"arg": "mem", "cat": "mmap"}}}
                        ]
                    },
                    "fifos": {}
                },
                "worker": {
                    "level": "lower",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [
                        {"cat": "mmap", "name": "data", "type": "float*", "width": 32}
                    ],
                    "tasks": {},
                    "fifos": {}
                }
            }
        }));

        let mut state = TopologyWithRtl::new(prog);
        state
            .attach_module(
                "top",
                parse_module(
                    "module top(input wire ap_clk, input wire ap_rst_n, input wire [63:0] mem_0_offset, input wire [63:0] mem_1_offset); endmodule",
                ),
            )
            .unwrap();
        state
            .attach_module(
                "worker",
                parse_module("module worker(input wire ap_clk, input wire ap_rst_n); endmodule"),
            )
            .unwrap();

        generate_rtl(&mut state).unwrap();

        let top_v = &state.generated_files["top.v"];
        assert!(top_v.contains("m_axi_mem_0_ARADDR"), "got:\n{top_v}");
        assert!(top_v.contains("m_axi_mem_1_ARADDR"), "got:\n{top_v}");
        assert!(
            top_v.contains(".m_axi_data_ARADDR(m_axi_mem_s0_ARADDR)"),
            "got:\n{top_v}"
        );
        assert!(
            top_v.contains(".m_axi_data_ARADDR(m_axi_mem_s1_ARADDR)"),
            "got:\n{top_v}"
        );
        assert!(top_v.contains("axi_crossbar__mem"), "got:\n{top_v}");
        assert!(top_v.contains("m_axi_mem_0_ARADDR_raw"), "got:\n{top_v}");
        assert!(
            top_v.contains(
                "assign m_axi_mem_1_ARADDR = (mem_1_offset + m_axi_mem_1_ARADDR_raw[11:0])"
            ),
            "got:\n{top_v}"
        );
        assert!(
            top_v.contains(".worker_0__data_offset_in(64'd0)"),
            "got:\n{top_v}"
        );
        assert!(
            top_v.contains(".worker_1__data_offset_in(64'd0)"),
            "got:\n{top_v}"
        );
    }

    #[test]
    fn test_generate_rtl_sanitizes_indexed_mmap_names() {
        let prog = program_from_json(serde_json::json!({
            "top": "top",
            "target": "xilinx-hls",
            "tasks": {
                "top": {
                    "level": "upper",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [
                        {"cat": "mmap", "name": "chan[0]", "type": "float*", "width": 32}
                    ],
                    "tasks": {
                        "worker": [
                            {"args": {"mem": {"arg": "chan[0]", "cat": "mmap"}}}
                        ]
                    },
                    "fifos": {}
                },
                "worker": {
                    "level": "lower",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [
                        {"cat": "mmap", "name": "mem", "type": "float*", "width": 32}
                    ],
                    "tasks": {},
                    "fifos": {}
                }
            }
        }));

        let mut state = TopologyWithRtl::new(prog);
        state
            .attach_module(
                "top",
                parse_module("module top(input wire ap_clk, input wire ap_rst_n); endmodule"),
            )
            .unwrap();
        state
            .attach_module(
                "worker",
                parse_module("module worker(input wire ap_clk, input wire ap_rst_n); endmodule"),
            )
            .unwrap();

        generate_rtl(&mut state).unwrap();

        let top_v = &state.generated_files["top.v"];
        assert!(top_v.contains("m_axi_chan_0_ARADDR"), "got:\n{top_v}");
        assert!(
            top_v.contains(".worker_0__mem_offset_in(chan_0_offset)"),
            "got:\n{top_v}"
        );
        assert!(!top_v.contains("chan[0]"), "got:\n{top_v}");
    }

    #[test]
    fn test_generate_rtl_instantiates_async_mmap_bridge() {
        let prog = program_from_json(serde_json::json!({
            "top": "top",
            "target": "xilinx-hls",
            "tasks": {
                "top": {
                    "level": "upper",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [
                        {"cat": "mmap", "name": "chan[0]", "type": "Elem*", "width": 512}
                    ],
                    "tasks": {
                        "copy": [
                            {"args": {"mem": {"arg": "chan[0]", "cat": "async_mmap"}}}
                        ]
                    },
                    "fifos": {}
                },
                "copy": {
                    "level": "lower",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [
                        {"cat": "async_mmap", "name": "mem", "type": "Elem*", "width": 512}
                    ],
                    "tasks": {},
                    "fifos": {}
                }
            }
        }));

        let mut state = TopologyWithRtl::new(prog);
        state
            .attach_module(
                "top",
                parse_module("module top(input wire ap_clk, input wire ap_rst_n); endmodule"),
            )
            .unwrap();
        state
            .attach_module(
                "copy",
                parse_module(
                    "module copy(\n\
                     input wire ap_clk,\n\
                     input wire ap_rst_n,\n\
                     output wire [63:0] mem_read_addr_s_din,\n\
                     input wire mem_read_addr_s_full_n,\n\
                     output wire mem_read_addr_s_write,\n\
                     input wire [63:0] mem_read_addr_offset,\n\
                     input wire [512:0] mem_read_data_s_dout,\n\
                     input wire mem_read_data_s_empty_n,\n\
                     output wire mem_read_data_s_read\n\
                     ); endmodule",
                ),
            )
            .unwrap();

        generate_rtl(&mut state).unwrap();

        let top_v = &state.generated_files["top.v"];
        assert!(
            top_v.contains("async_mmap #(") && top_v.contains("chan_0__m_axi"),
            "top should instantiate an async_mmap bridge:\n{top_v}"
        );
        assert!(
            top_v.contains("wire [63:0] chan_0_read_addr__din;"),
            "bridge stream wires should be declared:\n{top_v}"
        );
        assert!(
            top_v.contains(".m_axi_ARADDR(m_axi_chan_0_ARADDR)"),
            "bridge should connect to the top-level AXI port:\n{top_v}"
        );
        assert!(
            top_v.contains(".read_data_dout(chan_0_read_data__dout)"),
            "bridge should drive read data stream wire:\n{top_v}"
        );
        assert!(
            top_v.contains(".mem_read_addr_s_din(chan_0_read_addr__din)"),
            "child should consume bridge stream wires:\n{top_v}"
        );
        assert!(
            top_v.contains(".mem_read_data_s_dout({1'b0, chan_0_read_data__dout})"),
            "child read data should get a false EOT bit:\n{top_v}"
        );
        assert!(
            !top_v.contains(".m_axi_mem_ARADDR"),
            "async mmap child should not receive direct AXI ports:\n{top_v}"
        );
    }

    #[test]
    fn test_generate_rtl_top_instantiates_control_s_axi() {
        let prog = program_from_json(serde_json::json!({
            "top": "top",
            "target": "xilinx-hls",
            "tasks": {
                "top": {
                    "level": "upper",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [
                        {"cat": "mmap", "name": "mem", "type": "float*", "width": 32},
                        {"cat": "scalar", "name": "n", "type": "uint64_t", "width": 64}
                    ],
                    "tasks": {
                        "worker": [
                            {"args": {"data": {"arg": "mem", "cat": "mmap"}, "n": {"arg": "n", "cat": "scalar"}}}
                        ]
                    },
                    "fifos": {}
                },
                "worker": {
                    "level": "lower",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [
                        {"cat": "mmap", "name": "data", "type": "float*", "width": 32},
                        {"cat": "scalar", "name": "n", "type": "uint64_t", "width": 64}
                    ],
                    "tasks": {},
                    "fifos": {}
                }
            }
        }));

        let mut state = TopologyWithRtl::new(prog);
        state
            .attach_module(
                "top",
                parse_module(
                    "module top #(\n\
                       parameter C_S_AXI_CONTROL_ADDR_WIDTH = 6,\n\
                       parameter C_S_AXI_CONTROL_DATA_WIDTH = 32\n\
                     ) (\n\
                       input wire ap_clk,\n\
                       input wire ap_rst_n,\n\
                       input wire s_axi_control_AWVALID,\n\
                       output wire s_axi_control_AWREADY,\n\
                       input wire [C_S_AXI_CONTROL_ADDR_WIDTH-1:0] s_axi_control_AWADDR,\n\
                       input wire s_axi_control_WVALID,\n\
                       output wire s_axi_control_WREADY,\n\
                       input wire [C_S_AXI_CONTROL_DATA_WIDTH-1:0] s_axi_control_WDATA,\n\
                       input wire [3:0] s_axi_control_WSTRB,\n\
                       input wire s_axi_control_ARVALID,\n\
                       output wire s_axi_control_ARREADY,\n\
                       input wire [C_S_AXI_CONTROL_ADDR_WIDTH-1:0] s_axi_control_ARADDR,\n\
                       output wire s_axi_control_RVALID,\n\
                       input wire s_axi_control_RREADY,\n\
                       output wire [C_S_AXI_CONTROL_DATA_WIDTH-1:0] s_axi_control_RDATA,\n\
                       output wire [1:0] s_axi_control_RRESP,\n\
                       output wire s_axi_control_BVALID,\n\
                       input wire s_axi_control_BREADY,\n\
                       output wire [1:0] s_axi_control_BRESP,\n\
                       output wire interrupt\n\
                     );\n\
                     wire ap_start;\n\
                     wire ap_done;\n\
                     wire ap_idle;\n\
                     wire ap_ready;\n\
                     wire [63:0] mem_offset;\n\
                     wire [63:0] n;\n\
                     reg [1:0] ap_CS_fsm;\n\
                     always @(posedge ap_clk) begin\n\
                       if (ap_CS_fsm == 2'd0) begin\n\
                       end else begin\n\
                       end\n\
                     end\n\
                     assign ap_done = ap_start;\n\
                     assign ap_ready = ap_start;\n\
                     assign ap_idle = 1'b1;\n\
                     endmodule",
                ),
            )
            .unwrap();
        state
            .attach_module(
                "worker",
                parse_module(
                    "module worker(\n\
                       input wire ap_clk,\n\
                       input wire ap_rst_n,\n\
                       input wire ap_start,\n\
                       output wire ap_done,\n\
                       output wire ap_idle,\n\
                       output wire ap_ready,\n\
                       input wire [63:0] data_offset,\n\
                       input wire [63:0] n\n\
                     );\n\
                     endmodule",
                ),
            )
            .unwrap();

        generate_rtl(&mut state).unwrap();

        let top_v = &state.generated_files["top.v"];
        assert!(top_v.contains("top_control_s_axi"), "got:\n{top_v}");
        assert!(top_v.contains("control_s_axi_U"), "got:\n{top_v}");
        assert!(top_v.contains(".mem_offset(mem_offset)"), "got:\n{top_v}");
        assert!(top_v.contains(".n(n)"), "got:\n{top_v}");
        assert!(
            !top_v.contains("assign ap_done = ap_start"),
            "placeholder ap_done assign should be removed, got:\n{top_v}"
        );
        assert!(
            !top_v.contains("assign ap_ready = ap_start"),
            "placeholder ap_ready assign should be removed, got:\n{top_v}"
        );
        assert!(
            !top_v.contains("ap_CS_fsm"),
            "upper task emission should drop the original HLS FSM body, got:\n{top_v}"
        );
    }

    #[test]
    fn test_generate_rtl_top_control_unrolls_hmap_offsets() {
        let prog = program_from_json(serde_json::json!({
            "top": "top",
            "target": "xilinx-hls",
            "tasks": {
                "top": {
                    "level": "upper",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [
                        {
                            "cat": "mmap",
                            "name": "mem",
                            "type": "float*",
                            "width": 32,
                            "chan_count": 2,
                            "chan_size": 1024
                        }
                    ],
                    "tasks": {
                        "worker": [
                            {"args": {"data": {"arg": "mem", "cat": "mmap"}}}
                        ]
                    },
                    "fifos": {}
                },
                "worker": {
                    "level": "lower",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [
                        {"cat": "mmap", "name": "data", "type": "float*", "width": 32}
                    ],
                    "tasks": {},
                    "fifos": {}
                }
            }
        }));

        let mut state = TopologyWithRtl::new(prog);
        state
            .attach_module(
                "top",
                parse_module(
                    "module top #(\n\
                       parameter C_S_AXI_CONTROL_ADDR_WIDTH = 6,\n\
                       parameter C_S_AXI_CONTROL_DATA_WIDTH = 32\n\
                     ) (\n\
                       input wire ap_clk,\n\
                       input wire ap_rst_n,\n\
                       input wire s_axi_control_AWVALID,\n\
                       output wire s_axi_control_AWREADY,\n\
                       input wire [C_S_AXI_CONTROL_ADDR_WIDTH-1:0] s_axi_control_AWADDR,\n\
                       input wire s_axi_control_WVALID,\n\
                       output wire s_axi_control_WREADY,\n\
                       input wire [C_S_AXI_CONTROL_DATA_WIDTH-1:0] s_axi_control_WDATA,\n\
                       input wire [3:0] s_axi_control_WSTRB,\n\
                       input wire s_axi_control_ARVALID,\n\
                       output wire s_axi_control_ARREADY,\n\
                       input wire [C_S_AXI_CONTROL_ADDR_WIDTH-1:0] s_axi_control_ARADDR,\n\
                       output wire s_axi_control_RVALID,\n\
                       input wire s_axi_control_RREADY,\n\
                       output wire [C_S_AXI_CONTROL_DATA_WIDTH-1:0] s_axi_control_RDATA,\n\
                       output wire [1:0] s_axi_control_RRESP,\n\
                       output wire s_axi_control_BVALID,\n\
                       input wire s_axi_control_BREADY,\n\
                       output wire [1:0] s_axi_control_BRESP,\n\
                       output wire interrupt\n\
                     );\n\
                     wire ap_start;\n\
                     wire ap_done;\n\
                     wire ap_idle;\n\
                     wire ap_ready;\n\
                     wire [63:0] mem_0_offset;\n\
                     wire [63:0] mem_1_offset;\n\
                     endmodule",
                ),
            )
            .unwrap();
        state
            .attach_module(
                "worker",
                parse_module("module worker(input wire ap_clk, input wire ap_rst_n); endmodule"),
            )
            .unwrap();

        generate_rtl(&mut state).unwrap();

        let top_v = &state.generated_files["top.v"];
        assert!(
            top_v.contains(".mem_0_offset(mem_0_offset)"),
            "got:\n{top_v}"
        );
        assert!(
            top_v.contains(".mem_1_offset(mem_1_offset)"),
            "got:\n{top_v}"
        );
        assert!(
            !top_v.contains(".mem_offset(mem_offset)"),
            "hmap control offsets should remain unrolled, got:\n{top_v}"
        );
    }
}
