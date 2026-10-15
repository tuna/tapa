//! Hardware linking of a packaged TAPA XO through SLASH.

use crate::context::CliContext;
use crate::error::Result;
use clap::Parser;
use std::path::PathBuf;
use tapa_xilinx::tools::slash::SlashLinkJob;

#[derive(Debug, Parser)]
pub struct LinkArgs {
    /// Packaged TAPA kernel.
    #[arg(long)]
    pub xo: PathBuf,
    /// SLASH service-shell connectivity configuration.
    #[arg(long)]
    pub connectivity: PathBuf,
    /// Hardware VBIN output.
    #[arg(short, long)]
    pub output: PathBuf,
    /// SLASH executable on the tool host.
    #[arg(long, default_value = "slashkit")]
    pub slashkit: String,
    /// Vivado executable on the tool host.
    #[arg(long, default_value = "vivado")]
    pub vivado: String,
    /// Requested clock in Hz; a derated image fails validation.
    #[arg(long)]
    pub clock_hz: u64,
    #[arg(short, long, default_value_t = 8)]
    pub jobs: u32,
    /// TAPA floorplan constraints to apply during implementation.
    #[arg(long)]
    pub floorplan_xdc: Option<PathBuf>,
}

pub fn run(args: &LinkArgs, ctx: &CliContext) -> Result<()> {
    let job = SlashLinkJob {
        xo: crate::util::utf8(&args.xo),
        config: crate::util::utf8(&args.connectivity),
        output: crate::util::utf8(&args.output),
        work_dir: crate::util::utf8(ctx.work_dir.join("link")),
        slashkit: args.slashkit.clone(),
        vivado: args.vivado.clone(),
        jobs: args.jobs,
        clock_hz: args.clock_hz,
        floorplan_xdc: args.floorplan_xdc.as_ref().map(crate::util::utf8),
    };
    ctx.with_tool_runner(|runner| tapa_xilinx::tools::slash::link(runner, &job))?;
    Ok(())
}
