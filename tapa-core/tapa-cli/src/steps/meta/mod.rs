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
mod tests {
    use super::*;

    #[test]
    fn compile_args_round_trip_via_clap() {
        let args = CompileArgs::try_parse_from([
            "compile",
            "--input",
            "vadd.cpp",
            "--top",
            "VecAdd",
            "--platform",
            "xilinx_u250",
            "--output",
            "vadd.xo",
        ])
        .expect("compile args parse");
        assert_eq!(args.analyze.input_files.len(), 1);
        assert_eq!(args.analyze.top, "VecAdd");
        assert_eq!(args.synth.platform.as_deref(), Some("xilinx_u250"));
        assert_eq!(
            args.pack.output.as_ref().map(|p| p.display().to_string()),
            Some("vadd.xo".to_string()),
        );
    }
}
