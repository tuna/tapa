//! `tapa compile` and `tapa compile-with-floorplan-dse` — composite
//! commands matching `tapa/steps/meta.py`. Bodies bridge to Python while
//! the per-step Rust ports stabilize.

use clap::Parser;

use crate::context::CliContext;
use crate::error::Result;
use crate::steps::python_bridge;

#[derive(Debug, Parser)]
#[command(
    name = "compile",
    about = "Compile a TAPA program to a hardware design (analyze + synth + pack)."
)]
pub struct CompileArgs {
    /// Forwarded verbatim to the bridged Python `compile` command.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub argv: Vec<String>,
}

#[derive(Debug, Parser)]
#[command(
    name = "compile-with-floorplan-dse",
    about = "Compile a TAPA program with floorplan design space exploration."
)]
pub struct CompileWithFloorplanDseArgs {
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub argv: Vec<String>,
}

pub fn run_compile(args: &CompileArgs, ctx: &mut CliContext) -> Result<()> {
    python_bridge::require_enabled("compile")?;
    python_bridge::run("compile", &args.argv, ctx)
}

pub fn run_compile_with_floorplan_dse(
    args: &CompileWithFloorplanDseArgs,
    ctx: &mut CliContext,
) -> Result<()> {
    python_bridge::require_enabled("compile-with-floorplan-dse")?;
    python_bridge::run("compile-with-floorplan-dse", &args.argv, ctx)
}
