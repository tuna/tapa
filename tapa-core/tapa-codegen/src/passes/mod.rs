//! The typed pass pipeline.
//!
//! Every pass body takes per-concern [`crate::state::views`] borrow views
//! instead of the whole [`TopologyWithRtl`]; the functions below are the
//! passes themselves, run in the order `lib.rs` lists them. The driver-side
//! [`DesignPassCtx`] / [`TaskPassCtx`] carry the views built from disjoint
//! field borrows of the state. Only the context constructors, the lib.rs
//! driver, and [`TaskStageInputs::prepare`] (read-only staging) may name the
//! whole state object.

pub mod async_mmap;
mod axi_pipeline;
mod children;
mod cleanup;
mod distributed_control;
mod fifos;
mod floorplans;
mod frames;
pub mod m_axi;
mod rings;
mod s_axi;

use std::collections::BTreeMap;

use tapa_ir::floorplan::Coor;
use tapa_ir::SynthTarget;
use tapa_rtl::builder::Expr;
use tapa_rtl::module::sanitize_array_name;

use self::axi_pipeline::DirectAxiPipelinePlan;
use self::distributed_control::DistributedControlPlan;
use crate::error::CodegenError;
use crate::rtl_state::{MMapConnection, TopologyWithRtl};
use crate::state::views::{DesignView, FsmTable, ModuleTable, OutputSet};

/// Context handed to each design-scope pass: the narrowed per-concern views
/// built from the state's disjoint fields. Pass bodies never see the whole
/// `TopologyWithRtl`.
pub struct DesignPassCtx<'a> {
    /// Read access to the design model.
    pub design: DesignView<'a>,
    /// Mutable access to the attached HLS module table.
    pub modules: ModuleTable<'a>,
    /// Mutable access to the per-task FSM module table.
    pub fsms: FsmTable<'a>,
    /// Mutable access to the emitted-file outputs.
    pub outputs: OutputSet<'a>,
}

/// Split `state` into the disjoint per-concern views.
///
/// Along with the lib.rs driver and [`TaskStageInputs::prepare`]
/// (read-only staging), this is the only code outside `state/` allowed
/// to name the whole state object.
pub fn split_views(
    state: &mut TopologyWithRtl,
) -> (DesignView<'_>, ModuleTable<'_>, FsmTable<'_>, OutputSet<'_>) {
    (
        DesignView::new(
            state
                .flat_projection
                .as_ref()
                .map_or(&state.design, |projection| &projection.flat),
            state.floorplan.as_ref(),
        ),
        ModuleTable::new(&mut state.module_map),
        FsmTable::new(&mut state.fsm_modules),
        OutputSet::new(&mut state.generated_files, &mut state.template_files),
    )
}

impl<'a> DesignPassCtx<'a> {
    /// Build the design-scope context from the shared view split.
    pub(crate) fn new(state: &'a mut TopologyWithRtl) -> Self {
        let (design, modules, fsms, outputs) = split_views(state);
        Self {
            design,
            modules,
            fsms,
            outputs,
        }
    }
}

/// Per-upper-task pass context: the task identity, its driver-precomputed
/// staged inputs, and the narrowed per-concern views built from the state's
/// disjoint fields.
pub struct TaskPassCtx<'a> {
    /// The upper-level task currently being instrumented.
    pub name: &'a str,
    /// Driver-precomputed plans and cross-pass staging for `name`.
    pub inputs: &'a mut TaskStageInputs,
    /// Read access to the design model.
    pub design: DesignView<'a>,
    /// Mutable access to the attached HLS module table.
    pub modules: ModuleTable<'a>,
    /// Mutable access to the per-task FSM module table.
    pub fsms: FsmTable<'a>,
    /// Mutable access to the emitted-file outputs.
    pub outputs: OutputSet<'a>,
}

