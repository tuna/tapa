//! `tapa floorplan` and `tapa generate-floorplan` — clap parity with
//! `tapa/steps/floorplan.py`. Bodies bridge to Python for now.

use std::path::PathBuf;

use clap::Parser;

use crate::context::CliContext;
use crate::error::Result;
use crate::steps::python_bridge;

#[derive(Debug, Parser)]
#[command(
    name = "floorplan",
    about = "Floorplan TAPA program and store the program description."
)]
pub struct FloorplanArgs {
    #[arg(long = "floorplan-path", value_name = "FILE")]
    pub floorplan_path: Option<PathBuf>,
}

#[derive(Debug, Parser)]
#[command(
    name = "generate-floorplan",
    about = "Generate floorplan solution(s) for a TAPA program via AutoBridge."
)]
pub struct GenerateFloorplanArgs {
    /// Path to the device configuration file.
    #[arg(long = "device-config", value_name = "FILE", required = true)]
    pub device_config: PathBuf,

    /// Path to the floorplan configuration file.
    #[arg(long = "floorplan-config", value_name = "FILE", required = true)]
    pub floorplan_config: PathBuf,
}

pub fn to_python_argv_floorplan(args: &FloorplanArgs) -> Vec<String> {
    let mut out = Vec::<String>::new();
    if let Some(p) = &args.floorplan_path {
        out.push("--floorplan-path".to_string());
        out.push(p.display().to_string());
    }
    out
}

pub fn to_python_argv_generate(args: &GenerateFloorplanArgs) -> Vec<String> {
    vec![
        "--device-config".to_string(),
        args.device_config.display().to_string(),
        "--floorplan-config".to_string(),
        args.floorplan_config.display().to_string(),
    ]
}

pub fn run_floorplan(args: &FloorplanArgs, ctx: &mut CliContext) -> Result<()> {
    python_bridge::require_enabled("floorplan")?;
    python_bridge::run("floorplan", &to_python_argv_floorplan(args), ctx)
}

pub fn run_generate_floorplan(
    args: &GenerateFloorplanArgs,
    ctx: &mut CliContext,
) -> Result<()> {
    python_bridge::require_enabled("generate-floorplan")?;
    python_bridge::run(
        "generate-floorplan",
        &to_python_argv_generate(args),
        ctx,
    )
}
