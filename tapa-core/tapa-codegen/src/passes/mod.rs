//! The typed pass pipeline (REFACTOR-PLAN §4 Phase 1 item 1).
//!
//! Every pass body takes per-concern [`crate::state::views`] borrow views
//! (Phase 1b) instead of the whole [`TopologyWithRtl`]; the unit structs
//! below are thin delegates registered in the ordered [`crate::pipeline`]
//! table. The driver-side [`DesignPassCtx`] / [`TaskPassCtx`] carry the
//! views built from disjoint field borrows of the state. Only the context
//! constructors, the lib.rs driver, and [`TaskStageInputs::prepare`]
//! (read-only staging) may name the whole state object.

pub mod async_mmap;
mod axi_pipeline;
pub mod children;
mod cleanup;
mod distributed_control;
pub mod fifos;
mod floorplans;
pub mod m_axi;
mod s_axi;

use std::collections::BTreeMap;

use tapa_ir::SynthTarget;

use self::axi_pipeline::DirectAxiPipelinePlan;
use self::distributed_control::DistributedControlPlan;
use crate::error::CodegenError;
use crate::rtl_state::{MMapConnection, TopologyWithRtl};
use crate::state::views::{DesignView, FsmTable, ModuleTable, OutputSet};
use crate::{DesignPass, TaskPass};

/// Context handed to each [`DesignPass`]: the narrowed per-concern views
/// built from the state's disjoint fields (Phase 1b). Pass bodies never see
/// the whole `TopologyWithRtl`.
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
        DesignView::new(&state.design, state.floorplan.as_ref()),
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
/// disjoint fields (Phase 1b).
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
/// the historical "validate before any mutation" ordering.
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
    /// Compute every per-task plan up front so malformed memory geometry is
    /// rejected before any RTL state is mutated.
    pub fn prepare(state: &TopologyWithRtl, task_name: &str) -> Result<Self, CodegenError> {
        let is_top_task = task_name == state.design.top;

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
        let mut mmap_slave_map: std::collections::BTreeMap<(String, String, usize), usize> =
            std::collections::BTreeMap::new();
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
pub struct IgnoreTaskShells;

impl DesignPass for IgnoreTaskShells {
    fn run(&self, ctx: &mut DesignPassCtx<'_>) -> Result<(), CodegenError> {
        // Ignored tasks have no HLS result to attach. Build their
        // authoritative port-only shell from topology so parents can resolve
        // the module while the user authors the replacement RTL.
        let design = ctx.design.design();
        let task_names: Vec<String> = design.tasks.keys().cloned().collect();
        for task_name in &task_names {
            let task = &design.tasks[task_name];
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
}

/// Task pass: strip HLS artifacts from the task's module and normalize its
/// reset/handshake wiring before any child or FIFO wiring is added.
pub struct CleanupHlsArtifacts;

impl TaskPass for CleanupHlsArtifacts {
    fn run(&self, ctx: &mut TaskPassCtx<'_>) -> Result<(), CodegenError> {
        let is_top_task = ctx.inputs.is_top_task;
        let control_plan = ctx.inputs.control_plan.as_ref();
        cleanup::cleanup_hls_artifacts(
            ctx.design,
            &mut ctx.modules,
            ctx.name,
            is_top_task,
            control_plan,
        );
        Ok(())
    }
}

/// Task pass: create the per-task FSM module, unless a distributed control
/// plan replaces it.
pub struct CreateFsmModule;

impl TaskPass for CreateFsmModule {
    fn run(&self, ctx: &mut TaskPassCtx<'_>) -> Result<(), CodegenError> {
        if ctx.inputs.control_plan.is_none() {
            let task_level = ctx
                .design
                .design()
                .tasks
                .get(ctx.name)
                .ok_or_else(|| CodegenError::TaskNotFound(ctx.name.to_owned()))?
                .level;
            ctx.fsms.create_fsm_module(ctx.name, task_level)?;
        }
        Ok(())
    }
}

/// Task pass: generate the child-instance signals and wiring, remembering
/// the `is_done` nets for the control stage.
pub struct GenerateChildSignals;

impl TaskPass for GenerateChildSignals {
    fn run(&self, ctx: &mut TaskPassCtx<'_>) -> Result<(), CodegenError> {
        let is_done_signals = crate::passes::children::generate_child_signals(ctx)?;
        ctx.inputs.is_done_signals = is_done_signals;
        Ok(())
    }
}

/// Task pass: instantiate and connect the task's FIFO storage.
pub struct FifoInstantiateConnect;

impl TaskPass for FifoInstantiateConnect {
    fn run(&self, ctx: &mut TaskPassCtx<'_>) -> Result<(), CodegenError> {
        crate::passes::fifos::instantiate_fifos(ctx.design, &mut ctx.modules, ctx.name)?;
        crate::passes::fifos::connect_fifos(ctx.design, &mut ctx.modules, ctx.name)?;
        Ok(())
    }
}

/// Task pass: add M-AXI ports and crossbars, reusing the precomputed mmap
/// connections.
pub struct MAxiCrossbars;

impl TaskPass for MAxiCrossbars {
    fn run(&self, ctx: &mut TaskPassCtx<'_>) -> Result<(), CodegenError> {
        crate::m_axi::add_m_axi_and_crossbars(
            &mut ctx.modules,
            &mut ctx.outputs,
            ctx.name,
            &ctx.inputs.mmap_conns,
        )?;
        Ok(())
    }
}

/// Task pass: instantiate floorplan-routed direct M-AXI pipelines (top task
/// only; the plan is `None` otherwise).
pub struct AxiPipelineInstantiate;

impl TaskPass for AxiPipelineInstantiate {
    fn run(&self, ctx: &mut TaskPassCtx<'_>) -> Result<(), CodegenError> {
        if let Some(plan) = &ctx.inputs.axi_pipeline_plan {
            plan.instantiate(&mut ctx.modules, ctx.name)?;
        }
        Ok(())
    }
}

/// Task pass: apply the global FSM — from the distributed control plan when
/// one exists, otherwise by programming the per-task FSM module directly.
pub struct ControlFsm;

impl TaskPass for ControlFsm {
    fn run(&self, ctx: &mut TaskPassCtx<'_>) -> Result<(), CodegenError> {
        if let Some(plan) = &ctx.inputs.control_plan {
            plan.instantiate_global(&mut ctx.modules, ctx.name, &ctx.inputs.is_done_signals)?;
        } else if let Some(fsm_mm) = ctx.fsms.get_mut(ctx.name) {
            crate::program::apply_global_fsm(fsm_mm, &ctx.inputs.is_done_signals);
        }
        Ok(())
    }
}

/// Task pass: instantiate the top-level `s_axi` control slave (top task
/// only).
pub struct SAxiControl;

impl TaskPass for SAxiControl {
    fn run(&self, ctx: &mut TaskPassCtx<'_>) -> Result<(), CodegenError> {
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
}
