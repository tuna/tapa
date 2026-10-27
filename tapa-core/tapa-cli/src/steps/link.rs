//! Hardware or simulation linking of a packaged TAPA kernel.

use crate::context::CliContext;
use crate::error::{CliError, Result};
use clap::Parser;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use tapa_ir::{Target, WorkState};
use tapa_xilinx::tools::slash::{SlashLinkJob, SlashLinkMode};
use tapa_xilinx::{
    run_vitis_link, target_frequency_mhz, LinkOutput, ToolRunner, VitisLinkJob, VitisLinkMode,
};

#[derive(Debug, Parser)]
pub struct LinkArgs {
    /// Packaged TAPA kernel (`.xo` for Vitis, `.zip` for SLASH).
    #[arg(long)]
    pub package: PathBuf,
    /// Target-specific connectivity configuration (`--config` for Vitis).
    #[arg(long)]
    pub connectivity: Option<PathBuf>,
    /// Hardware bitstream (`.xclbin`) or SLASH hardware/simulation VBIN output.
    #[arg(short, long)]
    pub output: PathBuf,
    /// SLASH executable on the tool host.
    #[arg(long, default_value = "slashkit")]
    pub slashkit: String,
    /// Vivado executable on the tool host.
    #[arg(long, default_value = "vivado")]
    pub vivado: String,
    /// Vitis linker/packager executable on the tool host.
    #[arg(long, default_value = "v++")]
    pub vpp: String,
    /// Requested clock in MHz; defaults to the packaged flow's synthesized clock.
    #[arg(long)]
    pub clock_mhz: Option<u32>,
    #[arg(short, long, default_value_t = 8, value_parser = clap::value_parser!(u32).range(1..))]
    pub jobs: u32,
    /// TAPA floorplan constraints to apply during implementation.
    #[arg(long)]
    pub floorplan_xdc: Option<PathBuf>,
    /// Produce a SLASH simulation vbin instead of a hardware image.
    #[arg(long)]
    pub sim: bool,
}

#[derive(Debug)]
enum LinkPlan {
    Vitis(VitisLinkJob),
    Slash(SlashLinkJob),
}

pub fn run(args: &LinkArgs, ctx: &CliContext) -> Result<()> {
    let state = read_package_state(&args.package)?;
    let plan = plan_link(args, &ctx.work_dir, &state)?;
    let output = ctx.with_tool_runner(|runner| run_plan(runner, &plan))?;
    let backend = match plan {
        LinkPlan::Vitis(_) => "Vitis",
        LinkPlan::Slash(_) => "SLASH",
    };
    log::info!(
        "{backend} link produced {} at {:.6} MHz (requested {} MHz)",
        output.bitstream,
        output.recorded_mhz,
        output.requested_mhz,
    );
    Ok(())
}

fn run_plan(runner: &dyn ToolRunner, plan: &LinkPlan) -> Result<LinkOutput> {
    match plan {
        LinkPlan::Vitis(job) => Ok(run_vitis_link(runner, job)?),
        LinkPlan::Slash(job) => Ok(tapa_xilinx::tools::slash::link(runner, job)?),
    }
}

