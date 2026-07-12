//! Child task instantiation with FSM/port wiring: per-instance FSM
//! generation, argument pipelines, handshake wiring, and portarg
//! assembly.

use std::collections::BTreeMap;

use tapa_protocol::{
    HANDSHAKE_CLK, HANDSHAKE_DONE, HANDSHAKE_IDLE, HANDSHAKE_READY, HANDSHAKE_RST, HANDSHAKE_RST_N,
    HANDSHAKE_START, ISTREAM_SUFFIXES, OSTREAM_SUFFIXES,
};
use tapa_rtl::builder::{
    AlwaysBlock, CaseItem, ContinuousAssign, Expr, ModuleInstance, PortArg, Sensitivity, Statement,
};
use tapa_rtl::module::sanitize_array_name;
use tapa_rtl::VerilogModule;
use tapa_task_graph::port::ArgCategory;
use tapa_topology::instance::ArgDesign;

use crate::instance_signals::InstanceSignals;
use crate::rtl_state::TopologyWithRtl;
use crate::{async_mmap, instance_signals, m_axi, program};

/// FSM state constants for non-autorun child instances (2-bit encoding).
pub const STATE_IDLE: &str = "2'b00";
pub const STATE_RUNNING: &str = "2'b01";
pub const STATE_WAITING: &str = "2'b11";
pub const STATE_DONE: &str = "2'b10";

/// Generate the 4-state FSM always block for a non-autorun child instance.
///
/// States: IDLE(00) -> RUNNING(01) -> WAITING(11) or DONE(10) -> IDLE(00)
///
/// `start_input`: the signal that triggers IDLE->RUNNING transition.
/// `done_release`: the signal that releases `STATE_DONE` back to IDLE
/// (from the global done pipeline, so all children hold done until
/// the program FSM acknowledges completion).
pub fn generate_child_fsm(
    sig: &InstanceSignals,
    start_input: Expr,
    done_release: Expr,
) -> AlwaysBlock {
    let state = sig.state_expr();
    let done = sig.done_expr();
    let ready = Expr::ident(sig.ready_name());

    AlwaysBlock::posedge(
        HANDSHAKE_CLK,
        vec![Statement::If {
            cond: Expr::ident(HANDSHAKE_RST),
            then_body: vec![sig.set_state(Expr::lit(STATE_IDLE))],
            else_body: vec![Statement::Case {
                expr: state,
                items: vec![
                    // IDLE -> RUNNING when global start pipeline asserts
                    CaseItem::new(
                        Expr::lit(STATE_IDLE),
                        vec![Statement::If {
                            cond: start_input,
                            then_body: vec![sig.set_state(Expr::lit(STATE_RUNNING))],
                            else_body: vec![],
                        }],
                    ),
                    // RUNNING -> DONE if ready&done, WAITING if ready&!done
                    CaseItem::new(
                        Expr::lit(STATE_RUNNING),
                        vec![Statement::If {
                            cond: Expr::logical_and(ready.clone(), done.clone()),
                            then_body: vec![sig.set_state(Expr::lit(STATE_DONE))],
                            else_body: vec![Statement::If {
                                cond: ready,
                                then_body: vec![sig.set_state(Expr::lit(STATE_WAITING))],
                                else_body: vec![],
                            }],
                        }],
                    ),
                    // WAITING -> DONE when done
                    CaseItem::new(
                        Expr::lit(STATE_WAITING),
                        vec![Statement::If {
                            cond: done,
                            then_body: vec![sig.set_state(Expr::lit(STATE_DONE))],
                            else_body: vec![],
                        }],
                    ),
                    // DONE -> IDLE only when global done pipeline releases
                    CaseItem::new(
                        Expr::lit(STATE_DONE),
                        vec![Statement::If {
                            cond: done_release,
                            then_body: vec![sig.set_state(Expr::lit(STATE_IDLE))],
                            else_body: vec![],
                        }],
                    ),
                ],
                default: vec![sig.set_state(Expr::lit(STATE_IDLE))],
            }],
        }],
    )
}

/// Generate an `__is_done` assign inside the FSM module.
///
/// `assign is_done = (state == STATE_DONE)`
pub fn generate_is_done_assign(sig: &InstanceSignals) -> ContinuousAssign {
    ContinuousAssign::new(
        Expr::ident(sig.is_done_name()),
        sig.is_state(Expr::lit(STATE_DONE)),
    )
}

