//! Hardware or simulation linking of a K5 package through SLASH.

use crate::context::CliContext;
use crate::error::{CliError, Result};
use clap::Parser;
use std::path::PathBuf;
use tapa_xilinx::target_frequency_mhz;
use tapa_xilinx::tools::slash::{SlashLinkJob, SlashLinkMode};

#[derive(Debug, Parser)]
pub struct LinkArgs {
    /// Packaged TAPA kernel in the K5 SLASH ZIP layout.
    #[arg(long)]
    pub package: PathBuf,
    /// SLASH service-shell connectivity configuration.
    #[arg(long)]
    pub connectivity: PathBuf,
    /// Hardware or simulation VBIN output.
    #[arg(short, long)]
    pub output: PathBuf,
    /// SLASH executable on the tool host.
    #[arg(long, default_value = "slashkit")]
    pub slashkit: String,
    /// Vivado executable on the tool host.
    #[arg(long, default_value = "vivado")]
    pub vivado: String,
    /// Requested clock in MHz; defaults to the synthesized flow clock.
    #[arg(long)]
    pub clock_mhz: Option<u32>,
    #[arg(short, long, default_value_t = 8)]
    pub jobs: u32,
    /// TAPA floorplan constraints to apply during implementation.
    #[arg(long)]
    pub floorplan_xdc: Option<PathBuf>,
    /// Produce a SLASH simulation vbin instead of a hardware image.
    #[arg(long)]
    pub sim: bool,
}

pub fn run(args: &LinkArgs, ctx: &CliContext) -> Result<()> {
    let target_mhz = if let Some(target) = args.clock_mhz {
        target
    } else {
        let state = crate::state::work::load(&ctx.work_dir)?;
        let period = state.flow.clock_period.ok_or_else(|| {
            CliError::InvalidArg(
                "link requires `--clock-mhz` or a synthesized flow clock period".into(),
            )
        })?;
        target_frequency_mhz(period.nanoseconds())
            .map_err(|error| CliError::InvalidArg(error.to_string()))?
    };
    let job = SlashLinkJob {
        package: crate::util::utf8(&args.package),
        config: crate::util::utf8(&args.connectivity),
        output: crate::util::utf8(&args.output),
        work_dir: crate::util::utf8(ctx.work_dir.join("link")),
        slashkit: args.slashkit.clone(),
        vivado: args.vivado.clone(),
        jobs: args.jobs,
        target_mhz,
        floorplan_xdc: args.floorplan_xdc.as_ref().map(crate::util::utf8),
        mode: if args.sim {
            SlashLinkMode::Simulation
        } else {
            SlashLinkMode::Hardware
        },
    };
    let output = ctx.with_tool_runner(|runner| tapa_xilinx::tools::slash::link(runner, &job))?;
    log::info!(
        "SLASH link produced {} at {:.6} MHz (requested {} MHz)",
        output.bitstream,
        output.recorded_mhz,
        output.requested_mhz,
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_k5_package_clock_override_and_sim_mode() {
        let args = LinkArgs::try_parse_from([
            "link",
            "--package",
            "vadd.zip",
            "--connectivity",
            "config.cfg",
            "-o",
            "vadd.sim.vbin",
            "--clock-mhz",
            "300",
            "--sim",
        ])
        .expect("parse link args");
        assert_eq!(args.package, PathBuf::from("vadd.zip"));
        assert_eq!(args.clock_mhz, Some(300));
        assert!(args.sim);
    }
}
