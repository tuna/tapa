//! Output collection: gather the pass-produced RTL into the file maps
//! (`generated_files`, `template_files`) that the CLI then packages.

use tapa_ir::task::TaskLevel;
use tapa_ir::SynthTarget;

use crate::passes::DesignPassCtx;

/// Design pass: collect the emitted files.
///
/// Runs last: every mutation pass must have run before the Verilog is
/// emitted into `generated_files` / `template_files`.
pub fn collect_outputs(ctx: &mut DesignPassCtx<'_>) {
    let design = ctx.design.design();

    // `Ignore` tasks emit the authoritative template file from the shell
    // built by the `ignore-task-shells` pass. Lower HLS modules were already
    // copied from their original Verilog sources by the CLI; re-emitting them
    // from the parsed model drops legal port-reg redeclarations used by HLS.
    for (name, task) in &design.tasks {
        let rtl = if task.synth == SynthTarget::Ignore {
            // The pass registers every `Ignore` task; a missing one is a bug.
            let rtl = ctx.modules[name].emit();
            ctx.outputs
                .insert_template(format!("{name}.v"), rtl.clone());
            rtl
        } else if task.level == TaskLevel::Upper {
            let Some(mm) = ctx.modules.get(name) else {
                continue;
            };
            mm.emit()
        } else {
            continue;
        };
        ctx.outputs.insert_generated(format!("{name}.v"), rtl);
    }
    for (name, mm) in ctx.fsms.iter() {
        ctx.outputs
            .insert_generated(format!("{name}_fsm.v"), mm.emit());
    }
}
