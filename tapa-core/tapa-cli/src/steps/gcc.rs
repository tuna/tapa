//! `tapa g++` — invokes `g++` with TAPA include / link flags. Mirrors
//! `tapa/steps/gcc.py`.

use std::path::PathBuf;
use std::process::Command;

use clap::Parser;

use crate::context::CliContext;
use crate::error::Result;
use crate::tapacc::cflags::{get_tapa_cflags, get_tapa_ldflags};

#[derive(Debug, Parser)]
#[command(
    name = "g++",
    about = "Invoke g++ with TAPA include and library paths."
)]
pub struct GccArgs {
    /// Run the specified executable instead of `g++`.
    #[arg(long = "executable", default_value = "g++")]
    pub executable: PathBuf,

    /// Pass-through arguments forwarded to `g++` verbatim.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub argv: Vec<String>,
}

pub fn run(args: &GccArgs, _ctx: &mut CliContext) -> Result<()> {
    let mut cmd = Command::new(&args.executable);
    cmd.arg("-std=c++17");
    cmd.arg("-DHLS_NO_XIL_FPO_LIB");
    cmd.args(get_tapa_cflags());

    if let Some(xilinx_hls) = std::env::var_os("XILINX_HLS") {
        let include = PathBuf::from(xilinx_hls).join("include");
        if include.exists() {
            cmd.arg(format!("-isystem{}", include.display()));
        }
    }

    cmd.args(&args.argv);
    cmd.args(get_tapa_ldflags());

    let status = cmd.status()?;
    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(())
}
