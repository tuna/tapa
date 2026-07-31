//! The typed pass pipeline (REFACTOR-PLAN §4 Phase 1 item 1).
//!
//! Phase 1a is move-only: every pass body stays byte-identical in its home
//! module and the unit structs below are thin delegates registered in the
//! ordered [`crate::PIPELINE`] table. Narrowing [`PassCtx`] into per-concern
//! views is Phase 1b — do not do it in 1a.

mod cleanup;

use std::collections::BTreeMap;

use tapa_ir::SynthTarget;

use crate::axi_pipeline::DirectAxiPipelinePlan;
use crate::distributed_control::DistributedControlPlan;
use crate::error::CodegenError;
use crate::rtl_state::{MMapConnection, TopologyWithRtl};
use crate::RtlPass;

/// Context handed to each [`RtlPass`] in Phase 1a.
///
/// A thin bundle over the existing god-context (`TopologyWithRtl`) plus the
/// per-task staging area the driver precomputes.
pub struct PassCtx<'a> {
    /// The mutable design + RTL state every pass shares today.
    pub state: &'a mut TopologyWithRtl,
    /// Task identity + staged inputs; `Some` only while the driver runs a
    /// task-scoped pass group.
    pub task: Option<TaskPassCtx<'a>>,
}

/// Per-upper-task pass context.
pub struct TaskPassCtx<'a> {
    /// The upper-level task currently being instrumented.
    pub name: &'a str,
    /// Driver-precomputed plans and cross-pass staging for `name`.
    pub inputs: &'a mut TaskStageInputs,
}

/// Precomputed per-task inputs the pass group consumes.
///
/// Everything here is a pure function of the design/floorplan state as it
/// stands when the task's group starts, so computing it up front preserves
/// the historical "validate before any mutation" ordering.
pub struct TaskStageInputs {
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
            mmap_conns,
            mmap_slave_map,
            axi_pipeline_plan,
            control_plan,
            is_done_signals: Vec::new(),
        })
    }
}

/// Design pass: build the authoritative port-only shells for
/// `Ignore`-synthesized tasks into `module_map`.
pub struct IgnoreTaskShells;

impl RtlPass for IgnoreTaskShells {
    fn run(&self, ctx: &mut PassCtx<'_>) -> Result<(), CodegenError> {
        // Ignored tasks have no HLS result to attach. Build their
        // authoritative port-only shell from topology so parents can resolve
        // the module while the user authors the replacement RTL.
        let task_names: Vec<String> = ctx.state.design.tasks.keys().cloned().collect();
        for task_name in &task_names {
            let task = &ctx.state.design.tasks[task_name];
            if task.synth != SynthTarget::Ignore {
                continue;
            }
            let source = crate::template::render_task_template(task_name, task);
            let module = tapa_rtl::VerilogModule::parse(&source)?;
            ctx.state.module_map.insert(
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

impl RtlPass for CleanupHlsArtifacts {
    fn run(&self, ctx: &mut PassCtx<'_>) -> Result<(), CodegenError> {
        let task = ctx.task.as_mut().expect("cleanup-hls-artifacts is task-scoped");
        let is_top_task = task.name == ctx.state.design.top;
        let control_plan = task.inputs.control_plan.as_ref();
        cleanup::cleanup_hls_artifacts(ctx.state, task.name, is_top_task, control_plan);
        Ok(())
    }
}

/// Task pass: create the per-task FSM module, unless a distributed control
/// plan replaces it.
pub struct CreateFsmModule;

impl RtlPass for CreateFsmModule {
    fn run(&self, ctx: &mut PassCtx<'_>) -> Result<(), CodegenError> {
        let task = ctx.task.as_mut().expect("create-fsm-module is task-scoped");
        if task.inputs.control_plan.is_none() {
            ctx.state.create_fsm_module(task.name)?;
        }
        Ok(())
    }
}

/// Task pass: generate the child-instance signals and wiring, remembering
/// the `is_done` nets for the control stage.
pub struct GenerateChildSignals;

impl RtlPass for GenerateChildSignals {
    fn run(&self, ctx: &mut PassCtx<'_>) -> Result<(), CodegenError> {
        let task = ctx.task.as_mut().expect("generate-child-signals is task-scoped");
        task.inputs.is_done_signals = crate::children::generate_child_signals(
            ctx.state,
            task.name,
            &task.inputs.mmap_conns,
            &task.inputs.mmap_slave_map,
            task.inputs.axi_pipeline_plan.as_ref(),
            task.inputs.control_plan.as_ref(),
        )?;
        Ok(())
    }
}

/// Task pass: instantiate and connect the task's FIFO storage.
pub struct FifoInstantiateConnect;

impl RtlPass for FifoInstantiateConnect {
    fn run(&self, ctx: &mut PassCtx<'_>) -> Result<(), CodegenError> {
        let task = ctx
            .task
            .as_mut()
            .expect("fifo-instantiate-connect is task-scoped");
        crate::fifos::instantiate_fifos(ctx.state, task.name)?;
        crate::fifos::connect_fifos(ctx.state, task.name)?;
        Ok(())
    }
}

/// Task pass: add M-AXI ports and crossbars, reusing the precomputed mmap
/// connections.
pub struct MAxiCrossbars;

impl RtlPass for MAxiCrossbars {
    fn run(&self, ctx: &mut PassCtx<'_>) -> Result<(), CodegenError> {
        let task = ctx.task.as_mut().expect("m-axi-crossbars is task-scoped");
        crate::m_axi::add_m_axi_and_crossbars(ctx.state, task.name, &task.inputs.mmap_conns)?;
        Ok(())
    }
}

/// Task pass: instantiate floorplan-routed direct M-AXI pipelines (top task
/// only; the plan is `None` otherwise).
pub struct AxiPipelineInstantiate;

impl RtlPass for AxiPipelineInstantiate {
    fn run(&self, ctx: &mut PassCtx<'_>) -> Result<(), CodegenError> {
        let task = ctx
            .task
            .as_mut()
            .expect("axi-pipeline-instantiate is task-scoped");
        if let Some(plan) = &task.inputs.axi_pipeline_plan {
            plan.instantiate(ctx.state, task.name)?;
        }
        Ok(())
    }
}

/// Task pass: apply the global FSM — from the distributed control plan when
/// one exists, otherwise by programming the per-task FSM module directly.
pub struct ControlFsm;

impl RtlPass for ControlFsm {
    fn run(&self, ctx: &mut PassCtx<'_>) -> Result<(), CodegenError> {
        let task = ctx.task.as_mut().expect("control-fsm is task-scoped");
        if let Some(plan) = &task.inputs.control_plan {
            plan.instantiate_global(ctx.state, task.name, &task.inputs.is_done_signals)?;
        } else if let Some(fsm_mm) = ctx.state.fsm_modules.get_mut(task.name) {
            crate::program::apply_global_fsm(fsm_mm, &task.inputs.is_done_signals);
        }
        Ok(())
    }
}

/// Task pass: instantiate the top-level `s_axi` control slave (top task
/// only).
pub struct SAxiControl;

impl RtlPass for SAxiControl {
    fn run(&self, ctx: &mut PassCtx<'_>) -> Result<(), CodegenError> {
        let task = ctx.task.as_mut().expect("s-axi-control is task-scoped");
        if task.name == ctx.state.design.top {
            crate::s_axi::instantiate_top_control_s_axi(ctx.state, task.name);
        }
        Ok(())
    }
}