impl<'a> TaskPassCtx<'a> {
    /// Build the task-scope context: the shared view split plus the task
    /// identity and its staged inputs.
    pub(crate) fn new(
        state: &'a mut TopologyWithRtl,
        name: &'a str,
        inputs: &'a mut TaskStageInputs,
    ) -> Self {
        let (design, modules, fsms, outputs) = split_views(state);
        Self {
            name,
            inputs,
            design,
            modules,
            fsms,
            outputs,
        }
    }
}

/// Precomputed per-task inputs the pass group consumes.
///
/// Everything here is a pure function of the design/floorplan state as it
/// stands when the task's group starts, so computing it up front preserves
/// the validate-before-mutation ordering.
pub struct TaskStageInputs {
    /// Whether this task is the design's top task; staged because two
    /// passes special-case the top level.
    is_top_task: bool,
    /// Aggregated + validated mmap connections for the task.
    mmap_conns: BTreeMap<String, MMapConnection>,
    /// `(parent_arg, child_task, inst_idx) -> slave_idx` for
    /// crossbar-connected mmaps.
    mmap_slave_map: BTreeMap<(String, String, usize), usize>,
    /// Floorplan-routed direct M-AXI pipeline plan (top task only).
    axi_pipeline_plan: Option<DirectAxiPipelinePlan>,
    /// Distributed control plan (top task only).
    control_plan: Option<DistributedControlPlan>,
    /// `is_done` nets produced by `generate-child-signals`, consumed by
    /// `control-fsm`.
    is_done_signals: Vec<String>,
    /// Value of `TopologyWithRtl::top_instantiates_control_s_axi`, staged
    /// here because the predicate reads across design + module table. The
    /// value is stable: no pass adds or removes `s_axi_control_*` ports.
    top_instantiates_control_s_axi: bool,
}

impl TaskStageInputs {
    /// Compute every per-task plan up front so ambiguous child identities and
    /// malformed memory geometry are rejected before RTL state is mutated.
    pub fn prepare(state: &TopologyWithRtl, task_name: &str) -> Result<Self, CodegenError> {
        let is_top_task = task_name == state.design.top;

        let mut instance_names = BTreeMap::new();
        for (child_name, instances) in &state.data_design().tasks[task_name].tasks {
            for (index, instance) in instances.iter().enumerate() {
                let logical_name = instance.canonical_name(child_name, index);
                let rtl_name = tapa_rtl::module::sanitize_identifier_name(&logical_name);
                if let Some(previous) =
                    instance_names.insert(rtl_name.clone(), logical_name.clone())
                {
                    return Err(CodegenError::InvalidChildInstanceNames(format!(
                        "task `{task_name}`: `{previous}` and `{logical_name}` both \
                         produce RTL identifier `{rtl_name}`"
                    )));
                }
            }
        }

        // Reject malformed memory geometry before mutating any RTL state.
        let mmap_conns = state.aggregate_mmap_connections(task_name)?;
        for conn in mmap_conns.values() {
            crate::m_axi::validate_mmap_connection(conn)?;
        }
        let axi_pipeline_plan = if is_top_task {
            state
                .floorplan
                .as_ref()
                .map(|floorplan| {
                    DirectAxiPipelinePlan::from_floorplan(
                        state.direct_mmap_interfaces(task_name)?,
                        floorplan,
                    )
                })
                .transpose()?
        } else {
            None
        };
        let control_plan = if is_top_task {
            DistributedControlPlan::from_floorplan(state, task_name, &mmap_conns)?
        } else {
            None
        };

        // Pre-compute M-AXI slave indices for crossbar-connected mmaps
        // This maps (parent_arg, child_task, inst_idx) -> slave_idx
        let mut mmap_slave_map: BTreeMap<(String, String, usize), usize> = BTreeMap::new();
        for conn in mmap_conns.values() {
            if crate::m_axi::needs_crossbar(conn) {
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

        Ok(Self {
            is_top_task,
            mmap_conns,
            mmap_slave_map,
            axi_pipeline_plan,
            control_plan,
            is_done_signals: Vec::new(),
            top_instantiates_control_s_axi: state.top_instantiates_control_s_axi(),
        })
    }
}

/// Design pass: build the authoritative port-only shells for
/// `Ignore`-synthesized tasks into `module_map`.
pub fn ignore_task_shells(ctx: &mut DesignPassCtx<'_>) -> Result<(), CodegenError> {
    // Ignored tasks have no HLS result to attach. Build their
    // authoritative port-only shell from topology so parents can resolve
    // the module while the user authors the replacement RTL.
    let design = ctx.design.design();
    for (task_name, task) in &design.tasks {
        if task.synth != SynthTarget::Ignore {
            continue;
        }
        let source = crate::template::render_task_template(task_name, task);
        let module = tapa_rtl::VerilogModule::parse(&source)?;
        ctx.modules.insert(
            task_name.clone(),
            tapa_rtl::mutation::MutableModule::from_parsed(module),
        );
    }
    Ok(())
}

/// Task pass: strip HLS artifacts from the task's module and normalize its
/// reset/handshake wiring before any child or FIFO wiring is added.
#[allow(
    clippy::unnecessary_wraps,
    reason = "every pass shares one signature so the stage table stays uniform"
)]
pub fn cleanup_hls_artifacts(ctx: &mut TaskPassCtx<'_>) -> Result<(), CodegenError> {
    let is_top_task = ctx.inputs.is_top_task;
    let control_plan = ctx.inputs.control_plan.as_ref();
    cleanup::cleanup_hls_artifacts(
        ctx.design,
        &mut ctx.modules,
        ctx.name,
        is_top_task,
        control_plan,
    );
    if !is_top_task {
        frames::thread_views(ctx.design.design(), &mut ctx.modules, ctx.name);
    }
    Ok(())
}