fn plan_link(args: &LinkArgs, work_dir: &Path, state: &WorkState) -> Result<LinkPlan> {
    let target_mhz = match args.clock_mhz {
        Some(0) => {
            return Err(CliError::InvalidArg(
                "link clock must be greater than zero".into(),
            ));
        }
        Some(target) => target,
        None => {
            let period = state.flow.clock_period.ok_or_else(|| {
                CliError::InvalidArg(
                    "link requires `--clock-mhz` or a packaged synthesized flow clock period"
                        .into(),
                )
            })?;
            target_frequency_mhz(period.nanoseconds())
                .map_err(|error| CliError::InvalidArg(error.to_string()))?
        }
    };
    let link_work = crate::util::utf8(work_dir.join("link"));
    match state.graph.target {
        Target::XilinxVitis => {
            if args.sim {
                return Err(CliError::InvalidArg(
                    "`--sim` is available only for the `xilinx-slash` target".into(),
                ));
            }
            require_extension(&args.package, "xo", "Vitis package")?;
            require_extension(&args.output, "xclbin", "Vitis link output")?;
            let platform = state.flow.platform.clone().ok_or_else(|| {
                CliError::InvalidArg(
                    "Vitis link requires a platform in the packaged flow state; rerun `tapa synth \
                     --platform`"
                        .into(),
                )
            })?;
            let artifacts = link_work.join("artifacts");
            Ok(LinkPlan::Vitis(
                VitisLinkJob::builder()
                    .kernel_name(state.graph.top.clone())
                    .xo(crate::util::utf8(&args.package))
                    .platform(platform)
                    .target_mhz(target_mhz)
                    .work_dir(link_work.clone())
                    .artifacts_dir(artifacts.clone())
                    .output_xclbin(crate::util::utf8(&args.output))
                    .report_dir(artifacts.join("reports"))
                    .log_dir(artifacts.join("logs"))
                    .temp_dir(link_work.join("vitis.tmp"))
                    .vivado_threads(args.jobs)
                    .mode(VitisLinkMode::Full)
                    .vpp(args.vpp.clone())
                    .floorplan_xdc(args.floorplan_xdc.as_ref().map(crate::util::utf8))
                    .connectivity_config(args.connectivity.as_ref().map(crate::util::utf8))
                    .build(),
            ))
        }
        Target::XilinxSlash => {
            require_extension(&args.package, "zip", "SLASH package")?;
            require_extension(&args.output, "vbin", "SLASH link output")?;
            let connectivity = args.connectivity.as_ref().ok_or_else(|| {
                CliError::InvalidArg("SLASH link requires `--connectivity <config.cfg>`".into())
            })?;
            Ok(LinkPlan::Slash(SlashLinkJob {
                package: crate::util::utf8(&args.package),
                config: crate::util::utf8(connectivity),
                output: crate::util::utf8(&args.output),
                work_dir: link_work,
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
            }))
        }
        Target::XilinxHls => Err(CliError::InvalidArg(
            "`tapa link` requires a package persisted for `xilinx-vitis` or `xilinx-slash`; \
             `xilinx-hls` has no link backend"
                .into(),
        )),
    }
}

fn require_extension(path: &Path, expected: &str, label: &str) -> Result<()> {
    if path.extension().and_then(std::ffi::OsStr::to_str) == Some(expected) {
        Ok(())
    } else {
        Err(CliError::InvalidArg(format!(
            "{label} `{}` must use the `.{expected}` extension",
            path.display()
        )))
    }
}

