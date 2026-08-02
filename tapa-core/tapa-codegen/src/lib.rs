//! RTL code generation from the TAPA topology model: Verilog fragments are
//! built with the `tapa-rtl` builder API and existing HLS modules are
//! modified through the hybrid mutation API.

mod artifact;
mod emit;
pub mod error;
mod instance_signals;
mod passes;
mod program;
mod state;
pub mod support_assets;
mod target;
mod template;

pub use artifact::ArtifactManifest;
pub use passes::m_axi;
pub use state::rtl_state;
pub use target::top_stream_needs_axis_adapter;

use tapa_ir::{task::TaskLevel, SynthTarget};

use crate::error::CodegenError;
use crate::passes::{DesignPassCtx, TaskPassCtx, TaskStageInputs};
use crate::rtl_state::TopologyWithRtl;

/// A pipeline stage that runs once over the whole design.
pub(crate) trait DesignPass {
    /// Run the pass.
    fn run(&self, ctx: &mut DesignPassCtx<'_>) -> Result<(), CodegenError>;
}

/// A pipeline stage that runs in order for each upper-level, non-`Ignore`
/// task (in `BTreeMap` task order).
pub(crate) trait TaskPass {
    /// Run the pass for a single task.
    fn run(&self, ctx: &mut TaskPassCtx<'_>) -> Result<(), CodegenError>;
}

/// The task-scoped passes, in stage order.
///
/// They run for each upper-level, non-`Ignore` task (in `BTreeMap` task
/// order), between the [`passes::IgnoreTaskShells`] and
/// [`emit::CollectOutputs`] design passes.
const TASK_PASSES: &[&dyn TaskPass] = &[
    &passes::CleanupHlsArtifacts,
    &passes::CreateFsmModule,
    &passes::GenerateChildSignals,
    &passes::FifoInstantiateConnect,
    &passes::MAxiCrossbars,
    &passes::AxiPipelineInstantiate,
    &passes::ControlFsm,
    &passes::SAxiControl,
];

/// Run the full RTL codegen pipeline and return
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

    passes::IgnoreTaskShells.run(&mut DesignPassCtx::new(&mut *state))?;

    for task_name in &task_names {
        let task = &state.design.tasks[task_name];
        if task.synth == SynthTarget::Ignore || task.level != TaskLevel::Upper {
            continue;
        }
        let mut inputs = TaskStageInputs::prepare(state, task_name)?;
        for pass in TASK_PASSES {
            pass.run(&mut TaskPassCtx::new(&mut *state, task_name, &mut inputs))?;
        }
    }

    emit::CollectOutputs.run(&mut DesignPassCtx::new(&mut *state))?;

    Ok(ArtifactManifest::collect(state))
}

/// Test-only: parse a [`tapa_ir::Design`] from fixture JSON.
#[cfg(test)]
pub(crate) fn design_from_fixture_json(value: serde_json::Value) -> tapa_ir::Design {
    serde_json::from_value(value).expect("valid design fixture JSON")
}