/// Task pass: create the per-task FSM module, unless a distributed control
/// plan replaces it.
pub fn create_fsm_module(ctx: &mut TaskPassCtx<'_>) -> Result<(), CodegenError> {
    if ctx.inputs.control_plan.is_none() {
        ctx.fsms.create_fsm_module(ctx.name)?;
    }
    Ok(())
}

/// Task pass: generate the child-instance signals and wiring, remembering
/// the `is_done` nets for the control stage.
pub fn generate_child_signals(ctx: &mut TaskPassCtx<'_>) -> Result<(), CodegenError> {
    let is_done_signals = crate::passes::children::generate_child_signals(ctx)?;
    ctx.inputs.is_done_signals = is_done_signals;
    Ok(())
}

/// Task pass: instantiate and connect the task's FIFO storage.
pub fn fifo_instantiate_connect(ctx: &mut TaskPassCtx<'_>) -> Result<(), CodegenError> {
    crate::passes::fifos::instantiate_fifos(ctx.design, &mut ctx.modules, ctx.name)?;
    crate::passes::fifos::connect_fifos(ctx.design, &mut ctx.modules, ctx.name)?;
    Ok(())
}

/// Task pass: instantiate the rings of the top's frame arguments.
pub fn frame_rings(ctx: &mut TaskPassCtx<'_>) -> Result<(), CodegenError> {
    if ctx.inputs.is_top_task {
        rings::instantiate_rings(ctx.design, &mut ctx.modules, ctx.name)?;
    }
    Ok(())
}

/// Task pass: add M-AXI ports and crossbars, reusing the precomputed mmap
/// connections.
pub fn m_axi_crossbars(ctx: &mut TaskPassCtx<'_>) -> Result<(), CodegenError> {
    crate::m_axi::add_m_axi_and_crossbars(
        &mut ctx.modules,
        &mut ctx.outputs,
        ctx.name,
        &ctx.inputs.mmap_conns,
    )?;
    Ok(())
}

