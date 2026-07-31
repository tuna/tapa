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

/// A pipeline stage. Thin delegate in Phase 1a; see [`passes`].
pub(crate) trait RtlPass: Sync {
    /// Run the pass. Task-scoped passes see `ctx.task == Some(..)`.
    fn run(&self, ctx: &mut PassCtx<'_>) -> Result<(), CodegenError>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PassScope {
    Design,
    UpperTask,
}

/// One row of [`PIPELINE`]: scope and the delegate.
struct PipelineEntry {
    scope: PassScope,
    pass: &'static dyn RtlPass,
}

const fn entry(scope: PassScope, pass: &'static dyn RtlPass) -> PipelineEntry {
    PipelineEntry { scope, pass }
}

/// The pipeline as an ordered pass table. `Design` rows run once; each
/// maximal run of `UpperTask` rows runs in order for each upper-level,
/// non-`Ignore` task (in `BTreeMap` task order) — reproducing the
/// pre-refactor hand-written call sequence exactly.
static PIPELINE: &[PipelineEntry] = &[
    entry(PassScope::Design, &passes::IgnoreTaskShells),
    entry(PassScope::UpperTask, &passes::CleanupHlsArtifacts),
    entry(PassScope::UpperTask, &passes::CreateFsmModule),
    entry(PassScope::UpperTask, &passes::GenerateChildSignals),
    entry(PassScope::UpperTask, &passes::FifoInstantiateConnect),
    entry(PassScope::UpperTask, &passes::MAxiCrossbars),
    entry(PassScope::UpperTask, &passes::AxiPipelineInstantiate),
    entry(PassScope::UpperTask, &passes::ControlFsm),
    entry(PassScope::UpperTask, &passes::SAxiControl),
    entry(PassScope::Design, &emit::CollectOutputs),
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

    /// The driver contract is the scope shape: one leading `Design` run,
    /// one maximal `UpperTask` run per task, one trailing `Design` run.
    /// Byte-level stage-order drift is the golden tests' job, not a second
    /// copy of the table here.
    #[test]
    fn pipeline_has_the_expected_scope_shape() {
        use PassScope::{Design, UpperTask as Task};
        let scopes: Vec<PassScope> = PIPELINE.iter().map(|entry| entry.scope).collect();
        assert_eq!(
            scopes,
            [Design, Task, Task, Task, Task, Task, Task, Task, Task, Design]
        );
    }
}

#[cfg(test)]
#[path = "generate_rtl_tests.rs"]
mod tests;
