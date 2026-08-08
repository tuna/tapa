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
use super::instance::{build_child_instance_with_reset, ChildMmapBinding, ChildMmapBindings};
use crate::instance_signals::InstanceSignals;
use crate::passes::async_mmap;
use crate::passes::{distributed_control, TaskPassCtx};
use crate::state::views::{DesignView, FsmTable, ModuleTable};
use crate::{m_axi, program};

/// Staging context for the per-task child-signal stage.
///
/// Bundles the three narrowed state views with the task identity, the
/// driver-staged plans/bindings, and the accumulators (parent FIFO names,
/// `is_done` nets, FSM instantiation portargs) that the split stage
/// functions thread through.
struct ChildStageCtx<'ctx, 'a> {
    /// Read access to the design model.
    design: DesignView<'ctx>,
    /// Mutable access to the attached HLS module table.
    modules: &'a mut ModuleTable<'ctx>,
    /// Mutable access to the per-task FSM module table.
    fsms: &'a mut FsmTable<'ctx>,
    /// The upper-level task currently being instrumented.
    task_name: &'ctx str,
    /// Aggregated + validated mmap connections for the task.
    mmap_conns: &'a BTreeMap<String, crate::rtl_state::MMapConnection>,
    /// `(parent_arg, child_task, inst_idx) -> slave_idx` for crossbar mmaps.
    mmap_slave_map: &'a BTreeMap<(String, String, usize), usize>,
    /// Floorplan-routed direct M-AXI pipeline plan (top task only).
    axi_pipeline_plan: Option<&'a crate::passes::axi_pipeline::DirectAxiPipelinePlan>,
    /// Distributed control plan (top task only).
    control_plan: Option<&'a distributed_control::DistributedControlPlan>,
    /// `is_done` nets accumulated per non-autorun instance, in instance order.
    is_done_signals: Vec<String>,
    /// FSM module instantiation portargs, in per-instance emission order.
    fsm_portargs: Vec<tapa_rtl::builder::PortArg>,
}

impl<'ctx, 'a> ChildStageCtx<'ctx, 'a> {
    /// Bundle the task pass context with the derived per-task staging state.
    fn new(ctx: &'a mut TaskPassCtx<'ctx>) -> Self {
        let design = ctx.design;
        Self {
            design,
            modules: &mut ctx.modules,
            fsms: &mut ctx.fsms,
            task_name: ctx.name,
            mmap_conns: &ctx.inputs.mmap_conns,
            mmap_slave_map: &ctx.inputs.mmap_slave_map,
            axi_pipeline_plan: ctx.inputs.axi_pipeline_plan.as_ref(),
            control_plan: ctx.inputs.control_plan.as_ref(),
            is_done_signals: Vec::new(),
            fsm_portargs: Vec::new(),
        }
    }
}

/// One child task instance staged for wiring.
///
/// Carries the instance identity (task name, logical/sanitized instance
/// names), its handshake signal handles, its argument map, and the cloned
/// child module header shared across sibling instances.
struct ChildInstance<'a> {
    /// The child task name this instance instantiates.
    child_name: &'a str,
    /// The explicit instance name, or the `{child}_{idx}` fallback.
    logical_inst_name: String,
    /// The sanitized instance name used in generated identifiers.
    inst_name: String,
    /// Whether the instance auto-starts (negative step).
    is_autorun: bool,
    /// Handshake signals for this instance.
    sig: InstanceSignals,
    /// The instance's argument bindings (child port -> parent arg).
    args: &'a BTreeMap<String, Arg>,
    /// The cloned child module header, when the child RTL is attached.
    child_rtl: Option<&'a tapa_rtl::VerilogModule>,
}

impl<'a> ChildInstance<'a> {
    /// Stage one child instance: resolve names and handshake signal handles.
    fn new(
        child_name: &'a str,
        idx: usize,
        explicit_name: Option<String>,
        is_autorun: bool,
        args: &'a BTreeMap<String, Arg>,
        child_rtl: Option<&'a tapa_rtl::VerilogModule>,
    ) -> Self {
        let logical_inst_name = explicit_name.unwrap_or_else(|| format!("{child_name}_{idx}"));
        let inst_name = tapa_rtl::module::sanitize_identifier_name(&logical_inst_name);
        let sig = InstanceSignals::new(&inst_name, is_autorun);
        Self {
            child_name,
            logical_inst_name,
            inst_name,
            is_autorun,
            sig,
            args,
            child_rtl,
        }
    }
}