/// Task pass: instantiate floorplan-routed direct M-AXI pipelines (top task
/// only; the plan is `None` otherwise).
pub fn axi_pipeline_instantiate(ctx: &mut TaskPassCtx<'_>) -> Result<(), CodegenError> {
    if let Some(plan) = &ctx.inputs.axi_pipeline_plan {
        plan.instantiate(&mut ctx.modules, ctx.name)?;
    }
    Ok(())
}

/// Task pass: apply the global FSM — from the distributed control plan when
/// one exists, otherwise by programming the per-task FSM module directly.
pub fn control_fsm(ctx: &mut TaskPassCtx<'_>) -> Result<(), CodegenError> {
    if let Some(plan) = &ctx.inputs.control_plan {
        plan.instantiate_global(&mut ctx.modules, ctx.name, &ctx.inputs.is_done_signals)?;
    } else if let Some(fsm_mm) = ctx.fsms.get_mut(ctx.name) {
        crate::program::apply_global_fsm(fsm_mm, &ctx.inputs.is_done_signals);
    }
    Ok(())
}

/// Task pass: instantiate the top-level `s_axi` control slave (top task
/// only).
#[allow(
    clippy::unnecessary_wraps,
    reason = "every pass shares one signature so the stage table stays uniform"
)]
pub fn s_axi_control(ctx: &mut TaskPassCtx<'_>) -> Result<(), CodegenError> {
    if ctx.inputs.is_top_task {
        self::s_axi::instantiate_top_control_s_axi(
            ctx.design,
            &mut ctx.modules,
            ctx.name,
            ctx.inputs.top_instantiates_control_s_axi,
        );
    }
    Ok(())
}

/// The (`dl_x`, `dl_y`) slot of an atomic-region name, when it parses.
fn parse_atomic_region(region: &str) -> Option<(u32, u32)> {
    Coor::from_atomic_region_name(region).map(|coor| (coor.dl_x, coor.dl_y))
}

/// A floorplan-consistency failure, shared by the placement-reading passes.
fn invalid_floorplan(detail: impl Into<String>) -> CodegenError {
    CodegenError::InvalidFloorplan(detail.into())
}

/// The optional AXI address-channel outputs (AW/AR LOCK/CACHE/PROT/QOS) and
/// the constants they tie off to, keyed by lowercase signal stem; use sites
/// adjust the case to their port spelling.
const OPTIONAL_AXI_ADDRESS_PORTS: [(&str, &str); 8] = [
    ("awlock", "1'b0"),
    ("awcache", "4'b0011"),
    ("awprot", "3'b000"),
    ("awqos", "4'b0000"),
    ("arlock", "1'b0"),
    ("arcache", "4'b0011"),
    ("arprot", "3'b000"),
    ("arqos", "4'b0000"),
];

/// The driver expression for a scalar argument binding: a scalar is the one
/// binding that can be a constant rather than a parent wire; this is where
/// the constant becomes Verilog.
fn scalar_driver(source: &tapa_ir::ArgSource) -> Expr {
    match source {
        tapa_ir::ArgSource::Name(name) => Expr::ident(sanitize_array_name(name)),
        tapa_ir::ArgSource::Literal(value) => Expr::lit(value.to_string()),
    }
}

/// The offset source for a direct-mmap argument bound to `parent`: the
/// parent's `<arg>_offset` wire, or `64'd0` for hmap connections.
fn mmap_offset_source(parent: &str, mmap_connections: &BTreeMap<String, MMapConnection>) -> Expr {
    let parent_name = sanitize_array_name(parent);
    match mmap_connections.get(parent) {
        Some(connection) if connection.geometry.is_hmap() => Expr::lit("64'd0"),
        _ => Expr::ident(format!("{parent_name}_offset")),
    }
}

/// A wire named `name` of `width` bits; a 1-bit wire carries no range.
fn width_wire(name: &str, width: u32) -> tapa_rtl::signal::Signal {
    if width > 1 {
        tapa_rtl::mutation::wide_wire(name, &(width - 1).to_string(), "0")
    } else {
        tapa_rtl::mutation::wire(name)
    }
}
