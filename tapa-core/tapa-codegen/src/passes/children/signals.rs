//! Child instance signal and wiring generation: the per-task
//! `generate_child_signals` stage and its pipeline helpers.

use std::collections::{BTreeMap, BTreeSet};

use tapa_ir::Arg;
use tapa_protocol::{
    HANDSHAKE_CLK, HANDSHAKE_DONE, HANDSHAKE_IDLE, HANDSHAKE_READY, HANDSHAKE_RST, HANDSHAKE_RST_N,
    HANDSHAKE_START,
};
use tapa_rtl::builder::{ContinuousAssign, Expr};

use super::fsm::{
    generate_autorun_start, generate_child_fsm, generate_is_done_assign, generate_start_assign,
};
use super::instance::{build_child_instance_with_reset, ChildMmapBindings};
use crate::passes::distributed_control;
use crate::state::views::{DesignView, FsmTable, ModuleTable};
use crate::{async_mmap, instance_signals, m_axi, program};

/// Generate child instance signals, FSM/autorun logic, and actual child module instances.
///
/// FSM/start logic goes into the FSM module (not the parent task module).
/// The FSM module is then instantiated into the parent task module.
#[allow(clippy::too_many_lines, reason = "sequential child signal generation")]
#[allow(
    clippy::too_many_arguments,
    reason = "the three disjoint state views stay named per concern; the rest               are driver-staged inputs"
)]
pub fn generate_child_signals(
    design: DesignView<'_>,
    modules: &mut ModuleTable<'_>,
    fsms: &mut FsmTable<'_>,
    task_name: &str,
    mmap_conns: &std::collections::BTreeMap<String, crate::rtl_state::MMapConnection>,
    mmap_slave_map: &std::collections::BTreeMap<(String, String, usize), usize>,
    axi_pipeline_plan: Option<&crate::passes::axi_pipeline::DirectAxiPipelinePlan>,
    control_plan: Option<&distributed_control::DistributedControlPlan>,
) -> Result<Vec<String>, crate::error::CodegenError> {
    type ChildEntry = (usize, Option<String>, bool, BTreeMap<String, Arg>);

    let task = &design.design().tasks[task_name];
    let parent_fifos: BTreeSet<String> = task.fifos.keys().cloned().collect();
    let mut is_done_signals = Vec::new();
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
        let child_rtl = modules.get(&child_name).map(|module| module.inner.clone());
        for (idx, explicit_name, is_autorun, args) in entries {
            let logical_inst_name = explicit_name.unwrap_or_else(|| format!("{child_name}_{idx}"));
            let inst_name = tapa_rtl::module::sanitize_identifier_name(&logical_inst_name);
            let sig = instance_signals::InstanceSignals::new(&inst_name, is_autorun);

            // Parent task module: only declare handshake WIRES (not state regs)
            // The parent sees start/done/idle/ready/is_done as wires connected to FSM ports
            if let Some(mm) = modules.get_mut(task_name) {
                if control_plan.is_some() {
                    let _ = mm.add_signal(tapa_rtl::mutation::wire(sig.start_name()));
                    if !is_autorun {
                        let _ = mm.add_signal(tapa_rtl::mutation::wire(sig.done_name()));
                        let _ = mm.add_signal(tapa_rtl::mutation::wire(sig.idle_name()));
                        let _ = mm.add_signal(tapa_rtl::mutation::wire(sig.ready_name()));
                        let _ = mm.add_signal(tapa_rtl::mutation::wire(sig.is_done_name()));
                    }
                } else if is_autorun {
                    let _ = mm.add_signal(tapa_rtl::mutation::wire(sig.start_name()));
                } else {
                    for signal in sig.all_signals() {
                        let _ = mm.add_signal(signal);
                    }
                }
            }

            // FSM module interface: add handshake ports
            if control_plan.is_none() {
                if let Some(fsm_mm) = fsms.get_mut(task_name) {
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
            }

            if !is_autorun {
                is_done_signals.push(sig.is_done_name());

                // Add is_done port to FSM module interface
                if control_plan.is_none() {
                    if let Some(fsm_mm) = fsms.get_mut(task_name) {
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
            }

            // FSM/autorun logic goes into the FSM MODULE
            if control_plan.is_none() {
                if let Some(fsm_mm) = fsms.get_mut(task_name) {
                    for signal in sig.all_signals() {
                        let _ = fsm_mm.add_signal(signal);
                    }
                    if is_autorun {
                        fsm_mm.ensure_signal_kind(
                            &sig.start_name(),
                            tapa_rtl::signal::SignalKind::Reg,
                        );
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
            }

            if let Some(plan) = control_plan {
                plan.instantiate_child(modules, task_name, &logical_inst_name, &sig)?;
            } else {
                // Declare per-instance pipeline signals for scalar/mmap args
                declare_instance_pipeline_signals(
                    design,
                    modules,
                    fsms,
                    task_name,
                    &child_name,
                    &inst_name,
                    &args,
                );
            }

            if control_plan.is_none() {
                // Add pipeline portargs to FSM instantiation
                for (port_name, arg) in &args {
                    if arg.cat.is_scalar() {
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
                    } else if arg.cat.is_direct_mmap() {
                        let pipeline_out = format!("{inst_name}__{port_name}_offset");
                        let fsm_in_port = format!("{inst_name}__{port_name}_offset_in");
                        let arg_name = tapa_rtl::module::sanitize_array_name(&arg.arg);
                        let offset_source = mmap_conns.get(&arg.arg).map_or_else(
                            || Expr::ident(format!("{arg_name}_offset")),
                            |conn| {
                                if conn.chan_count.is_some() {
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
                    // Streams and Immap/Ommap contribute no FSM portargs.
                }
            }

            // Build per-instance mmap slave index map for crossbar routing
            let mut mmap_bindings = ChildMmapBindings::default();
            for (child_port, arg) in &args {
                if arg.cat.is_direct_mmap() {
                    if let Some(prefix) = axi_pipeline_plan.and_then(|plan| {
                        plan.child_wire_prefix(&tapa_ir::AxiEndpoint {
                            instance: logical_inst_name.clone(),
                            port: child_port.clone(),
                            top_port: arg.arg.clone(),
                        })
                    }) {
                        mmap_bindings
                            .direct_wire_prefixes
                            .insert(arg.arg.clone(), prefix);
                    }
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

            let reset_n = control_plan.map_or_else(
                || Expr::ident(HANDSHAKE_RST_N),
                |_| {
                    Expr::ident(
                        distributed_control::DistributedControlPlan::child_reset_name(
                            &logical_inst_name,
                        ),
                    )
                },
            );
            // A floorplanned async bridge is co-located with its child, so the
            // routed child reset is also its physically local reset;
            // non-floorplanned builds use the parent reset.
            let bridge_reset = if control_plan.is_some() {
                Expr::logical_not(reset_n.clone())
            } else {
                Expr::ident(HANDSHAKE_RST)
            };

            // Build and add the actual child module instance to parent
            if let Some(child_rtl) = child_rtl.as_ref() {
                for (child_port, arg) in &args {
                    if !matches!(arg.cat, tapa_ir::port::ArgCategory::AsyncMmap) {
                        continue;
                    }
                    let active_tags = async_mmap::active_tags(child_rtl, child_port);
                    if active_tags.is_empty() {
                        continue;
                    }
                    let enabled =
                        async_mmap::enabled_axi_directions(child_rtl, child_port, &active_tags);
                    let m_axi_wire_prefix = mmap_bindings.wire_prefix(&arg.arg);
                    let upstream_m_axi_prefix = mmap_bindings.upstream_wire_prefix(&arg.arg);
                    let bridge_base = async_mmap::bridge_base_from_m_axi_prefix(&m_axi_wire_prefix);
                    // Aggregation already derived the width with the same
                    // parent-then-child port precedence.
                    let data_width = mmap_conns.get(&arg.arg).map_or(64, |c| c.data_width);
                    let connect_optional_axi_ports =
                        !mmap_bindings.slave_indices.contains_key(&arg.arg);
                    if let Some(mm) = modules.get_mut(task_name) {
                        async_mmap::add_bridge_signals(mm, &bridge_base, &active_tags, data_width);
                        mm.add_instance(async_mmap::build_bridge_instance(
                            &bridge_base,
                            &tapa_ir::async_mmap_bridge_instance_name(&arg.arg),
                            &m_axi_wire_prefix,
                            &upstream_m_axi_prefix,
                            &active_tags,
                            enabled,
                            data_width,
                            connect_optional_axi_ports,
                            bridge_reset.clone(),
                        ));
                    }
                }
            }
            let child_inst = build_child_instance_with_reset(
                &child_name,
                &inst_name,
                &sig,
                &args,
                &mmap_bindings,
                &parent_fifos,
                modules.get(task_name).map(|mm| &mm.inner),
                child_rtl.as_ref(),
                reset_n,
            );
            if let Some(mm) = modules.get_mut(task_name) {
                mm.add_instance(child_inst);
            }
            if child_rtl.is_some() {
                crate::m_axi::add_crossbar_slave_id_padding(
                    modules,
                    task_name,
                    &args,
                    &mmap_bindings,
                );
            }
        }
    }

    if control_plan.is_none() {
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
        if let Some(mm) = modules.get_mut(task_name) {
            mm.add_instance(fsm_inst);
        }
    }

    Ok(is_done_signals)
}

/// Declare per-instance pipeline signals for scalar and mmap arguments.
///
/// Creates FSM-owned pipeline ports and parent-side wires:
/// - FSM module gets an input port for the parent arg and an output port for the pipeline signal
/// - Parent module gets a wire for the pipeline output
/// - The FSM module uses a registered pipeline (always @(posedge clk)) to
///   delay the signal by one cycle, matching `add_pipeline` behavior
fn declare_instance_pipeline_signals(
    design: DesignView<'_>,
    modules: &mut ModuleTable<'_>,
    fsms: &mut FsmTable<'_>,
    task_name: &str,
    child_name: &str,
    inst_name: &str,
    args: &std::collections::BTreeMap<String, tapa_ir::Arg>,
) {
    for (port_name, arg) in args {
        let (pipeline_out, fsm_in, width) = if arg.cat.is_scalar() {
            (
                format!("{inst_name}__{port_name}"),
                format!("{inst_name}__{port_name}_in"),
                resolve_child_scalar_width(design, modules, child_name, port_name),
            )
        } else if arg.cat.is_direct_mmap() {
            (
                format!("{inst_name}__{port_name}_offset"),
                format!("{inst_name}__{port_name}_offset_in"),
                Some(("63".to_string(), "0".to_string())), // 64-bit
            )
        } else {
            continue;
        };
        add_pipeline_stage(
            modules,
            fsms,
            task_name,
            &pipeline_out,
            &fsm_in,
            width.as_ref(),
        );
    }
}

/// Add a registered pipeline stage: parent wire + FSM input/output ports + internal reg.
fn add_pipeline_stage(
    modules: &mut ModuleTable<'_>,
    fsms: &mut FsmTable<'_>,
    task_name: &str,
    pipeline_out: &str,
    fsm_in_port: &str,
    width: Option<&(String, String)>, // None = 1-bit, Some((msb, lsb))
) {
    // Parent: wire for pipeline output
    if let Some(mm) = modules.get_mut(task_name) {
        let sig = match width {
            Some((msb, lsb)) => tapa_rtl::mutation::wide_wire(pipeline_out, msb, lsb),
            None => tapa_rtl::mutation::wire(pipeline_out),
        };
        let _ = mm.add_signal(sig);
    }

    // FSM module: input port + output port + internal _reg + registered always block
    if let Some(fsm_mm) = fsms.get_mut(task_name) {
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
    design: DesignView<'_>,
    modules: &ModuleTable<'_>,
    child_name: &str,
    port_name: &str,
) -> Option<(String, String)> {
    if let Some(module) = modules.get(child_name) {
        if let Some(port) = module.inner.find_port(port_name) {
            return port.width.as_ref().map(|width| {
                (
                    tapa_rtl::expression::expression_source(&width.msb),
                    tapa_rtl::expression::expression_source(&width.lsb),
                )
            });
        }
    }

    design
        .design()
        .tasks
        .get(child_name)?
        .ports
        .iter()
        .find(|port| port.name == port_name && port.cat.is_scalar())
        .and_then(|port| {
            if port.width > 1 {
                Some(((port.width - 1).to_string(), "0".to_string()))
            } else {
                None
            }
        })
}