/// Generate child instance signals, FSM/autorun logic, and actual child module instances.
///
/// FSM/start logic goes into the FSM module (not the parent task module).
/// The FSM module is then instantiated into the parent task module.
pub fn generate_child_signals(
    ctx: &mut TaskPassCtx<'_>,
) -> Result<Vec<String>, crate::error::CodegenError> {
    type ChildEntry = (usize, Option<String>, bool, BTreeMap<String, Arg>);

    let mut stage = ChildStageCtx::new(ctx);
    let design = stage.design.design();
    let task = &design.tasks[stage.task_name];
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
        let child_rtl = stage
            .modules
            .get(&child_name)
            .map(|module| module.inner.clone());
        for (idx, explicit_name, is_autorun, args) in entries {
            let inst = ChildInstance::new(
                &child_name,
                idx,
                explicit_name,
                is_autorun,
                &args,
                child_rtl.as_ref(),
            );
            wire_child_instance(&mut stage, &inst, idx)?;
        }
    }

    instantiate_fsm_module(&mut stage);
    Ok(std::mem::take(&mut stage.is_done_signals))
}

/// Wire one child instance: handshake, FSM logic, pipelines, mmap, instance.
fn wire_child_instance(
    stage: &mut ChildStageCtx<'_, '_>,
    inst: &ChildInstance<'_>,
    idx: usize,
) -> Result<(), crate::error::CodegenError> {
    declare_parent_handshake_wires(stage, &inst.sig, inst.is_autorun);
    register_fsm_handshake_ports(stage, &inst.sig, inst.is_autorun);
    add_fsm_control_logic(stage, &inst.sig, inst.is_autorun);
    apply_control_plan_or_pipelines(stage, inst)?;
    push_pipeline_portargs(stage, &inst.inst_name, inst.args);
    let mmap_bindings = build_mmap_bindings(stage, inst, idx);
    let (reset_n, bridge_reset) =
        instance_reset_exprs(stage.control_plan.is_some(), &inst.logical_inst_name);
    add_async_mmap_bridges(stage, inst, &mmap_bindings, &bridge_reset);
    add_child_instance(stage, inst, &mmap_bindings, reset_n);
    Ok(())
}