/// Generate the start logic for an autorun child instance.
///
/// Autorun instances latch their start signal when the global `ap_start`
/// is first asserted and keep it high until reset.
pub fn generate_autorun_start(sig: &InstanceSignals) -> AlwaysBlock {
    AlwaysBlock::new(
        Sensitivity::Posedge(HANDSHAKE_CLK.into()),
        vec![Statement::If {
            cond: Expr::ident(HANDSHAKE_RST),
            then_body: vec![Statement::NonblockingAssign {
                lhs: sig.start_expr(),
                rhs: Expr::lit("1'b0"),
            }],
            else_body: vec![Statement::If {
                cond: Expr::ident(HANDSHAKE_START),
                then_body: vec![Statement::NonblockingAssign {
                    lhs: sig.start_expr(),
                    rhs: Expr::lit("1'b1"),
                }],
                else_body: vec![],
            }],
        }],
    )
}

/// Generate the combinational start assign for a non-autorun instance.
///
/// `instance_start = (state == STATE_RUNNING)`
pub fn generate_start_assign(sig: &InstanceSignals) -> ContinuousAssign {
    ContinuousAssign::new(sig.start_expr(), sig.is_state(Expr::lit(STATE_RUNNING)))
}

/// Build a child task `ModuleInstance` with all port argument bindings.
///
/// Connects handshake signals (from `InstanceSignals`), scalar arguments,
/// stream arguments (istream/ostream suffixes), and mmap offset arguments.
///
/// Mmap bindings describe how each child mmap argument reaches parent AXI wires.
#[derive(Debug, Default)]
pub struct ChildMmapBindings {
    pub slave_indices: BTreeMap<String, usize>,
    pub wire_id_widths: BTreeMap<String, u32>,
    pub child_id_widths: BTreeMap<String, u32>,
}

impl ChildMmapBindings {
    pub fn wire_prefix(&self, arg_name: &str) -> String {
        mmap_wire_prefix(arg_name, &self.slave_indices)
    }

    pub fn wire_id_width(&self, arg_name: &str) -> Option<u32> {
        self.wire_id_widths.get(arg_name).copied()
    }

