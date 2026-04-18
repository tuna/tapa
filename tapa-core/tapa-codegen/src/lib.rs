//! RTL code generation from the TAPA topology model.
//!
//! This crate replaces the Python codegen pipeline
//! (`tapa/program_codegen/`, `tapa/task_codegen/`, `tapa/codegen/`).
//! It uses the `tapa-rtl` builder API to construct Verilog fragments
//! and the hybrid mutation API to modify existing HLS modules.

pub mod children;
pub mod error;
pub mod fifos;
pub mod fsm;
pub mod instance_signals;
pub mod m_axi;
pub mod program;
pub mod rtl_state;

use tapa_rtl::builder::{ContinuousAssign, Expr};
use tapa_rtl::mutation::wire;
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
pub fn generate_rtl(
    state: &mut TopologyWithRtl,
) -> Result<(), CodegenError> {
    let task_names: Vec<String> = state.program.tasks.keys().cloned().collect();

    for task_name in &task_names {
        let task = &state.program.tasks[task_name];
        if task.level != TaskLevel::Upper {
            continue;
        }
        instrument_upper_task(state, task_name)?;
    }

    // Collect emitted files
    for (name, mm) in &state.module_map {
        state
            .generated_files
            .insert(format!("{name}.v"), mm.emit());
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
fn instrument_upper_task(
    state: &mut TopologyWithRtl,
    task_name: &str,
) -> Result<(), CodegenError> {
    let is_top_task = task_name == state.program.top;
    let task = &state.program.tasks[task_name];

    // Check if this is a template task (no child instances)
    let is_template = task.tasks.is_empty();

    if let Some(mm) = state.module_map.get_mut(task_name) {
        mm.cleanup_hls_artifacts();
        let _ = mm.add_signal(wire("ap_rst"));
        mm.add_assign(ContinuousAssign::new(
            Expr::ident("ap_rst"),
            Expr::logical_not(Expr::ident("ap_rst_n")),
        ));

        if is_top_task {
            mm.add_comment("RS clk port=ap_clk".to_owned());
            mm.add_comment("RS rst port=ap_rst_n active=low".to_owned());

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
                let comma = if i + 1 < mm.inner.ports.len() { "," } else { "" };
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
    for conn in mmap_conns.values() {
        if m_axi::needs_crossbar(conn) {
            for (slave_idx, (task, idx, _port)) in conn.args.iter().enumerate() {
                #[allow(clippy::cast_possible_truncation, reason = "index fits")]
                let idx_usize = *idx as usize;
                mmap_slave_map.insert(
                    (conn.arg_name.clone(), task.clone(), idx_usize),
                    slave_idx,
                );
            }
        }
    }

    let (is_done_signals, instance_infos) =
        generate_child_signals(state, task_name, &mmap_slave_map);

    instantiate_fifos(state, task_name);

    connect_fifos(state, task_name);

    // Add M-AXI ports and crossbars (reuse pre-computed mmap connections)
    add_m_axi_and_crossbars(state, task_name, &mmap_conns)?;

    // Add FSM pragmas
    let scalar_ports: Vec<String> = state.program.tasks[task_name]
        .ports
        .iter()
        .filter(|p| matches!(p.cat, tapa_task_graph::port::ArgCategory::Scalar))
        .map(|p| p.name.clone())
        .collect();

    if let Some(fsm_mm) = state.fsm_modules.get_mut(task_name) {
        fsm::add_rs_pragmas_to_fsm(fsm_mm, &scalar_ports, &instance_infos);
        program::apply_global_fsm(fsm_mm, &is_done_signals);
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
    mmap_slave_map: &std::collections::BTreeMap<(String, String, usize), usize>,
) -> (Vec<String>, Vec<(String, bool)>) {
    use std::collections::BTreeMap;
    use tapa_topology::instance::ArgDesign;

    type ChildEntry = (usize, bool, BTreeMap<String, ArgDesign>);

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
                .map(|(idx, inst)| (idx, inst.step < 0, inst.args.clone()))
                .collect();
            (name.clone(), entries)
        })
        .collect();

    for (child_name, entries) in child_entries {
        for (idx, is_autorun, args) in entries {
            let inst_name = format!("{child_name}_{idx}");
            let sig = instance_signals::InstanceSignals::new(&inst_name, is_autorun);

            // Parent task module: only declare handshake WIRES (not state regs)
            // The parent sees start/done/idle/ready/is_done as wires connected to FSM ports
            if let Some(mm) = state.module_map.get_mut(task_name) {
                // For non-autorun: wire signals only (state owned by FSM)
                // For autorun: start reg (driven by autorun logic in FSM)
                for signal in sig.all_signals() {
                    let _ = mm.add_signal(signal);
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
                    fsm_mm.add_always(children::generate_autorun_start(&sig));
                } else {
                    // State register and FSM logic owned by FSM module
                    for signal in sig.all_signals() {
                        let _ = fsm_mm.add_signal(signal);
                    }
                    // Use pipelined start_q/done_q from global FSM
                    let start_input = Expr::ident(program::START_Q);
                    let done_release = Expr::ident(program::DONE_Q);
                    fsm_mm.add_always(children::generate_child_fsm(&sig, start_input, done_release));
                    // ap_start output: combinationally driven from state
                    fsm_mm.add_assign(children::generate_start_assign(&sig));
                    // is_done: driven from state inside FSM module
                    fsm_mm.add_assign(children::generate_is_done_assign(&sig));
                }
            }

            // Declare per-instance pipeline signals for scalar/mmap args
            declare_instance_pipeline_signals(state, task_name, &inst_name, &args);

            // Add pipeline portargs to FSM instantiation
            for (port_name, arg) in &args {
                match arg.cat {
                    tapa_task_graph::port::ArgCategory::Scalar => {
                        let pipeline_out = format!("{inst_name}__{port_name}");
                        let fsm_in_port = format!("{inst_name}__{port_name}_in");
                        fsm_portargs.push(tapa_rtl::builder::PortArg::new(
                            &fsm_in_port,
                            Expr::ident(&arg.arg),
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
                        fsm_portargs.push(tapa_rtl::builder::PortArg::new(
                            &fsm_in_port,
                            Expr::ident(format!("{}_offset", arg.arg)),
                        ));
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
            let mut inst_mmap_slaves = std::collections::BTreeMap::new();
            for arg in args.values() {
                if matches!(
                    arg.cat,
                    tapa_task_graph::port::ArgCategory::Mmap
                        | tapa_task_graph::port::ArgCategory::AsyncMmap
                ) {
                    if let Some(&slave_idx) = mmap_slave_map.get(&(
                        arg.arg.clone(),
                        child_name.clone(),
                        idx,
                    )) {
                        inst_mmap_slaves.insert(arg.arg.clone(), slave_idx);
                    }
                }
            }

            // Build and add the actual child module instance to parent
            let child_inst = children::build_child_instance(
                &child_name,
                &inst_name,
                &sig,
                &args,
                &inst_mmap_slaves,
            );
            if let Some(mm) = state.module_map.get_mut(task_name) {
                mm.add_instance(child_inst);
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

/// Declare per-instance pipeline signals for scalar and mmap arguments.
///
/// Creates FSM-owned pipeline ports and parent-side wires:
/// - FSM module gets an input port for the parent arg and an output port for the pipeline signal
/// - Parent module gets a wire for the pipeline output
/// - The FSM module uses a registered pipeline (always @(posedge clk)) to
///   delay the signal by one cycle, matching Python's `add_pipeline` behavior
fn declare_instance_pipeline_signals(
    state: &mut TopologyWithRtl,
    task_name: &str,
    inst_name: &str,
    args: &std::collections::BTreeMap<String, tapa_topology::instance::ArgDesign>,
) {
    for (port_name, arg) in args {
        let (pipeline_out, fsm_in, width) = match arg.cat {
            tapa_task_graph::port::ArgCategory::Scalar => (
                format!("{inst_name}__{port_name}"),
                format!("{inst_name}__{port_name}_in"),
                None, // 1-bit
            ),
            tapa_task_graph::port::ArgCategory::Mmap
            | tapa_task_graph::port::ArgCategory::AsyncMmap => (
                format!("{inst_name}__{port_name}_offset"),
                format!("{inst_name}__{port_name}_offset_in"),
                Some(("63", "0")), // 64-bit
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
    width: Option<(&str, &str)>, // None = 1-bit, Some((msb, lsb))
) {
    // Parent: wire for pipeline output
    if let Some(mm) = state.module_map.get_mut(task_name) {
        let sig = match width {
            Some((msb, lsb)) => tapa_rtl::mutation::wide_wire(pipeline_out, msb, lsb),
            None => tapa_rtl::mutation::wire(pipeline_out),
        };
        let _ = mm.add_signal(sig);
    }

    // FSM module: input port + output port + internal _reg + registered always block
    if let Some(fsm_mm) = state.fsm_modules.get_mut(task_name) {
        let reg_name = format!("{pipeline_out}_reg");
        let (in_port, out_port, reg_sig) = match width {
            Some((msb, lsb)) => (
                tapa_rtl::mutation::wide_port(fsm_in_port, tapa_rtl::port::Direction::Input, msb, lsb),
                tapa_rtl::mutation::wide_port(pipeline_out, tapa_rtl::port::Direction::Output, msb, lsb),
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

/// Resolve the width of an M-AXI suffix from protocol metadata.
/// FIFO entry: (name, depth, `is_consumed`, `producer_endpoint`).
type FifoEntry = (String, Option<u32>, bool, Option<(String, u32)>);
/// FIFO connection entry: (name, depth, `has_consumer`, `has_producer`, `producer_endpoint`).
type FifoConnEntry = (String, Option<u32>, bool, bool, Option<(String, u32)>);

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
            // Extract producer endpoint for width resolution
            let producer = fifo.produced_by.as_ref().map(|ep| {
                (ep.0.clone(), ep.1)
            });
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
            let assigns = fifos::build_external_fifo_assigns(
                &fifo_name,
                &fifo_name,
                is_consumed,
            );
            if let Some(mm) = state.module_map.get_mut(task_name) {
                for assign in assigns {
                    mm.add_assign(assign);
                }
            }
        }
    }
}

/// Resolve FIFO width from the producer child's attached RTL module.
///
/// Looks for a `*_dout` port on the producer child's parsed module
/// and uses its width. Falls back to topology port width, then 32.
fn resolve_fifo_width(
    state: &TopologyWithRtl,
    producer: Option<&(String, u32)>,
) -> u32 {
    if let Some((task_name, _idx)) = producer {
        // Check attached RTL module for producer port width
        if let Some(mm) = state.module_map.get(task_name.as_str()) {
            // Look for an ostream _dout port — its width is the FIFO data width
            for port in &mm.inner.ports {
                if port.name.ends_with("_dout") || port.name.ends_with("_din") {
                    if let Some(w) = &port.width {
                        // Width tokens → parse MSB as the width
                        let msb_str: String = w.msb.iter().map(|t| t.repr.as_str()).collect::<Vec<_>>().join("");
                        if let Ok(msb) = msb_str.parse::<u32>() {
                            return msb + 1; // [msb:0] means msb+1 bits
                        }
                    }
                }
            }
        }
        // Fallback: check topology port definitions for the producer task
        if let Some(task) = state.program.tasks.get(task_name.as_str()) {
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

/// Connect FIFOs: declare inter-task wires and connect external FIFOs.
///
/// For internal FIFOs (both endpoints in this task): declare wires with
/// proper width using stream suffixes so child instances can connect.
/// For external FIFOs: connect to parent module ports, potentially
/// through AXIS adapters.
fn connect_fifos(state: &mut TopologyWithRtl, task_name: &str) {
    use tapa_protocol::{ISTREAM_SUFFIXES, OSTREAM_SUFFIXES};

    let task = &state.program.tasks[task_name];

    // Collect FIFO connection info with producer endpoint for width resolution
    let fifo_entries: Vec<FifoConnEntry> = task
        .fifos
        .iter()
        .map(|(name, fifo)| {
            let has_consumer = fifo.consumed_by.is_some();
            let has_producer = fifo.produced_by.is_some();
            let producer = fifo.produced_by.as_ref().map(|ep| (ep.0.clone(), ep.1));
            (name.clone(), fifo.depth, has_consumer, has_producer, producer)
        })
        .collect();

    for (fifo_name, depth, has_consumer, has_producer, producer) in &fifo_entries {
        // Resolve width from producer child's attached RTL
        let width = resolve_fifo_width(state, producer.as_ref());

        if depth.is_some() && *has_consumer && *has_producer {
            // Internal FIFO: declare wires for both read and write sides
            if let Some(mm) = state.module_map.get_mut(task_name) {
                // Declare wires for each FIFO suffix (read side)
                for suffix in ISTREAM_SUFFIXES {
                    let wire_name = format!("{fifo_name}{suffix}");
                    let sig = if suffix.contains("dout") {
                        tapa_rtl::mutation::wide_wire(&wire_name, &(width - 1).to_string(), "0")
                    } else {
                        tapa_rtl::mutation::wire(&wire_name)
                    };
                    let _ = mm.add_signal(sig);
                }
                // Declare wires for write side
                for suffix in OSTREAM_SUFFIXES {
                    let wire_name = format!("{fifo_name}{suffix}");
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
            // Check if this should be an AXIS adapter
            let is_axis = task
                .ports
                .iter()
                .any(|p| p.name == *fifo_name && p.name.contains("axis"));

            if is_axis {
                // Instantiate AXIS adapter
                let is_input = *has_consumer;
                let adapter = fifos::build_axis_adapter(fifo_name, is_input);
                if let Some(mm) = state.module_map.get_mut(task_name) {
                    mm.add_instance(adapter);
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
            m_axi::add_m_axi_ports(mm, &conn.arg_name, conn.data_width, 64);
        }
        if m_axi::needs_crossbar(conn) {
            // Declare downstream m_axi_{arg}_{idx}_* wires in parent
            // Size each wire using protocol metadata for correct widths
            if let Some(mm) = state.module_map.get_mut(task_name) {
                for (slave_idx, _) in conn.args.iter().enumerate() {
                    let wire_prefix = format!("m_axi_{}_{slave_idx}", conn.arg_name);
                    for suffix in tapa_protocol::M_AXI_SUFFIXES_COMPACT {
                        let wire_name = format!("{wire_prefix}{suffix}");
                        // Resolve width from suffix name using protocol constants
                        let width = m_axi::resolve_suffix_width(suffix, conn.data_width);
                        let sig = if width > 1 {
                            tapa_rtl::mutation::wide_wire(
                                &wire_name,
                                &(width - 1).to_string(),
                                "0",
                            )
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

        // Downstream wires m_axi_mem_0_* and m_axi_mem_1_* should be declared
        assert!(
            top_v.contains("m_axi_mem_0_"),
            "parent should have m_axi_mem_0_* wires, got:\n{top_v}"
        );
        assert!(
            top_v.contains("m_axi_mem_1_"),
            "parent should have m_axi_mem_1_* wires, got:\n{top_v}"
        );

        // Crossbar auxiliary RTL file should be generated
        assert!(
            state.generated_files.keys().any(|k| k.contains("axi_crossbar")),
            "should emit crossbar RTL file, got keys: {:?}",
            state.generated_files.keys().collect::<Vec<_>>()
        );
    }
}
