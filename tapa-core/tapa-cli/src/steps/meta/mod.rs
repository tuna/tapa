//! Composite CLI commands.
//!
//! `tapa compile` materializes the union of its constituent commands'
//! flag surfaces by flattening the underlying `Args` structs.

use clap::Parser;

use crate::chain::{run_pipeline_step, PipelineStep};
use crate::context::CliContext;
use crate::error::Result;
use crate::steps::{analyze, pack, synth};

// ---------------------------------------------------------------------
// `compile` = analyze + synth + pack
// ---------------------------------------------------------------------
//
// No flag conflicts among analyze / synth / pack so we flatten directly.

#[derive(Debug, Clone, Parser)]
#[command(
    name = "compile",
    about = "Compile a TAPA program to a hardware design (analyze + synth + pack)."
)]
pub struct CompileArgs {
    #[command(flatten)]
    pub analyze: analyze::AnalyzeArgs,
    #[command(flatten)]
    pub synth: synth::SynthArgs,
    #[command(flatten)]
    pub pack: pack::PackArgs,
}

pub fn run_compile_composite(args: &CompileArgs, ctx: &mut CliContext) -> Result<()> {
    for step in [
        PipelineStep::Analyze(&args.analyze),
        PipelineStep::Synth(&args.synth),
        PipelineStep::Pack(&args.pack),
    ] {
        run_pipeline_step(step, ctx)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