    pub fn child_id_width(&self, arg_name: &str) -> Option<u32> {
        self.child_id_widths.get(arg_name).copied()
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "child instance assembly is inherently sequential; \
              splitting would fragment the port-arg wiring logic"
)]
pub fn build_child_instance(
    child_task_name: &str,
    instance_name: &str,
    sig: &InstanceSignals,
    args: &BTreeMap<String, ArgDesign>,
    mmap_bindings: &ChildMmapBindings,
    child_rtl: Option<&VerilogModule>,
) -> ModuleInstance {
    let mut port_args = Vec::new();

    // Clock and reset
    port_args.push(PortArg::new(HANDSHAKE_CLK, Expr::ident(HANDSHAKE_CLK)));
    port_args.push(PortArg::new(HANDSHAKE_RST_N, Expr::ident(HANDSHAKE_RST_N)));

    // Handshake signals from InstanceSignals
    port_args.extend(sig.instance_portargs());

    let resolve_child_stream_port = |name: &str, suffix: &str| {
        child_rtl
            .and_then(|module| module.get_port_of(name, suffix))
            .map_or_else(
                || format!("{}_s{}", sanitize_array_name(name), suffix),
                |p| p.name.clone(),
            )
    };
    let resolve_child_stream_peek_port = |name: &str, suffix: &str| {
        let module = child_rtl?;
        let peek_suffix = format!("_peek{suffix}");
        if let Some((base, idx, tail)) = array_name_parts(name) {
            let candidate = format!("{base}_peek_{idx}{tail}{suffix}");
            return module.find_port(&candidate).map(|p| p.name.clone());
        }
        module
            .get_port_of(name, &peek_suffix)
            .map(|p| p.name.clone())
    };
    let stream_signal =
        |name: &str, suffix: &str| format!("{}{}", sanitize_array_name(name), suffix);

    // Argument port bindings
    for (child_port, arg) in args {
        match arg.cat {
            ArgCategory::Scalar => {
                // Scalar: connect to per-instance pipeline signal
                let pipeline_name = format!("{instance_name}__{child_port}");
                port_args.push(PortArg::new(
                    child_port.as_str(),
                    Expr::ident(pipeline_name),
                ));
            }
            ArgCategory::Istream | ArgCategory::Istreams => {
                // Input stream: connect with ISTREAM_SUFFIXES
                for suffix in ISTREAM_SUFFIXES {
                    let signal = Expr::ident(stream_signal(&arg.arg, suffix));
                    port_args.push(PortArg::new(
                        resolve_child_stream_port(child_port, suffix),
                        signal.clone(),
                    ));
                    if matches!(*suffix, "_dout" | "_empty_n") {
                        if let Some(peek_port) = resolve_child_stream_peek_port(child_port, suffix)
                        {
                            port_args.push(PortArg::new(peek_port, signal));
                        }
                    }
                }
            }
            ArgCategory::Ostream | ArgCategory::Ostreams => {
                // Output stream: connect with OSTREAM_SUFFIXES
                for suffix in OSTREAM_SUFFIXES {
                    port_args.push(PortArg::new(
                        resolve_child_stream_port(child_port, suffix),
                        Expr::ident(stream_signal(&arg.arg, suffix)),
                    ));
                }
            }
            ArgCategory::Mmap | ArgCategory::AsyncMmap => {
                // Connect to per-instance pipeline offset
                let offset_sig = format!("{instance_name}__{child_port}_offset");
                // Bind M-AXI channel ports:
                // If crossbar exists (slave index present), bind to downstream wires
                // Otherwise bind directly to upstream parent m_axi signals
                let m_axi_wire_prefix = mmap_bindings.wire_prefix(&arg.arg);
                let m_axi_wire_id_width = mmap_bindings.wire_id_width(&arg.arg);
                let child_m_axi_id_width = mmap_bindings.child_id_width(&arg.arg);
                if matches!(arg.cat, ArgCategory::AsyncMmap) {
                    if child_has_direct_mmap_ports(child_rtl, child_port) {
                        let child_rtl_filter =
                            if child_has_direct_mmap_offset(child_rtl, child_port) {
                                None
                            } else {
                                child_rtl
                            };
                        add_direct_mmap_portargs(
                            &mut port_args,
                            child_port,
                            &offset_sig,
                            &m_axi_wire_prefix,
                            m_axi_wire_id_width,
                            child_m_axi_id_width,
                            child_rtl_filter,
                            child_rtl,
                        );
                    } else if let Some(module) = child_rtl {
                        let bridge_base =
                            crate::async_mmap::bridge_base_from_m_axi_prefix(&m_axi_wire_prefix);
                        port_args.extend(crate::async_mmap::child_portargs(
                            module,
                            child_port,
                            &bridge_base,
                            &offset_sig,
                        ));
                    }
                } else {
                    add_direct_mmap_portargs(
                        &mut port_args,
                        child_port,
                        &offset_sig,
                        &m_axi_wire_prefix,
                        m_axi_wire_id_width,
                        child_m_axi_id_width,
                        None,
                        child_rtl,
                    );
                }
            }
            ArgCategory::Immap | ArgCategory::Ommap => {
                // Other categories: direct connection
                port_args.push(PortArg::new(
                    sanitize_array_name(child_port),
                    Expr::ident(sanitize_array_name(&arg.arg)),
                ));
            }
        }
    }

    ModuleInstance::new(child_task_name, instance_name).with_ports(port_args)
}

fn child_has_direct_mmap_offset(child_rtl: Option<&VerilogModule>, child_port: &str) -> bool {
    child_rtl.is_some_and(|module| module.find_port(&format!("{child_port}_offset")).is_some())
}

fn child_has_direct_mmap_ports(child_rtl: Option<&VerilogModule>, child_port: &str) -> bool {
    let Some(module) = child_rtl else {
        return false;
    };
    child_has_direct_mmap_offset(child_rtl, child_port)
        || tapa_protocol::M_AXI_SUFFIXES_COMPACT.iter().any(|suffix| {
            module
                .find_port(&format!("m_axi_{child_port}{suffix}"))
                .is_some()
        })
}

