//! `tapa g++` — invokes `g++` with TAPA include / link flags. Mirrors
//! `tapa/steps/gcc.py`.

use std::path::PathBuf;
use std::process::Command;

use clap::Parser;

use crate::context::CliContext;
use crate::error::Result;
use crate::tapacc::cflags::get_tapa_cflags;

#[derive(Debug, Parser)]
#[command(
    name = "g++",
    about = "Invoke g++ with TAPA include and library paths."
)]
pub struct Args {
    /// Run the specified executable instead of `g++`.
    #[arg(long = "executable", default_value = "g++")]
    pub executable: PathBuf,

    /// Pass-through arguments forwarded to `g++` verbatim.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub argv: Vec<String>,
}

pub fn run(args: &Args, _ctx: &mut CliContext) -> Result<()> {
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
    cmd.args(get_tapa_ldflags_passthrough());

    let status = cmd.status()?;
    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(())
}

/// Mirrors `get_tapa_ldflags()` from Python; bridge to the same helper
/// once the Rust port lands. For now we delegate to the Python side
/// transparently by emitting the same `-l` flags it does.
fn get_tapa_ldflags_passthrough() -> Vec<String> {
    let libs = [
        "tapa", "frt_cpp", "context", "thread", "frt", "asio", "filesystem", "glog",
        "gflags", "OpenCL", "minizip_ng", "tinyxml2", "z", "yaml-cpp", "stdc++fs",
    ];
    libs.iter().map(|l| format!("-l{l}")).collect()
}
