//! `tapa g++` — invokes `g++` with TAPA include and link flags.

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

pub fn run(args: &GccArgs, _ctx: &CliContext) -> Result<()> {
    let mut cmd = Command::new(&args.executable);
    cmd.arg("-std=c++17");
    cmd.arg("-DHLS_NO_XIL_FPO_LIB");
    cmd.args(get_tapa_cflags());

    for env_name in ["XILINX_HLS", "XILINX_VITIS"] {
        if let Some(root) = std::env::var_os(env_name) {
            let include = PathBuf::from(root).join("include");
            if include.exists() {
                cmd.arg(format!("-isystem{}", include.display()));
            }
            break;
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
