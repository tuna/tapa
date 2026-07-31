//! RTL code generation from the TAPA topology model: Verilog fragments are
//! built with the `tapa-rtl` builder API and existing HLS modules are
//! modified through the hybrid mutation API.

mod emit;
pub mod error;
pub mod instance_signals;
mod passes;
pub mod program;
mod state;
pub mod support_assets;
mod target;
mod template;

pub use passes::{async_mmap, children, fifos, m_axi};
pub use state::rtl_state;
pub use target::top_stream_needs_axis_adapter;

use tapa_ir::{task::TaskLevel, SynthTarget};

use crate::error::CodegenError;
use crate::passes::{PassCtx, TaskPassCtx, TaskStageInputs};
use crate::rtl_state::TopologyWithRtl;

/// A pipeline stage (REFACTOR-PLAN §4 Phase 1 item 1): in Phase 1a a thin
/// delegate over the byte-identical pass bodies in [`passes`] / [`emit`].
pub(crate) trait RtlPass: Sync {
    /// Run the pass. Task-scoped passes see `ctx.task == Some(..)`.
    fn run(&self, ctx: &mut PassCtx<'_>) -> Result<(), CodegenError>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PassScope {
    Design,
    UpperTask,
}

/// One row of [`PIPELINE`]: stage name, scope, and the delegate.
struct PipelineEntry {
    #[cfg_attr(not(test), allow(dead_code, reason = "pinned by pipeline_tests"))]
    name: &'static str,
    scope: PassScope,
    pass: &'static dyn RtlPass,
}

const fn entry(name: &'static str, scope: PassScope, pass: &'static dyn RtlPass) -> PipelineEntry {
    PipelineEntry { name, scope, pass }
}

/// The pipeline as an ordered pass table. `Design` rows run once; each
/// maximal run of `UpperTask` rows runs in order for each upper-level,
/// non-`Ignore` task (in `BTreeMap` task order) — reproducing the
/// pre-refactor hand-written call sequence exactly.
static PIPELINE: &[PipelineEntry] = &[
    entry(
        "ignore-task-shells",
        PassScope::Design,
        &passes::IgnoreTaskShells,
    ),
    entry(
        "cleanup-hls-artifacts",
        PassScope::UpperTask,
        &passes::CleanupHlsArtifacts,
    ),
    entry(
        "create-fsm-module",
        PassScope::UpperTask,
        &passes::CreateFsmModule,
    ),
    entry(
        "generate-child-signals",
        PassScope::UpperTask,
        &passes::GenerateChildSignals,
    ),
    entry(
        "fifo-instantiate-connect",
        PassScope::UpperTask,
        &passes::FifoInstantiateConnect,
    ),
    entry(
        "m-axi-crossbars",
        PassScope::UpperTask,
        &passes::MAxiCrossbars,
    ),
    entry(
        "axi-pipeline-instantiate",
        PassScope::UpperTask,
        &passes::AxiPipelineInstantiate,
    ),
    entry("control-fsm", PassScope::UpperTask, &passes::ControlFsm),
    entry("s-axi-control", PassScope::UpperTask, &passes::SAxiControl),
    entry("collect-outputs", PassScope::Design, &emit::CollectOutputs),
];

/// Run the full RTL codegen pipeline (the [`PIPELINE`] driver).
///
/// Per-task inputs are staged (and validated) before any mutation pass
/// runs; the file maps on `state` then hold the modified modules and
/// auxiliary files.
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

    /// The declared table is the pipeline contract; pin stages and order.
    #[test]
    fn pipeline_declares_the_documented_stage_order() {
        use PassScope::{Design, UpperTask as Task};
        let stages: Vec<(&str, PassScope)> = PIPELINE
            .iter()
            .map(|entry| (entry.name, entry.scope))
            .collect();
        assert_eq!(
            stages,
            [
                ("ignore-task-shells", Design),
                ("cleanup-hls-artifacts", Task),
                ("create-fsm-module", Task),
                ("generate-child-signals", Task),
                ("fifo-instantiate-connect", Task),
                ("m-axi-crossbars", Task),
                ("axi-pipeline-instantiate", Task),
                ("control-fsm", Task),
                ("s-axi-control", Task),
                ("collect-outputs", Design),
            ]
        );
    }
}

#[cfg(test)]
#[path = "generate_rtl_tests.rs"]
mod tests;