#[allow(
    clippy::too_many_arguments,
    reason = "mmap port arg wiring needs all 8 parameters"
)]
fn add_direct_mmap_portargs(
    port_args: &mut Vec<PortArg>,
    child_port: &str,
    offset_sig: &str,
    m_axi_wire_prefix: &str,
    m_axi_wire_id_width: Option<u32>,
    child_m_axi_id_width: Option<u32>,
    child_rtl_filter: Option<&VerilogModule>,
    child_rtl_for_width: Option<&VerilogModule>,
) {
    let offset_port = format!("{child_port}_offset");
    if child_rtl_filter.is_none_or(|module| module.find_port(&offset_port).is_some()) {
        port_args.push(PortArg::new(offset_port, Expr::ident(offset_sig)));
    }
    for suffix in tapa_protocol::M_AXI_SUFFIXES_COMPACT {
        let child_axi_port = format!("m_axi_{child_port}{suffix}");
        if child_rtl_filter.is_none_or(|module| module.find_port(&child_axi_port).is_some()) {
            let wire_name = format!("{m_axi_wire_prefix}{suffix}");
            port_args.push(PortArg::new(
                child_axi_port.as_str(),
                direct_mmap_connection_expr(
                    child_rtl_for_width,
                    &child_axi_port,
                    &wire_name,
                    suffix,
                    m_axi_wire_id_width,
                    child_m_axi_id_width,
                ),
            ));
        }
    }
}

fn direct_mmap_connection_expr(
    child_rtl: Option<&VerilogModule>,
    child_axi_port: &str,
    wire_name: &str,
    suffix: &str,
    m_axi_wire_id_width: Option<u32>,
    child_m_axi_id_width: Option<u32>,
) -> Expr {
    let is_id = matches!(suffix, "_ARID" | "_AWID" | "_BID" | "_RID");
    let Some(target_width) = m_axi_wire_id_width else {
        return Expr::ident(wire_name);
    };
    if !is_id || target_width <= 1 {
        return Expr::ident(wire_name);
    }
    let child_width = child_m_axi_id_width
        .or_else(|| {
            child_rtl
                .and_then(|module| module.find_port(child_axi_port))
                .and_then(tapa_rtl::port::Port::bit_width)
        })
        .unwrap_or(target_width);
    if child_width >= target_width {
        return Expr::ident(wire_name);
    }
    Expr::range(
        Expr::ident(wire_name),
        Expr::int(u64::from(child_width - 1)),
        Expr::int(0),
    )
}

pub fn mmap_wire_prefix(
    arg_name: &str,
    mmap_slave_indices: &std::collections::BTreeMap<String, usize>,
) -> String {
    let sanitized_arg = sanitize_array_name(arg_name);
    if let Some(slave_idx) = mmap_slave_indices.get(arg_name) {
        crate::m_axi::crossbar_slave_prefix(&sanitized_arg, *slave_idx)
    } else {
        format!("m_axi_{sanitized_arg}")
    }
}

fn array_name_parts(name: &str) -> Option<(&str, &str, &str)> {
    let left = name.find('[')?;
    let right = name[left + 1..].find(']')? + left + 1;
    Some((&name[..left], &name[left + 1..right], &name[right + 1..]))
}

