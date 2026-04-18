//! `tapa pack` — clap parity with `tapa/steps/pack.py`. Body delegates
//! to the Python bridge for now.

use std::path::PathBuf;

use clap::Parser;

use crate::context::CliContext;
use crate::error::Result;
use crate::steps::python_bridge;

#[derive(Debug, Parser)]
#[command(name = "pack", about = "Pack the generated RTL into a Xilinx object file.")]
pub struct Args {
    /// Output `.xo` (Vitis target) or `.zip` (HLS target).
    #[arg(short = 'o', long = "output", value_name = "FILE")]
    pub output: Option<PathBuf>,

    /// Bitstream-generation script path.
    #[arg(short = 's', long = "bitstream-script", value_name = "FILE")]
    pub bitstream_script: Option<PathBuf>,

    /// Custom RTL files / folders (may repeat).
    #[arg(long = "custom-rtl", value_name = "PATH")]
    pub custom_rtl: Vec<PathBuf>,

    /// `GraphIR` file to embed in the `.xo`.
    #[arg(long = "graphir-path", value_name = "FILE")]
    pub graphir_path: Option<PathBuf>,
}

pub fn to_python_argv(args: &Args) -> Vec<String> {
    let mut out = Vec::<String>::new();
    if let Some(p) = &args.output {
        out.push("--output".to_string());
        out.push(p.display().to_string());
    }
    if let Some(p) = &args.bitstream_script {
        out.push("--bitstream-script".to_string());
        out.push(p.display().to_string());
    }
    for c in &args.custom_rtl {
        out.push("--custom-rtl".to_string());
        out.push(c.display().to_string());
    }
    if let Some(p) = &args.graphir_path {
        out.push("--graphir-path".to_string());
        out.push(p.display().to_string());
    }
    out
}

pub fn run(args: &Args, ctx: &mut CliContext) -> Result<()> {
    python_bridge::require_enabled("pack")?;
    python_bridge::run("pack", &to_python_argv(args), ctx)
}
