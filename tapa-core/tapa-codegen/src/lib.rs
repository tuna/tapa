//! RTL code generation from the TAPA topology model: Verilog fragments are
//! built with the `tapa-rtl` builder API and existing HLS modules are
//! modified through the hybrid mutation API.

mod artifact;
mod emit;
pub mod error;
pub mod instance_signals;
mod passes;
pub mod program;
mod state;
pub mod support_assets;
mod target;
mod template;

pub use artifact::ArtifactManifest;
pub use passes::{async_mmap, children, fifos, m_axi};
pub use state::rtl_state;
pub use target::top_stream_needs_axis_adapter;

use tapa_ir::{task::TaskLevel, SynthTarget};

use crate::error::CodegenError;
use crate::passes::{DesignPassCtx, TaskPassCtx, TaskStageInputs};
use crate::rtl_state::TopologyWithRtl;

/// A pipeline stage that runs once over the whole design.
pub(crate) trait DesignPass {
    /// Run the pass.
    fn run(&mut self, ctx: &mut DesignPassCtx<'_>) -> Result<(), CodegenError>;
}

/// A pipeline stage that runs in order for each upper-level, non-`Ignore`
/// task (in `BTreeMap` task order).
pub(crate) trait TaskPass {
    /// Run the pass for a single task.
    fn run(&mut self, ctx: &mut TaskPassCtx<'_>) -> Result<(), CodegenError>;
}

/// One entry in the [`pipeline`] table.
enum PipelineEntry {
    /// A design pass that runs once over the whole design.
    Design(Box<dyn DesignPass>),
    /// An ordered group of task passes, run per eligible task.
    TaskGroup(Vec<Box<dyn TaskPass>>),
}

/// The pipeline as an ordered pass table.
///
/// `Design` entries run once; the task group runs in order for each
/// upper-level, non-`Ignore` task (in `BTreeMap` task order) — reproducing
/// the pre-refactor hand-written call sequence exactly.
fn pipeline() -> Vec<PipelineEntry> {
    use PipelineEntry::{Design, TaskGroup};
    vec![
        Design(Box::new(passes::IgnoreTaskShells)),
        TaskGroup(vec![
            Box::new(passes::CleanupHlsArtifacts),
            Box::new(passes::CreateFsmModule),
            Box::new(passes::GenerateChildSignals),
            Box::new(passes::FifoInstantiateConnect),
            Box::new(passes::MAxiCrossbars),
            Box::new(passes::AxiPipelineInstantiate),
            Box::new(passes::ControlFsm),
            Box::new(passes::SAxiControl),
        ]),
        Design(Box::new(emit::CollectOutputs)),
    ]
}

/// Run the full RTL codegen pipeline (the [`pipeline`] driver) and return
/// the complete [`ArtifactManifest`].
///
/// Per-task inputs are staged (and validated) before any mutation pass
/// runs; the file maps on `state` then hold the modified modules and
/// auxiliary files. The manifest is assembled from those maps plus the
/// embedded support assets — the codegen charter's output type, whose
/// packaging (CLI, golden harness) is a copy operation. The `state` file
/// maps remain public for now; consumers must not re-derive the shipped
/// set from them.
pub fn generate_rtl(state: &mut TopologyWithRtl) -> Result<ArtifactManifest, CodegenError> {
    let task_names: Vec<String> = state.design.tasks.keys().cloned().collect();

    for entry in pipeline() {
        match entry {
            PipelineEntry::Design(mut pass) => {
                pass.run(&mut DesignPassCtx::new(&mut *state))?;
            }
            PipelineEntry::TaskGroup(mut group) => {
                for task_name in &task_names {
                    let task = &state.design.tasks[task_name];
                    if task.synth == SynthTarget::Ignore || task.level != TaskLevel::Upper {
                        continue;
                    }
                    let mut inputs = TaskStageInputs::prepare(state, task_name)?;
                    for pass in &mut group {
                        pass.run(&mut TaskPassCtx::new(&mut *state, task_name, &mut inputs))?;
                    }
                }
            }
        }
    }

    Ok(ArtifactManifest::collect(state))
}

/// Test-only: parse a [`tapa_ir::Design`] from fixture JSON.
#[cfg(test)]
pub(crate) fn design_from_fixture_json(value: serde_json::Value) -> tapa_ir::Design {
    serde_json::from_value(value).expect("valid design fixture JSON")
}

#[cfg(test)]
mod pipeline_tests {
    use super::{pipeline, PipelineEntry};

    /// The driver contract is the pipeline shape: one leading design pass,
    /// one task group of eight passes per eligible task, one trailing
    /// design pass. Byte-level stage-order drift is the golden tests' job,
    /// not a second copy of the table here.
    #[test]
    fn pipeline_has_the_expected_shape() {
        let shape: Vec<_> = pipeline()
            .iter()
            .map(|entry| match entry {
                PipelineEntry::Design(_) => ("design", 0),
                PipelineEntry::TaskGroup(group) => ("task-group", group.len()),
            })
            .collect();
        assert_eq!(shape, [("design", 0), ("task-group", 8), ("design", 0)]);
    }
}