/// Generate child instance signals, FSM/autorun logic, and actual child module instances.
///
/// FSM/start logic goes into the FSM module (not the parent task module).
/// The FSM module is then instantiated into the parent task module.
#[allow(clippy::too_many_lines, reason = "sequential child signal generation")]
pub(crate) fn generate_child_signals(
    state: &mut TopologyWithRtl,
    task_name: &str,
    mmap_conns: &std::collections::BTreeMap<String, crate::rtl_state::MMapConnection>,
    mmap_slave_map: &std::collections::BTreeMap<(String, String, usize), usize>,
) -> (Vec<String>, Vec<(String, bool)>) {
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
        // One clone per distinct child task (shared across instances):
        // `VerilogModule` carries the full RTL source text.
        let child_rtl = state
            .module_map
            .get(&child_name)
            .map(|module| module.inner.clone());
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
                for signal in sig.all_signals() {
                    let _ = fsm_mm.add_signal(signal);
                }
                if is_autorun {
                    fsm_mm.ensure_signal_kind(&sig.start_name(), tapa_rtl::signal::SignalKind::Reg);
                    fsm_mm.add_always(generate_autorun_start(&sig));
                } else {
                    // State register and FSM logic owned by FSM module
                    // Use pipelined start_q/done_q from global FSM
                    let start_input = Expr::ident(program::START_Q);
                    let done_release = Expr::ident(program::DONE_Q);
                    fsm_mm.add_always(generate_child_fsm(&sig, start_input, done_release));
                    // ap_start output: combinationally driven from state
                    fsm_mm.add_assign(generate_start_assign(&sig));
                    // is_done: driven from state inside FSM module
                    fsm_mm.add_assign(generate_is_done_assign(&sig));
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
                                if conn.channel_count() > 1 {
                                    Expr::lit("64'd0")
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
            let mut mmap_bindings = ChildMmapBindings::default();
            for arg in args.values() {
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
                            if let Some(slave) = conn.slaves.get(slave_idx) {
                                mmap_bindings
                                    .child_id_widths
                                    .insert(arg.arg.clone(), slave.id_width);
                            }
                        }
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
                    // Aggregation already derived the width with the same
                    // parent-then-child port precedence.
                    let data_width = mmap_conns.get(&arg.arg).map_or(64, |c| c.data_width);
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
            let child_inst = build_child_instance(
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
                crate::m_axi::add_crossbar_slave_id_padding(
                    state,
                    task_name,
                    &args,
                    &mmap_bindings,
                );
            }

            instance_infos.push((inst_name, is_autorun));
        }
    }

    // Instantiate FSM module into parent task module
    let fsm_module_name = format!("{task_name}_fsm");
    let mut fsm_inst_ports = vec![
        tapa_rtl::builder::PortArg::new(HANDSHAKE_CLK, Expr::ident(HANDSHAKE_CLK)),
        tapa_rtl::builder::PortArg::new(HANDSHAKE_RST_N, Expr::ident(HANDSHAKE_RST_N)),
        // Top-level handshake ports
        tapa_rtl::builder::PortArg::new(HANDSHAKE_START, Expr::ident(HANDSHAKE_START)),
        tapa_rtl::builder::PortArg::new(HANDSHAKE_DONE, Expr::ident(HANDSHAKE_DONE)),
        tapa_rtl::builder::PortArg::new(HANDSHAKE_IDLE, Expr::ident(HANDSHAKE_IDLE)),
        tapa_rtl::builder::PortArg::new(HANDSHAKE_READY, Expr::ident(HANDSHAKE_READY)),
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
        add_pipeline_stage(state, task_name, &pipeline_out, &fsm_in, width.as_ref());
    }
}

