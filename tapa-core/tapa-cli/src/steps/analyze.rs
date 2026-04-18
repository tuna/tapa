//! `tapa analyze` — clap-parsed parity with `tapa/steps/analyze.py`.
//! For now, the body delegates to the Python bridge. The native HLS / IR
//! port lands in a follow-up round per the plan's lower bound.

use std::path::PathBuf;

use clap::Parser;

use crate::context::CliContext;
use crate::error::Result;
use crate::steps::python_bridge;

#[derive(Debug, Clone, Parser)]
#[command(
    name = "analyze",
    about = "Analyze TAPA program and store the program description."
)]
pub struct AnalyzeArgs {
    /// Input file, usually TAPA C++ source code (may repeat).
    #[arg(short = 'f', long = "input", value_name = "FILE", required = true)]
    pub input_files: Vec<PathBuf>,

    /// Name of the top-level task.
    #[arg(short = 't', long = "top", value_name = "TASK", required = true)]
    pub top: String,

    /// Compiler flags for the kernel; may appear many times.
    #[arg(short = 'c', long = "cflags", value_name = "FLAG")]
    pub cflags: Vec<String>,

    /// Flatten the hierarchy with all leaf-level tasks at top.
    #[arg(long = "flatten-hierarchy", default_value_t = false)]
    pub flatten_hierarchy: bool,

    /// Counterpart to `--flatten-hierarchy`; default behavior.
    #[arg(long = "keep-hierarchy", conflicts_with = "flatten_hierarchy")]
    pub keep_hierarchy: bool,

    /// Target flow.
    #[arg(long = "target", default_value = "xilinx-vitis")]
    pub target: String,
}

/// Re-render `args` as the click-flavored argv the Python step expects.
pub fn to_python_argv(args: &AnalyzeArgs) -> Vec<String> {
    let mut out = Vec::<String>::new();
    for f in &args.input_files {
        out.push("--input".to_string());
        out.push(f.display().to_string());
    }
    out.push("--top".to_string());
    out.push(args.top.clone());
    for c in &args.cflags {
        out.push("--cflags".to_string());
        out.push(c.clone());
    }
    if args.flatten_hierarchy {
        out.push("--flatten-hierarchy".to_string());
    } else if args.keep_hierarchy {
        out.push("--keep-hierarchy".to_string());
    }
    out.push("--target".to_string());
    out.push(args.target.clone());
    out
}

pub fn run(args: &AnalyzeArgs, ctx: &mut CliContext) -> Result<()> {
    python_bridge::require_enabled("analyze")?;
    python_bridge::run("analyze", &to_python_argv(args), ctx)
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::similar_names,
        reason = "the `args`/`argv` pair appears throughout the dispatcher; \
                  matching the production names keeps tests legible"
    )]

    use super::*;

    #[test]
    fn argv_round_trips_python_shape() {
        let args = AnalyzeArgs::try_parse_from([
            "analyze",
            "--input",
            "vadd.cpp",
            "--top",
            "VecAdd",
            "--target",
            "xilinx-hls",
        ])
        .unwrap();
        let argv = to_python_argv(&args);
        assert!(argv.contains(&"--input".to_string()));
        assert!(argv.contains(&"vadd.cpp".to_string()));
        assert!(argv.contains(&"--top".to_string()));
        assert!(argv.contains(&"VecAdd".to_string()));
        assert!(argv.contains(&"--target".to_string()));
        assert!(argv.contains(&"xilinx-hls".to_string()));
    }
}