fn read_package_state(package: &Path) -> Result<WorkState> {
    let file = fs_err::File::open(package)?;
    let mut archive = zip::ZipArchive::new(file).map_err(|error| {
        CliError::Archive(format!(
            "cannot open link package `{}` as a ZIP archive: {error}",
            package.display()
        ))
    })?;
    let mut entry = archive
        .by_name(crate::state::work::FILE_NAME)
        .map_err(|error| {
            CliError::Archive(format!(
                "link package `{}` has no `{}` entry with its typed flow target: {error}",
                package.display(),
                crate::state::work::FILE_NAME
            ))
        })?;
    let mut text = String::new();
    entry.read_to_string(&mut text)?;
    WorkState::from_json(&text).map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;
    use tapa_ir::ClockPeriod;

    fn args(package: &str, output: &str) -> LinkArgs {
        LinkArgs::try_parse_from([
            "link",
            "--package",
            package,
            "--connectivity",
            "config.cfg",
            "-o",
            output,
            "--clock-mhz",
            "300",
            "--vpp",
            "vpp-custom",
            "--slashkit",
            "slash-custom",
            "--vivado",
            "vivado-custom",
            "--jobs",
            "3",
        ])
        .expect("parse link args")
    }

    fn state(target: Target) -> WorkState {
        let mut state = crate::testutil::state_from_json(&format!(
            r#"{{
                "schema_version": 3,
                "cflags": [], "top": "Top", "target": "{}",
                "tasks": {{"Top": {{"readable_name":"Top", "srcs": ["Top.cpp"], "include_dirs": [], "defines": [], "level":"lower",
                    "synth":"hls", "ports":[], "tasks":{{}}, "fifos":{{}}}}}}
            }}"#,
            match target {
                Target::XilinxVitis => "xilinx-vitis",
                Target::XilinxSlash => "xilinx-slash",
                Target::XilinxHls => "xilinx-hls",
            }
        ));
        state.flow.platform = Some("platform_from_package".into());
        state.flow.clock_period = Some(ClockPeriod::from_picoseconds(3330));
        state
    }

    #[test]
    fn parses_vitis_and_slash_tool_flags() {
        let parsed = args("Top.xo", "Top.xclbin");
        assert_eq!(parsed.package, PathBuf::from("Top.xo"));
        assert_eq!(parsed.clock_mhz, Some(300));
        assert_eq!(parsed.vpp, "vpp-custom");
        assert_eq!(parsed.slashkit, "slash-custom");
        assert_eq!(parsed.vivado, "vivado-custom");
        assert_eq!(parsed.jobs, 3);
    }

    #[test]
    fn vitis_plan_uses_packaged_flow_metadata_and_full_mode() {
        let mut parsed = args("Top.xo", "Top.xclbin");
        parsed.clock_mhz = None;
        let plan = plan_link(&parsed, Path::new("/work"), &state(Target::XilinxVitis))
            .expect("Vitis plan");
        let LinkPlan::Vitis(job) = plan else {
            panic!("expected Vitis plan");
        };
        assert_eq!(job.mode, VitisLinkMode::Full);
        assert_eq!(job.platform, "platform_from_package");
        assert_eq!(job.kernel_name, "Top");
        assert_eq!(job.target_mhz, 300);
        assert_eq!(job.vpp, "vpp-custom");
        assert_eq!(job.vivado_threads, 3);
    }

    #[test]
    fn slash_plan_preserves_sim_and_rejects_it_for_vitis() {
        let mut parsed = args("Top.zip", "Top.sim.vbin");
        parsed.sim = true;
        let plan = plan_link(&parsed, Path::new("/work"), &state(Target::XilinxSlash))
            .expect("SLASH plan");
        let LinkPlan::Slash(job) = plan else {
            panic!("expected SLASH plan");
        };
        assert_eq!(job.mode, SlashLinkMode::Simulation);
        assert_eq!(job.slashkit, "slash-custom");
        assert_eq!(job.vivado, "vivado-custom");

        let parsed = args("Top.xo", "Top.xclbin");
        let error = plan_link(&parsed, Path::new("/work"), &state(Target::XilinxSlash))
            .expect_err("ZIP is required");
        assert!(error.to_string().contains(".zip"));

        let mut parsed = args("Top.xo", "Top.xclbin");
        parsed.sim = true;
        let error = plan_link(&parsed, Path::new("/work"), &state(Target::XilinxVitis))
            .expect_err("Vitis sim must fail");
        assert!(error.to_string().contains("only"));
    }

    #[test]
    fn target_is_read_from_the_package_not_the_work_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        let package = dir.path().join("Top.xo");
        let file = fs_err::File::create(&package).expect("create package");
        let mut zip = zip::ZipWriter::new(file);
        zip.start_file(
            crate::state::work::FILE_NAME,
            zip::write::FileOptions::<'_, '_, ()>::default(),
        )
        .expect("state entry");
        zip.write_all(&crate::state::work::to_bytes(&state(Target::XilinxVitis)).expect("state"))
            .expect("write state");
        zip.finish().expect("finish package");

        let packed = read_package_state(&package).expect("read package state");
        assert_eq!(packed.graph.target, Target::XilinxVitis);
        assert!(!dir.path().join("tapa.json").exists());
    }
}
