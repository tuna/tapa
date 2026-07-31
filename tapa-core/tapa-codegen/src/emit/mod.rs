//! Output collection: gather the pass-produced RTL into the file maps
//! (`generated_files`, `template_files`) that the CLI then packages.

use tapa_ir::task::TaskLevel;
use tapa_ir::SynthTarget;

use crate::error::CodegenError;
use crate::passes::PassCtx;
use crate::RtlPass;

/// Design pass: collect the emitted files.
///
/// Runs last: every mutation pass must have run before the Verilog is
/// emitted into `generated_files` / `template_files`.
pub struct CollectOutputs;

impl RtlPass for CollectOutputs {
    fn run(&self, ctx: &mut PassCtx<'_>) -> Result<(), CodegenError> {
        let state = &mut *ctx.state;

        // `Ignore` tasks: emit the authoritative template file from the
        // shell built by the `ignore-task-shells` pass.
        let task_names: Vec<String> = state.design.tasks.keys().cloned().collect();
        for task_name in &task_names {
            let task = &state.design.tasks[task_name];
            if task.synth == SynthTarget::Ignore {
                let template = state.module_map[task_name].emit();
                state
                    .template_files
                    .insert(format!("{task_name}.v"), template);
            }
        }

        // Collect emitted files. Lower HLS modules were already copied from
        // their original Verilog sources by the CLI; re-emitting them from the
        // parsed model drops legal port-reg redeclarations used by HLS.
        for (name, mm) in &state.module_map {
            if state
                .design
                .tasks
                .get(name.as_str())
                .is_some_and(|task| {
                    task.level == TaskLevel::Upper || task.synth == SynthTarget::Ignore
                })
            {
                state.generated_files.insert(format!("{name}.v"), mm.emit());
            }
        }
        for (name, mm) in &state.fsm_modules {
            state
                .generated_files
                .insert(format!("{name}_fsm.v"), mm.emit());
        }

        Ok(())
    }
}