/// Declare the parent-side handshake wires for one child instance.
fn declare_parent_handshake_wires(
    stage: &mut ChildStageCtx<'_, '_>,
    sig: &InstanceSignals,
    is_autorun: bool,
) {
    // Parent task module: only declare handshake WIRES (not state regs)
    // The parent sees start/done/idle/ready/is_done as wires connected to FSM ports
    if let Some(mm) = stage.modules.get_mut(stage.task_name) {
        if stage.control_plan.is_some() {
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
}

/// Register FSM handshake ports and instantiation portargs for one child.
///
/// Also registers the `is_done` output port (and its portarg) for
/// non-autorun instances. Called for every instance; the port/portarg
/// mutations are skipped when a distributed control plan owns the wiring.
fn register_fsm_handshake_ports(
    stage: &mut ChildStageCtx<'_, '_>,
    sig: &InstanceSignals,
    is_autorun: bool,
) {
    if stage.control_plan.is_none() {
        // FSM module interface: add handshake ports, and build the FSM module
        // instantiation portargs using child-specific port names.
        // FSM module ports are child-specific (e.g., child_0__ap_start),
        // parent wires have the same names — so both sides match.
        // `fsm_ports()` is pure over `sig`, so one merged pass emits the FSM
        // module ports and the instantiation portargs in the same per-target
        // order as two sequential loops; the portarg push stays outside the
        // module lookup exactly as before.
        for port in sig.fsm_ports() {
            let portarg = tapa_rtl::builder::PortArg::new(&port.name, Expr::ident(&port.name));
            if let Some(fsm_mm) = stage.fsms.get_mut(stage.task_name) {
                let _ = fsm_mm.add_port(port);
            }
            stage.fsm_portargs.push(portarg);
        }
    }

    if !is_autorun {
        stage.is_done_signals.push(sig.is_done_name());

        if stage.control_plan.is_none() {
            // Add is_done port to FSM module interface
            if let Some(fsm_mm) = stage.fsms.get_mut(stage.task_name) {
                let _ = fsm_mm.add_port(tapa_rtl::mutation::simple_port(
                    sig.is_done_name(),
                    tapa_rtl::port::Direction::Output,
                ));
            }
            // Add is_done portarg for FSM instantiation
            stage.fsm_portargs.push(tapa_rtl::builder::PortArg::new(
                sig.is_done_name(),
                Expr::ident(sig.is_done_name()),
            ));
        }
    }
}

/// Add the FSM/autorun start logic, owned by the per-task FSM module.
fn add_fsm_control_logic(
    stage: &mut ChildStageCtx<'_, '_>,
    sig: &InstanceSignals,
    is_autorun: bool,
) {
    // FSM/autorun logic goes into the FSM MODULE
    if stage.control_plan.is_some() {
        return;
    }
    if let Some(fsm_mm) = stage.fsms.get_mut(stage.task_name) {
        for signal in sig.all_signals() {
            let _ = fsm_mm.add_signal(signal);
        }
        if is_autorun {
            fsm_mm.ensure_signal_kind(&sig.start_name(), tapa_rtl::signal::SignalKind::Reg);
            fsm_mm.add_always(generate_autorun_start(sig));
        } else {
            // State register and FSM logic owned by FSM module
            // Use pipelined start_q/done_q from global FSM
            let start_input = Expr::ident(program::START_Q);
            let done_release = Expr::ident(program::DONE_Q);
            fsm_mm.add_always(generate_child_fsm(sig, start_input, done_release));
            // ap_start output: combinationally driven from state
            fsm_mm.add_assign(generate_start_assign(sig));
            // is_done: driven from state inside FSM module
            fsm_mm.add_assign(generate_is_done_assign(sig));
        }
    }
}

/// Route child control through the distributed plan or the FSM pipeline.
fn apply_control_plan_or_pipelines(
    stage: &mut ChildStageCtx<'_, '_>,
    inst: &ChildInstance<'_>,
) -> Result<(), crate::error::CodegenError> {
    if let Some(plan) = stage.control_plan {
        plan.instantiate_child(
            stage.modules,
            stage.task_name,
            &inst.logical_inst_name,
            &inst.sig,
        )?;
    } else {
        // Declare per-instance pipeline signals for scalar/mmap args
        declare_instance_pipeline_signals(stage, inst.child_name, &inst.inst_name, inst.args);
    }
    Ok(())
}

/// Return the pipeline and FSM-input wire names for a pipelined argument.
fn pipeline_wire_names(inst_name: &str, port_name: &str, is_mmap: bool) -> (String, String) {
    if is_mmap {
        (
            format!("{inst_name}__{port_name}_offset"),
            format!("{inst_name}__{port_name}_offset_in"),
        )
    } else {
        (
            format!("{inst_name}__{port_name}"),
            format!("{inst_name}__{port_name}_in"),
        )
    }
}

/// Declare per-instance pipeline signals for scalar and mmap arguments.
///
/// Creates FSM-owned pipeline ports and parent-side wires:
/// - FSM module gets an input port for the parent arg and an output port for the pipeline signal
/// - Parent module gets a wire for the pipeline output
/// - The FSM module uses a registered pipeline (always @(posedge clk)) to
///   delay the signal by one cycle, matching `add_pipeline` behavior
fn declare_instance_pipeline_signals(
    stage: &mut ChildStageCtx<'_, '_>,
    child_name: &str,
    inst_name: &str,
    args: &BTreeMap<String, Arg>,
) {
    for (port_name, arg) in args {
        let width = if arg.cat.is_scalar() {
            resolve_child_scalar_width(stage, child_name, port_name)
        } else if arg.cat.is_direct_mmap() {
            Some(("63".to_string(), "0".to_string())) // 64-bit
        } else {
            continue;
        };
        let (pipeline_out, fsm_in) =
            pipeline_wire_names(inst_name, port_name, arg.cat.is_direct_mmap());
        add_pipeline_stage(stage, &pipeline_out, &fsm_in, width.as_ref());
    }
}

/// Add a registered pipeline stage: parent wire + FSM input/output ports + internal reg.
fn add_pipeline_stage(
    stage: &mut ChildStageCtx<'_, '_>,
    pipeline_out: &str,
    fsm_in_port: &str,
    width: Option<&(String, String)>, // None = 1-bit, Some((msb, lsb))
) {
    // Parent: wire for pipeline output
    if let Some(mm) = stage.modules.get_mut(stage.task_name) {
        let sig = match width {
            Some((msb, lsb)) => tapa_rtl::mutation::wide_wire(pipeline_out, msb, lsb),
            None => tapa_rtl::mutation::wire(pipeline_out),
        };
        let _ = mm.add_signal(sig);
    }

    // FSM module: input port + output port + internal _reg + registered always block
    if let Some(fsm_mm) = stage.fsms.get_mut(stage.task_name) {
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

/// Queue the scalar/mmap pipeline portargs for the FSM instantiation.
fn push_pipeline_portargs(
    stage: &mut ChildStageCtx<'_, '_>,
    inst_name: &str,
    args: &BTreeMap<String, Arg>,
) {
    if stage.control_plan.is_some() {
        return;
    }
    // Add pipeline portargs to FSM instantiation
    for (port_name, arg) in args {
        if arg.cat.is_scalar() {
            let (pipeline_out, fsm_in_port) = pipeline_wire_names(inst_name, port_name, false);
            // A scalar is the one binding that can be a constant rather than
            // a parent wire; this is where the constant becomes Verilog.
            let driver = match &arg.arg {
                tapa_ir::ArgSource::Name(name) => {
                    Expr::ident(tapa_rtl::module::sanitize_array_name(name))
                }
                tapa_ir::ArgSource::Literal(value) => Expr::lit(value.to_string()),
            };
            stage
                .fsm_portargs
                .push(tapa_rtl::builder::PortArg::new(&fsm_in_port, driver));
            stage.fsm_portargs.push(tapa_rtl::builder::PortArg::new(
                &pipeline_out,
                Expr::ident(&pipeline_out),
            ));
        } else if arg.cat.is_direct_mmap() {
            // An mmap always names a parent wire, never a constant.
            let Some(parent) = arg.name() else { continue };
            let (pipeline_out, fsm_in_port) = pipeline_wire_names(inst_name, port_name, true);
            let arg_name = tapa_rtl::module::sanitize_array_name(parent);
            let offset_source = stage.mmap_conns.get(parent).map_or_else(
                || Expr::ident(format!("{arg_name}_offset")),
                |conn| {
                    if conn.chan_count.is_some() {
                        Expr::lit("64'd0")
                    } else {
                        Expr::ident(format!("{arg_name}_offset"))
                    }
                },
            );
            stage
                .fsm_portargs
                .push(tapa_rtl::builder::PortArg::new(&fsm_in_port, offset_source));
            stage.fsm_portargs.push(tapa_rtl::builder::PortArg::new(
                &pipeline_out,
                Expr::ident(&pipeline_out),
            ));
        }
        // Streams and Immap/Ommap contribute no FSM portargs.
    }
}

/// Build the per-instance mmap binding map used for crossbar routing.
fn build_mmap_bindings(
    stage: &ChildStageCtx<'_, '_>,
    inst: &ChildInstance<'_>,
    idx: usize,
) -> ChildMmapBindings {
    // One per-instance binding per mmap argument for crossbar routing.
    let mut mmap_bindings = ChildMmapBindings::default();
    for (child_port, arg) in inst.args {
        if arg.cat.is_direct_mmap() {
            // An mmap always names a parent wire, never a constant.
            let Some(parent) = arg.name() else { continue };
            let mut binding = ChildMmapBinding::default();
            if let Some(prefix) = stage.axi_pipeline_plan.and_then(|plan| {
                plan.child_wire_prefix(&tapa_ir::AxiEndpoint {
                    instance: inst.logical_inst_name.clone(),
                    port: child_port.clone(),
                    top_port: parent.to_owned(),
                })
            }) {
                binding.direct_wire_prefix = Some(prefix);
            }
            if let Some(&slave_idx) =
                stage
                    .mmap_slave_map
                    .get(&(parent.to_owned(), inst.child_name.to_owned(), idx))
            {
                binding.slave_index = Some(slave_idx);
                if let Some(conn) = stage.mmap_conns.get(parent) {
                    binding.wire_id_width = Some(m_axi::crossbar_slave_id_width(conn));
                    if let Some(slave) = conn.slaves.get(slave_idx) {
                        binding.child_id_width = Some(slave.id_width);
                    }
                }
            }
            mmap_bindings.insert(parent.to_owned(), binding);
        }
    }
    mmap_bindings
}

/// Compute the child-local reset and the async-bridge reset for one instance.
///
/// Takes the presence of a distributed control plan rather than the plan
/// itself: presence is all the choice depends on, and a plan can only be
/// built from a floorplan, which would put a whole pipeline behind a
/// two-branch decision.
fn instance_reset_exprs(distributed_control: bool, logical_inst_name: &str) -> (Expr, Expr) {
    let reset_n = if distributed_control {
        Expr::ident(
            distributed_control::DistributedControlPlan::child_reset_name(logical_inst_name),
        )
    } else {
        Expr::ident(HANDSHAKE_RST_N)
    };
    // A floorplanned async bridge is co-located with its child, so the
    // routed child reset is also its physically local reset;
    // non-floorplanned builds use the parent reset.
    let bridge_reset = if distributed_control {
        Expr::logical_not(reset_n.clone())
    } else {
        Expr::ident(HANDSHAKE_RST)
    };
    (reset_n, bridge_reset)
}

/// Build and register the floorplanned async-mmap bridge instances.
fn add_async_mmap_bridges(
    stage: &mut ChildStageCtx<'_, '_>,
    inst: &ChildInstance<'_>,
    mmap_bindings: &ChildMmapBindings,
    bridge_reset: &Expr,
) {
    let Some(child_rtl) = inst.child_rtl else {
        return;
    };
    for (child_port, arg) in inst.args {
        if !matches!(arg.cat, tapa_ir::port::ArgCategory::AsyncMmap) {
            continue;
        }
        let Some(parent) = arg.name() else { continue };
        let active_tags = async_mmap::active_tags(child_rtl, child_port);
        if active_tags.is_empty() {
            continue;
        }
        let enabled = async_mmap::enabled_axi_directions(child_rtl, child_port, &active_tags);
        let m_axi_wire_prefix = mmap_bindings.wire_prefix(parent);
        let upstream_m_axi_prefix = mmap_bindings.upstream_wire_prefix(parent);
        let bridge_base = async_mmap::bridge_base_from_m_axi_prefix(&m_axi_wire_prefix);
        // Aggregation already derived the width with the same
        // parent-then-child port precedence.
        let data_width = stage.mmap_conns.get(parent).map_or(64, |c| c.data_width);
        let connect_optional_axi_ports = mmap_bindings.slave_index(parent).is_none();
        if let Some(mm) = stage.modules.get_mut(stage.task_name) {
            async_mmap::add_bridge_signals(mm, &bridge_base, &active_tags, data_width);
            mm.add_instance(async_mmap::build_bridge_instance(
                &bridge_base,
                &tapa_ir::async_mmap_bridge_instance_name(parent),
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

/// Build the child module instance and add it to the parent task module.
fn add_child_instance(
    stage: &mut ChildStageCtx<'_, '_>,
    inst: &ChildInstance<'_>,
    mmap_bindings: &ChildMmapBindings,
    reset_n: Expr,
) {
    let parent_fifos: BTreeSet<String> = stage.design.design().tasks[stage.task_name]
        .fifos
        .keys()
        .cloned()
        .collect();

    // Build and add the actual child module instance to parent
    let child_inst = build_child_instance_with_reset(
        inst.child_name,
        &inst.inst_name,
        &inst.sig,
        inst.args,
        mmap_bindings,
        &parent_fifos,
        stage.modules.get(stage.task_name).map(|mm| &mm.inner),
        inst.child_rtl,
        reset_n,
    );
    if let Some(mm) = stage.modules.get_mut(stage.task_name) {
        mm.add_instance(child_inst);
    }
    if inst.child_rtl.is_some() {
        crate::m_axi::add_crossbar_slave_id_padding(
            stage.modules,
            stage.task_name,
            inst.args,
            mmap_bindings,
        );
    }
}

/// Instantiate the per-task FSM module into the parent task module.
fn instantiate_fsm_module(stage: &mut ChildStageCtx<'_, '_>) {
    if stage.control_plan.is_some() {
        return;
    }
    // Instantiate FSM module into parent task module
    let task_name = stage.task_name;
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
    fsm_inst_ports.append(&mut stage.fsm_portargs);
    let fsm_inst = tapa_rtl::builder::ModuleInstance::new(&fsm_module_name, "__tapa_fsm_unit")
        .with_ports(fsm_inst_ports);
    if let Some(mm) = stage.modules.get_mut(task_name) {
        mm.add_instance(fsm_inst);
    }
}

/// Resolve the scalar port width from the child module header or the design.
fn resolve_child_scalar_width(
    stage: &ChildStageCtx<'_, '_>,
    child_name: &str,
    port_name: &str,
) -> Option<(String, String)> {
    if let Some(module) = stage.modules.get(child_name) {
        if let Some(port) = module.inner.find_port(port_name) {
            return port.width.as_ref().map(|width| {
                (
                    tapa_rtl::expression::expression_source(&width.msb),
                    tapa_rtl::expression::expression_source(&width.lsb),
                )
            });
        }
    }

    stage
        .design
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pipeline_wire_names_tag_mmap_arguments_as_offsets() {
        // An mmap argument pipelines its *offset*, not the argument, so the
        // two families must not collide in the parent's wire namespace.
        assert_eq!(
            pipeline_wire_names("worker_0", "data", true),
            (
                "worker_0__data_offset".to_owned(),
                "worker_0__data_offset_in".to_owned()
            )
        );
        assert_eq!(
            pipeline_wire_names("worker_0", "data", false),
            ("worker_0__data".to_owned(), "worker_0__data_in".to_owned())
        );
    }

    #[test]
    fn without_distributed_control_an_instance_uses_the_parent_resets() {
        let (reset_n, bridge_reset) = instance_reset_exprs(false, "worker_0");
        assert_eq!(reset_n.to_string(), HANDSHAKE_RST_N);
        // Active-high parent reset, not the negation of reset_n: the bridge
        // has no routed reset of its own in a non-floorplanned build.
        assert_eq!(bridge_reset.to_string(), HANDSHAKE_RST);
    }

    #[test]
    fn with_distributed_control_an_instance_uses_its_routed_reset() {
        let (reset_n, bridge_reset) = instance_reset_exprs(true, "worker_0");
        assert_eq!(
            reset_n.to_string(),
            distributed_control::DistributedControlPlan::child_reset_name("worker_0")
        );
        // The bridge is co-located with its child, so it takes the child's
        // routed reset inverted -- never the parent's. Logical `!`, not
        // bitwise `~`: the reset is one bit and the bridge wants active-high.
        assert_eq!(bridge_reset.to_string(), format!("!{reset_n}"));
        assert_ne!(bridge_reset.to_string(), HANDSHAKE_RST);
    }

    #[test]
    fn the_routed_reset_is_derived_from_the_instance_name() {
        // Two instances must not share a reset net.
        let (a, _) = instance_reset_exprs(true, "worker_0");
        let (b, _) = instance_reset_exprs(true, "worker_1");
        assert_ne!(a.to_string(), b.to_string());
    }
}
