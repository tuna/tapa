//! Composite CLI commands.
//!
//! `tapa compile` materializes the union of its constituent commands'
//! flag surfaces by flattening the underlying `Args` structs.

use clap::Parser;

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
    analyze::run(&args.analyze, ctx)?;
    synth::run(&args.synth, ctx)?;
    pack::run(&args.pack, ctx)
}

#[cfg(test)]
mod tests;
