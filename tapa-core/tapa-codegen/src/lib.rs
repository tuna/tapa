//! RTL code generation from the TAPA topology model.
//!
//! It uses the `tapa-rtl` builder API to construct Verilog fragments and the
//! hybrid mutation API to modify existing HLS modules.

mod emit;
pub mod error;
pub mod instance_signals;
mod passes;
pub mod program;
mod state;
pub mod support_assets;
mod template;

pub use passes::{async_mmap, children, fifos, m_axi};
pub use state::rtl_state;

use tapa_ir::task::TaskLevel;
use tapa_ir::{SynthTarget, Target};

use crate::error::CodegenError;
use crate::passes::{PassCtx, TaskPassCtx, TaskStageInputs};
use crate::rtl_state::TopologyWithRtl;

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

/// A stage of the RTL codegen pipeline (REFACTOR-PLAN §4 Phase 1 item 1).
///
/// In Phase 1a the pass bodies stay in their home modules, byte-identical;
/// implementations are thin delegates. `Sync` so the [`PIPELINE`] table can
/// hold them in a `static`.
pub(crate) trait RtlPass: Sync {
    /// Run the pass over `ctx`. Task-scoped passes may rely on
    /// `ctx.task` being `Some`; design-scoped passes always see `None`.
    fn run(&self, ctx: &mut PassCtx<'_>) -> Result<(), CodegenError>;
}

/// Which driver loop runs a pipeline entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PassScope {
    /// Runs once for the whole design (shell prep, output collection).
    Design,
    /// Runs once per upper-level, non-`Ignore` task.
    UpperTask,
}

/// One row of [`PIPELINE`]: stage name, scope, and the delegate.
struct PipelineEntry {
    /// Asserted in the `pipeline_tests` module; documentation as data in
    /// non-test builds.
    #[cfg_attr(not(test), allow(dead_code, reason = "pinned by pipeline_tests"))]
    name: &'static str,
    scope: PassScope,
    pass: &'static dyn RtlPass,
}

/// Build one [`PIPELINE`] row.
const fn entry(name: &'static str, scope: PassScope, pass: &'static dyn RtlPass) -> PipelineEntry {
    PipelineEntry { name, scope, pass }
}

/// The pipeline as an ordered pass table (REFACTOR-PLAN §4 Phase 1 item 1).
///
/// `Design` rows run once; each maximal run of `UpperTask` rows runs in
/// order for each upper-level, non-`Ignore` task (in `BTreeMap` task order).
/// Together the rows reproduce the pre-refactor hand-written call sequence
/// exactly: ignore-task template shells first, the per-task instrumentation
/// stages second, and output collection last.
static PIPELINE: &[PipelineEntry] = &[
    entry("ignore-task-shells", PassScope::Design, &passes::IgnoreTaskShells),
    entry("cleanup-hls-artifacts", PassScope::UpperTask, &passes::CleanupHlsArtifacts),
    entry("create-fsm-module", PassScope::UpperTask, &passes::CreateFsmModule),
    entry("generate-child-signals", PassScope::UpperTask, &passes::GenerateChildSignals),
    entry("fifo-instantiate-connect", PassScope::UpperTask, &passes::FifoInstantiateConnect),
    entry("m-axi-crossbars", PassScope::UpperTask, &passes::MAxiCrossbars),
    entry("axi-pipeline-instantiate", PassScope::UpperTask, &passes::AxiPipelineInstantiate),
    entry("control-fsm", PassScope::UpperTask, &passes::ControlFsm),
    entry("s-axi-control", PassScope::UpperTask, &passes::SAxiControl),
    entry("collect-outputs", PassScope::Design, &emit::CollectOutputs),
];

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

    let mut index = 0;
    while index < PIPELINE.len() {
        match PIPELINE[index].scope {
            PassScope::Design => {
                PIPELINE[index].pass.run(&mut PassCtx {
                    state: &mut *state,
                    task: None,
                })?;
                index += 1;
            }
            PassScope::UpperTask => {
                // Run this whole maximal task-scoped run once per upper task,
                // in task order, staging (and validating) the per-task inputs
                // before any mutation pass runs.
                let group_end = PIPELINE[index..]
                    .iter()
                    .position(|entry| matches!(entry.scope, PassScope::Design))
                    .map_or(PIPELINE.len(), |offset| index + offset);
                for task_name in &task_names {
                    let task = &state.design.tasks[task_name];
                    if task.synth == SynthTarget::Ignore || task.level != TaskLevel::Upper {
                        continue;
                    }
                    let mut inputs = TaskStageInputs::prepare(state, task_name)?;
                    for pipeline_entry in &PIPELINE[index..group_end] {
                        pipeline_entry.pass.run(&mut PassCtx {
                            state: &mut *state,
                            task: Some(TaskPassCtx {
                                name: task_name,
                                inputs: &mut inputs,
                            }),
                        })?;
                    }
                }
                index = group_end;
            }
        }
    }

    Ok(())
}

/// Test-only: parse a [`tapa_ir::Design`] from fixture JSON.
#[cfg(test)]
pub(crate) fn design_from_fixture_json(value: serde_json::Value) -> tapa_ir::Design {
    serde_json::from_value(value).expect("valid design fixture JSON")
}

#[cfg(test)]
mod pipeline_tests {
    use super::{PassScope, PIPELINE};

    /// The declared table is the pipeline contract; pin stage names, scopes,
    /// and order so re-sequencing is a deliberate, reviewed change.
    #[test]
    fn pipeline_declares_the_documented_stage_order() {
        let stages: Vec<(&str, PassScope)> = PIPELINE
            .iter()
            .map(|entry| (entry.name, entry.scope))
            .collect();
        assert_eq!(
            stages,
            &[
                ("ignore-task-shells", PassScope::Design),
                ("cleanup-hls-artifacts", PassScope::UpperTask),
                ("create-fsm-module", PassScope::UpperTask),
                ("generate-child-signals", PassScope::UpperTask),
                ("fifo-instantiate-connect", PassScope::UpperTask),
                ("m-axi-crossbars", PassScope::UpperTask),
                ("axi-pipeline-instantiate", PassScope::UpperTask),
                ("control-fsm", PassScope::UpperTask),
                ("s-axi-control", PassScope::UpperTask),
                ("collect-outputs", PassScope::Design),
            ]
        );
    }
}

#[cfg(test)]
#[path = "generate_rtl_tests.rs"]
mod tests;