/// Add a registered pipeline stage: parent wire + FSM input/output ports + internal reg.
fn add_pipeline_stage(
    state: &mut TopologyWithRtl,
    task_name: &str,
    pipeline_out: &str,
    fsm_in_port: &str,
    width: Option<&(String, String)>, // None = 1-bit, Some((msb, lsb))
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
            HANDSHAKE_CLK,
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
                    tapa_rtl::expression::expression_source(&width.msb),
                    tapa_rtl::expression::expression_source(&width.lsb),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn child_fsm_has_four_states_with_done_hold() {
        let sig = InstanceSignals::new("child_0", false);
        let start_input = Expr::ident("global_start_q");
        let done_release = Expr::ident("global_done_q");
        let block = generate_child_fsm(&sig, start_input, done_release);
        let text = block.to_string();
        assert!(text.contains("case (child_0__state)"), "got:\n{text}");
        assert!(text.contains(STATE_IDLE), "got:\n{text}");
        assert!(text.contains(STATE_RUNNING), "got:\n{text}");
        assert!(text.contains(STATE_DONE), "got:\n{text}");
        // IDLE->RUNNING uses global_start_q
        assert!(
            text.contains("global_start_q"),
            "should use start input, got:\n{text}"
        );
        // DONE->IDLE gated by global_done_q (not unconditional)
        assert!(
            text.contains("global_done_q"),
            "DONE->IDLE should be gated by done_release, got:\n{text}"
        );
    }

    #[test]
    fn autorun_start_latches_until_reset() {
        let sig = InstanceSignals::new("auto_inst", true);
        let block = generate_autorun_start(&sig);
        let text = block.to_string();
        assert!(
            text.contains("if (ap_start)")
                && text.contains("auto_inst__ap_start <= 1'b1")
                && text.contains("auto_inst__ap_start <= 1'b0"),
            "got:\n{text}"
        );
        assert!(
            !text.contains("auto_inst__ap_start <= ap_start"),
            "autorun start must not deassert with the host start pulse:\n{text}"
        );
    }

    #[test]
    fn start_assign_checks_running_state() {
        let sig = InstanceSignals::new("child_0", false);
        let assign = generate_start_assign(&sig);
        let text = assign.to_string();
        assert!(
            text.contains("child_0__ap_start") && text.contains(STATE_RUNNING),
            "got: {text}"
        );
    }

    #[test]
    fn build_child_instance_has_handshake_and_args() {
        use std::collections::BTreeMap;
        let sig = InstanceSignals::new("worker_0", false);
        let mut args = BTreeMap::new();
        args.insert(
            "data_in".to_owned(),
            ArgDesign {
                arg: "fifo_0".to_owned(),
                cat: ArgCategory::Istream,
                extra: BTreeMap::new(),
            },
        );
        args.insert(
            "size".to_owned(),
            ArgDesign {
                arg: "n".to_owned(),
                cat: ArgCategory::Scalar,
                extra: BTreeMap::new(),
            },
        );
        let inst = build_child_instance(
            "worker",
            "worker_0",
            &sig,
            &args,
            &ChildMmapBindings::default(),
            None,
        );
        let text = inst.to_string();
        // Should have module name and instance name
        assert!(text.contains("worker worker_0"), "got:\n{text}");
        // Should have handshake ports
        assert!(
            text.contains(".ap_start(worker_0__ap_start)"),
            "got:\n{text}"
        );
        assert!(text.contains(".ap_done(worker_0__ap_done)"), "got:\n{text}");
        // Should have scalar arg connected to per-instance pipeline signal
        assert!(text.contains(".size(worker_0__size)"), "got:\n{text}");
        // Should have istream suffixes
        assert!(text.contains("data_in_s_dout"), "got:\n{text}");
    }

    #[test]
    fn build_child_instance_uses_hls_stream_names_without_child_rtl() {
        use std::collections::BTreeMap;
        let sig = InstanceSignals::new("worker_0", false);
        let mut args = BTreeMap::new();
        args.insert(
            "data_in".to_owned(),
            ArgDesign {
                arg: "fifo_0".to_owned(),
                cat: ArgCategory::Istream,
                extra: BTreeMap::new(),
            },
        );
        args.insert(
            "data_out".to_owned(),
            ArgDesign {
                arg: "fifo_1".to_owned(),
                cat: ArgCategory::Ostream,
                extra: BTreeMap::new(),
            },
        );
        let inst = build_child_instance(
            "worker",
            "worker_0",
            &sig,
            &args,
            &ChildMmapBindings::default(),
            None,
        );
        let text = inst.to_string();
        assert!(
            text.contains(".data_in_s_dout(fifo_0_dout)"),
            "got:\n{text}"
        );
        assert!(text.contains(".data_out_s_din(fifo_1_din)"), "got:\n{text}");
    }

    #[test]
    fn build_child_instance_sanitizes_indexed_stream_names() {
        use std::collections::BTreeMap;
        let sig = InstanceSignals::new("worker_0", false);
        let child_rtl = VerilogModule::parse(
            "module worker(input wire ap_clk, input wire qs_24_Network_s_dout, input wire qs_24_Network_s_empty_n, output wire qs_24_Network_s_read); endmodule",
        )
        .unwrap();
        let mut args = BTreeMap::new();
        args.insert(
            "qs[24]_Network".to_owned(),
            ArgDesign {
                arg: "qs[24]_Network".to_owned(),
                cat: ArgCategory::Istream,
                extra: BTreeMap::new(),
            },
        );
        let inst = build_child_instance(
            "worker",
            "worker_0",
            &sig,
            &args,
            &ChildMmapBindings::default(),
            Some(&child_rtl),
        );
        let text = inst.to_string();
        assert!(
            text.contains(".qs_24_Network_s_dout(qs_24_Network_dout)"),
            "got:\n{text}"
        );
        assert!(
            !text.contains("qs[24]"),
            "indexed names must be sanitized in emitted Verilog:\n{text}"
        );
    }

    #[test]
    fn build_child_instance_connects_istream_peek_inputs() {
        use std::collections::BTreeMap;
        let sig = InstanceSignals::new("switch_0", false);
        let child_rtl = VerilogModule::parse(
            "module switch(\n\
             input wire ap_clk,\n\
             input wire pkt_in_q0_dout,\n\
             input wire pkt_in_q0_empty_n,\n\
             output wire pkt_in_q0_read,\n\
             input wire pkt_in_q0_peek_dout,\n\
             input wire pkt_in_q0_peek_empty_n\n\
             ); endmodule",
        )
        .unwrap();
        let mut args = BTreeMap::new();
        args.insert(
            "pkt_in_q0".to_owned(),
            ArgDesign {
                arg: "fifo_0".to_owned(),
                cat: ArgCategory::Istream,
                extra: BTreeMap::new(),
            },
        );
        let inst = build_child_instance(
            "switch",
            "switch_0",
            &sig,
            &args,
            &ChildMmapBindings::default(),
            Some(&child_rtl),
        );
        let text = inst.to_string();
        assert!(
            text.contains(".pkt_in_q0_peek_dout(fifo_0_dout)"),
            "peek dout should reuse the base FIFO dout signal:\n{text}"
        );
        assert!(
            text.contains(".pkt_in_q0_peek_empty_n(fifo_0_empty_n)"),
            "peek empty_n should reuse the base FIFO empty signal:\n{text}"
        );
    }

    #[test]
    fn build_child_instance_connects_array_istream_peek_inputs() {
        use std::collections::BTreeMap;
        let sig = InstanceSignals::new("stage_0", false);
        let child_rtl = VerilogModule::parse(
            "module stage(\n\
             input wire ap_clk,\n\
             input wire in_q_0_dout,\n\
             input wire in_q_0_empty_n,\n\
             output wire in_q_0_read,\n\
             input wire in_q_peek_0_dout,\n\
             input wire in_q_peek_0_empty_n\n\
             ); endmodule",
        )
        .unwrap();
        let mut args = BTreeMap::new();
        args.insert(
            "in_q[0]".to_owned(),
            ArgDesign {
                arg: "fifo[0]".to_owned(),
                cat: ArgCategory::Istream,
                extra: BTreeMap::new(),
            },
        );
        let inst = build_child_instance(
            "stage",
            "stage_0",
            &sig,
            &args,
            &ChildMmapBindings::default(),
            Some(&child_rtl),
        );
        let text = inst.to_string();
        assert!(
            text.contains(".in_q_peek_0_dout(fifo_0_dout)"),
            "array peek dout should use compatible name ordering:\n{text}"
        );
        assert!(
            text.contains(".in_q_peek_0_empty_n(fifo_0_empty_n)"),
            "array peek empty_n should use compatible name ordering:\n{text}"
        );
    }

    #[test]
    fn build_child_instance_sanitizes_indexed_mmap_signals() {
        use std::collections::BTreeMap;
        let sig = InstanceSignals::new("worker_0", false);
        let mut args = BTreeMap::new();
        args.insert(
            "mem".to_owned(),
            ArgDesign {
                arg: "chan[0]".to_owned(),
                cat: ArgCategory::Mmap,
                extra: BTreeMap::new(),
            },
        );
        let inst = build_child_instance(
            "worker",
            "worker_0",
            &sig,
            &args,
            &ChildMmapBindings::default(),
            None,
        );
        let text = inst.to_string();
        assert!(
            text.contains(".m_axi_mem_ARADDR(m_axi_chan_0_ARADDR)"),
            "got:\n{text}"
        );
        assert!(!text.contains("m_axi_chan[0]"), "got:\n{text}");
    }

    #[test]
    fn build_child_instance_connects_async_mmap_stream_ports() {
        use std::collections::BTreeMap;
        let sig = InstanceSignals::new("copy_0", false);
        let child_rtl = VerilogModule::parse(
            "module copy(\n\
             input wire ap_clk,\n\
             output wire [63:0] mem_read_addr_s_din,\n\
             input wire mem_read_addr_s_full_n,\n\
             output wire mem_read_addr_s_write,\n\
             input wire [63:0] mem_read_addr_offset,\n\
             input wire [512:0] mem_read_data_s_dout,\n\
             input wire mem_read_data_s_empty_n,\n\
             output wire mem_read_data_s_read,\n\
             input wire [512:0] mem_read_data_peek_dout,\n\
             input wire mem_read_data_peek_empty_n,\n\
             output wire mem_write_addr_s_write,\n\
             input wire [63:0] mem_write_addr_offset,\n\
             output wire [512:0] mem_write_data_s_din,\n\
             input wire mem_write_data_s_full_n,\n\
             input wire [8:0] mem_write_resp_s_dout,\n\
             input wire mem_write_resp_s_empty_n,\n\
             output wire mem_write_resp_s_read\n\
             ); endmodule",
        )
        .unwrap();
        let mut args = BTreeMap::new();
        args.insert(
            "mem".to_owned(),
            ArgDesign {
                arg: "chan[0]".to_owned(),
                cat: ArgCategory::AsyncMmap,
                extra: BTreeMap::new(),
            },
        );
        let inst = build_child_instance(
            "copy",
            "copy_0",
            &sig,
            &args,
            &ChildMmapBindings::default(),
            Some(&child_rtl),
        );
        let text = inst.to_string();
        assert!(
            text.contains(".mem_read_addr_s_din(chan_0_read_addr__din)"),
            "read address stream should connect to async_mmap bridge wires:\n{text}"
        );
        assert!(
            text.contains(".mem_read_data_s_dout({1'b0, chan_0_read_data__dout})"),
            "read data stream should prepend a false EOT bit:\n{text}"
        );
        assert!(
            text.contains(".mem_read_data_peek_dout({1'b0, chan_0_read_data__dout})"),
            "read data peek should mirror the bridge data signal:\n{text}"
        );
        assert!(
            text.contains(".mem_write_resp_s_dout({1'b0, chan_0_write_resp__dout})"),
            "write response stream should prepend a false EOT bit:\n{text}"
        );
        assert!(
            text.contains(".mem_read_addr_offset(copy_0__mem_offset)"),
            "read address offset should use the per-instance offset pipeline:\n{text}"
        );
        assert!(
            text.contains(".mem_write_addr_offset(copy_0__mem_offset)"),
            "write address offset should use the per-instance offset pipeline:\n{text}"
        );
        assert!(
            !text.contains(".m_axi_mem_"),
            "async_mmap children should not be wired as direct AXI children:\n{text}"
        );
    }

    #[test]
    fn build_child_instance_connects_async_mmap_slot_axi_ports() {
        use std::collections::BTreeMap;
        let sig = InstanceSignals::new("SLOT_X0Y2_SLOT_X0Y2_0", false);
        let child_rtl = VerilogModule::parse(
            "module SLOT_X0Y2_SLOT_X0Y2(\n\
             input wire ap_clk,\n\
             input wire [63:0] mem_Copy_0_offset\n\
             ); endmodule",
        )
        .unwrap();
        let mut args = BTreeMap::new();
        args.insert(
            "mem_Copy_0".to_owned(),
            ArgDesign {
                arg: "chan[0]".to_owned(),
                cat: ArgCategory::AsyncMmap,
                extra: BTreeMap::new(),
            },
        );
        let inst = build_child_instance(
            "SLOT_X0Y2_SLOT_X0Y2",
            "SLOT_X0Y2_SLOT_X0Y2_0",
            &sig,
            &args,
            &ChildMmapBindings::default(),
            Some(&child_rtl),
        );
        let text = inst.to_string();
        assert!(
            text.contains(".mem_Copy_0_offset(SLOT_X0Y2_SLOT_X0Y2_0__mem_Copy_0_offset)"),
            "slot async mmap offset should connect to the per-instance offset pipeline:\n{text}"
        );
        assert!(
            text.contains(".m_axi_mem_Copy_0_AWADDR(m_axi_chan_0_AWADDR)"),
            "slot async mmap AXI ports should connect to the parent channel once the slot exposes the direct offset:\n{text}"
        );
        assert!(
            text.contains(".m_axi_mem_Copy_0_AWVALID(m_axi_chan_0_AWVALID)"),
            "slot async mmap binding should emit the full direct AXI bundle:\n{text}"
        );
    }
}
