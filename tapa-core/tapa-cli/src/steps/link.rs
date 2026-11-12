//! Hardware or simulation linking of a packaged TAPA kernel: an `.xo`, or
//! the `.zip` of a top on the AI Engine, for Vitis; a `.zip` for SLASH.

use crate::context::CliContext;
use crate::error::{CliError, Result};
use clap::Parser;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use tapa_ir::{Target, WorkState};
use tapa_xilinx::tools::aie::link::AieLinkJob;
use tapa_xilinx::tools::slash::{SlashLinkJob, SlashLinkMode};
use tapa_xilinx::{
    run_vitis_link, target_frequency_mhz, LinkOutput, ToolRunner, VitisLinkJob, VitisLinkMode,
};

#[derive(Debug, Parser)]
pub struct LinkArgs {
    /// Packaged TAPA kernel: `.xo` for Vitis, or `.zip` when the top runs on
    /// the AI Engine; `.zip` for SLASH.
    #[arg(long)]
    pub package: PathBuf,
    /// Target-specific connectivity configuration (`--config` for Vitis).
    #[arg(long)]
    pub connectivity: Option<PathBuf>,
    /// Hardware bitstream (`.xclbin`) or SLASH hardware/simulation VBIN output.
    #[arg(short, long)]
    pub output: PathBuf,
    #[command(flatten)]
    pub programs: ToolPrograms,
    /// Requested clock in MHz; defaults to the packaged flow's synthesized clock.
    #[arg(long)]
    pub clock_mhz: Option<u32>,
    #[arg(short, long, default_value_t = 8, value_parser = clap::value_parser!(u32).range(1..))]
    pub jobs: u32,
    /// TAPA floorplan constraints to apply during implementation.
    #[arg(long)]
    pub floorplan_xdc: Option<PathBuf>,
    /// Produce a simulation image instead of a hardware one: a SLASH
    /// simulation vbin, or the Vitis hardware emulation (`hw_emu`) xclbin of
    /// a top on the AI Engine.
    #[arg(long)]
    pub sim: bool,
}

/// The tool-host programs a link runs. Each belongs to one backend, and
/// naming one for the other backend is an error rather than a flag that is
/// silently ignored.
#[derive(Debug, Clone, Default, clap::Args)]
pub struct ToolPrograms {
    /// SLASH linker on the tool host [default: slashkit]; `xilinx-slash` only.
    #[arg(long)]
    pub slashkit: Option<String>,
    /// Vivado on the tool host [default: vivado]; `xilinx-slash` only.
    #[arg(long)]
    pub vivado: Option<String>,
    /// Vitis linker/packager on the tool host [default: v++]; `xilinx-vitis` only.
    #[arg(long)]
    pub vpp: Option<String>,
}

impl ToolPrograms {
    /// The `v++` of a Vitis link; SLASH's programs are rejected.
    pub(crate) fn vitis(&self) -> Result<String> {
        for (flag, value) in [("--slashkit", &self.slashkit), ("--vivado", &self.vivado)] {
            if value.is_some() {
                return Err(wrong_target(flag, Target::XilinxSlash, Target::XilinxVitis));
            }
        }
        Ok(self.vpp.clone().unwrap_or_else(|| "v++".to_string()))
    }

    /// The `slashkit` and `vivado` of a SLASH link; `--vpp` is rejected.
    pub(crate) fn slash(&self) -> Result<(String, String)> {
        if self.vpp.is_some() {
            return Err(wrong_target(
                "--vpp",
                Target::XilinxVitis,
                Target::XilinxSlash,
            ));
        }
        Ok((
            self.slashkit
                .clone()
                .unwrap_or_else(|| "slashkit".to_string()),
            self.vivado.clone().unwrap_or_else(|| "vivado".to_string()),
        ))
    }
}

fn wrong_target(flag: &str, applies_to: Target, target: Target) -> CliError {
    CliError::InvalidArg(format!(
        "`{flag}` names a `{}` program, but this design targets `{}`",
        applies_to.as_str(),
        target.as_str(),
    ))
}

#[derive(Debug)]
enum LinkPlan {
    Vitis(VitisLinkJob),
    /// A top on the AI Engine: the graph alone, from the package's entries.
    Aie(AieLinkJob),
    Slash(SlashLinkJob),
}

pub fn run(args: &LinkArgs, ctx: &CliContext) -> Result<()> {
    let state = read_package_state(&args.package)?;
    let plan = plan_link(args, &ctx.work_dir, &state)?;
    if let LinkPlan::Aie(job) = &plan {
        extract_aie_package(&args.package, job)?;
    }
    let output = ctx.with_tool_runner(|runner| run_plan(runner, &plan))?;
    // Each backend records the clock its bitstream runs at and warns when it
    // missed the request; the result line reports both with the routed timing.
    match plan {
        LinkPlan::Vitis(_) => log::info!("{}", link_summary("Vitis", &output)),
        LinkPlan::Aie(job) => log::info!(
            "AI Engine link produced {}, with no PL kernel, and {}",
            output.bitstream,
            tapa_xilinx::tools::aie::link::xsa(&job.output_xclbin)
        ),
        LinkPlan::Slash(_) => log::info!("{}", link_summary("SLASH", &output)),
    }
    Ok(())
}

/// The result line of a link: the clock the bitstream records, the request,
/// and the routed kernel timing whenever the link produced one.
fn link_summary(backend: &str, output: &LinkOutput) -> String {
    use std::fmt::Write as _;
    let mut line = format!(
        "{backend} link produced {} at {:.6} MHz (requested {} MHz)",
        output.bitstream, output.recorded_mhz, output.requested_mhz,
    );
    if let Some(timing) = &output.timing {
        write!(
            line,
            "; routed kernel WNS {:.3} ns, Fmax {:.3} MHz",
            timing.wns_ns, timing.fmax_mhz
        )
        .expect("write String");
    }
    line
}

fn run_plan(runner: &dyn ToolRunner, plan: &LinkPlan) -> Result<LinkOutput> {
    match plan {
        LinkPlan::Vitis(job) => Ok(run_vitis_link(runner, job)?),
        LinkPlan::Aie(job) => Ok(tapa_xilinx::tools::aie::link::link(runner, job)?),
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
            if args.package.extension().is_some_and(|ext| ext == "zip") {
                return plan_aie_link(args, link_work, state);
            }
            if args.sim {
                return Err(CliError::InvalidArg(
                    "`--sim` is available for the `xilinx-slash` target, and for a Vitis package \
                     whose top runs on the AI Engine"
                        .into(),
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
            // A platform installed elsewhere, as for a remote runner, is
            // left to v++ to judge.
            let versal = platform_is_versal(&platform)?;
            Ok(LinkPlan::Vitis(
                VitisLinkJob::builder()
                    .kernel_name(state.graph.top.clone())
                    .xo(crate::util::utf8(&args.package))
                    .platform(platform)
                    .target_mhz(target_mhz)
                    .work_dir(link_work.clone())
                    .artifacts_dir(artifacts.clone())
                    .output_xclbin(crate::util::utf8(&args.output))
                    .versal(versal)
                    .report_dir(artifacts.join("reports"))
                    .log_dir(artifacts.join("logs"))
                    .temp_dir(link_work.join("vitis.tmp"))
                    .vivado_threads(args.jobs)
                    .mode(VitisLinkMode::Full)
                    .vpp(args.programs.vitis()?)
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
            let (slashkit, vivado) = args.programs.slash()?;
            Ok(LinkPlan::Slash(SlashLinkJob {
                platform: slash_platform(state)?,
                package: crate::util::utf8(&args.package),
                config: crate::util::utf8(connectivity),
                output: crate::util::utf8(&args.output),
                work_dir: link_work,
                slashkit,
                vivado,
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

/// The link of a top on the AI Engine, whose package is a `.zip`.
fn plan_aie_link(
    args: &LinkArgs,
    work: camino::Utf8PathBuf,
    state: &WorkState,
) -> Result<LinkPlan> {
    let top_frames = crate::aie::top_frames(state.flow.platform.as_deref());
    let pl_less = tapa_ir::AiePartition::derive(&state.graph, top_frames)?
        .is_some_and(|p| p.pl_less && !p.has_rings());
    if !pl_less {
        return Err(CliError::InvalidArg(format!(
            "`{}` is a Vitis package of a top that is not on the AI Engine; link its `.xo`",
            args.package.display()
        )));
    }
    require_extension(&args.output, "xclbin", "Vitis link output")?;
    let platform = state.flow.platform.clone().ok_or_else(|| {
        CliError::InvalidArg(
            "Vitis link requires a platform in the packaged flow state; rerun `tapa synth \
             --platform`"
                .into(),
        )
    })?;
    Ok(LinkPlan::Aie(
        AieLinkJob::builder()
            .platform(platform)
            .emulation(args.sim)
            .libadf(work.join(crate::aie::PACKAGE_LIBADF))
            .arguments(work.join(crate::aie::PACKAGE_ARGUMENTS))
            .work_dir(work)
            .output_xclbin(crate::util::utf8(&args.output))
            .vpp(args.programs.vitis()?)
            .build(),
    ))
}

/// Writes the graph and the argument list of a top on the AI Engine out of
/// its package, where the link reads them.
fn extract_aie_package(package: &Path, job: &AieLinkJob) -> Result<()> {
    let file = fs_err::File::open(package)?;
    let mut archive = zip::ZipArchive::new(file).map_err(|error| {
        CliError::Archive(format!(
            "cannot open link package `{}` as a ZIP archive: {error}",
            package.display()
        ))
    })?;
    fs_err::create_dir_all(&job.work_dir)?;
    for (name, path) in [
        (crate::aie::PACKAGE_LIBADF, &job.libadf),
        (crate::aie::PACKAGE_ARGUMENTS, &job.arguments),
    ] {
        let mut entry = archive.by_name(name).map_err(|error| {
            CliError::Archive(format!(
                "link package `{}` has no `{name}`: {error}",
                package.display()
            ))
        })?;
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes)?;
        fs_err::write(path, bytes)?;
    }
    Ok(())
}

/// Whether `platform` is a Versal device, whose `v++ --link` makes an `.xsa`
/// that `v++ --package` turns into the xclbin. A platform not installed here,
/// as for a remote tool host, is left to v++ to judge.
pub(crate) fn platform_is_versal(platform: &str) -> Result<bool> {
    Ok(crate::steps::synth::resolve_platform_dir(platform)
        .map(|dir| tapa_xilinx::platform_is_versal(&dir))
        .transpose()?
        .unwrap_or(false))
}

pub(crate) fn slash_platform(state: &WorkState) -> Result<&'static tapa_xilinx::SlashPlatform> {
    let platform = state
        .flow
        .platform
        .as_deref()
        .and_then(tapa_xilinx::find_slash_platform)
        .ok_or_else(|| {
            CliError::InvalidArg(
                "SLASH requires a known shell platform; rerun `tapa synth --platform`".into(),
            )
        })?;
    if state.flow.part_num.as_deref() != Some(platform.part_num) {
        return Err(CliError::InvalidArg(format!(
            "SLASH platform {} requires part {}",
            platform.name, platform.part_num
        )));
    }
    Ok(platform)
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
        args_with(package, output, &[])
    }

    fn args_with(package: &str, output: &str, programs: &[&str]) -> LinkArgs {
        let mut argv = vec![
            "link",
            "--package",
            package,
            "--connectivity",
            "config.cfg",
            "-o",
            output,
            "--clock-mhz",
            "300",
            "--jobs",
            "3",
        ];
        argv.extend_from_slice(programs);
        LinkArgs::try_parse_from(argv).expect("parse link args")
    }

    const VITIS_PROGRAMS: &[&str] = &["--vpp", "vpp-custom"];
    const SLASH_PROGRAMS: &[&str] = &["--slashkit", "slash-custom", "--vivado", "vivado-custom"];

    fn state(target: Target) -> WorkState {
        let mut state = crate::testutil::state_from_json(&format!(
            r#"{{
                "schema_version": 4,
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
        if target == Target::XilinxSlash {
            state.flow.platform = Some("slash_v80_service".into());
            state.flow.part_num = Some("xcv80-lsva4737-2MHP-e-S".into());
        } else {
            state.flow.platform = Some("platform_from_package".into());
        }
        state.flow.clock_period = Some(ClockPeriod::from_picoseconds(3330));
        state
    }

    #[test]
    fn parses_vitis_and_slash_tool_flags() {
        let parsed = args_with(
            "Top.xo",
            "Top.xclbin",
            &[VITIS_PROGRAMS, SLASH_PROGRAMS].concat(),
        );
        assert_eq!(parsed.package, PathBuf::from("Top.xo"));
        assert_eq!(parsed.clock_mhz, Some(300));
        assert_eq!(parsed.programs.vpp.as_deref(), Some("vpp-custom"));
        assert_eq!(parsed.programs.slashkit.as_deref(), Some("slash-custom"));
        assert_eq!(parsed.programs.vivado.as_deref(), Some("vivado-custom"));
        assert_eq!(parsed.jobs, 3);
    }

    /// v80.md: a mismatch between the program flags and the target is an
    /// error in either direction, never a silently ignored flag.
    #[test]
    fn a_program_flag_for_the_other_backend_is_an_error() {
        let error = plan_link(
            &args_with("Top.xo", "Top.xclbin", &["--slashkit", "slash-custom"]),
            Path::new("/work"),
            &state(Target::XilinxVitis),
        )
        .expect_err("--slashkit on a Vitis link");
        assert!(error.to_string().contains("`--slashkit`"), "{error}");
        let error = plan_link(
            &args_with("Top.zip", "Top.vbin", VITIS_PROGRAMS),
            Path::new("/work"),
            &state(Target::XilinxSlash),
        )
        .expect_err("--vpp on a SLASH link");
        assert!(error.to_string().contains("`--vpp`"), "{error}");

        let LinkPlan::Vitis(job) = plan_link(
            &args("Top.xo", "Top.xclbin"),
            Path::new("/work"),
            &state(Target::XilinxVitis),
        )
        .expect("defaults") else {
            panic!("expected Vitis plan");
        };
        assert_eq!(job.vpp, "v++");
    }

    #[test]
    fn slash_profiles_agree_with_floorplan_interfaces_and_reach_link_jobs() {
        for platform in tapa_xilinx::SLASH_PLATFORMS {
            let device = tapa_floorplan::device::select_device_with_shell(
                platform.part_num,
                Some(platform.shell),
            )
            .expect("table");
            for (kind, count) in [("HBM", platform.hbm_ports), ("DDR", platform.ddr_ports)] {
                for index in 0..count {
                    assert_eq!(
                        device.slots_with_tag(&format!("{kind}[{index}]")).count(),
                        1
                    );
                }
                assert_eq!(
                    device.slots_with_tag(&format!("{kind}[{count}]")).count(),
                    0
                );
            }
            let mut state = state(Target::XilinxSlash);
            state.flow.platform = Some(platform.name.into());
            let plan = plan_link(&args("Top.zip", "Top.vbin"), Path::new("/work"), &state)
                .expect("link plan");
            let LinkPlan::Slash(job) = plan else {
                panic!("SLASH job")
            };
            assert_eq!(job.platform, platform);
        }
    }

    #[test]
    fn vitis_plan_uses_packaged_flow_metadata_and_full_mode() {
        let mut parsed = args_with("Top.xo", "Top.xclbin", VITIS_PROGRAMS);
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
        let mut parsed = args_with("Top.zip", "Top.sim.vbin", SLASH_PROGRAMS);
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
        assert!(error.to_string().contains("top runs on the AI Engine"));
    }

    /// The work state of a top on the AI Engine, which is all it runs.
    fn aie_top_state() -> WorkState {
        let mut state = crate::testutil::state_from_json(
            r#"{
                "schema_version": 4, "cflags": [], "top": "Top", "target": "xilinx-vitis",
                "tasks": {
                    "Top": {"readable_name": "Top", "srcs": ["Top.cpp"], "include_dirs": [],
                        "defines": [], "level": "upper", "synth": "aie",
                        "ports": [{"cat": "istream", "name": "src", "type": "int", "width": 32}],
                        "tasks": {"Sink": [{"args": {"in": {"arg": "src", "cat": "istream"}}}]},
                        "fifos": {}},
                    "Sink": {"readable_name": "Sink", "srcs": ["Top.cpp"], "include_dirs": [],
                        "defines": [], "level": "lower", "synth": "aie",
                        "ports": [{"cat": "istream", "name": "in", "type": "int", "width": 32,
                            "frame": 16}]}}
            }"#,
        );
        state.flow.platform = Some("xilinx_vck190_base_202520_1".into());
        state.flow.clock_period = Some(ClockPeriod::from_picoseconds(3330));
        state
    }

    #[test]
    fn a_top_on_the_ai_engine_links_its_zip_and_simulates_as_hw_emu() {
        let mut parsed = args("Top.zip", "Top.xclbin");
        parsed.sim = true;
        let plan = plan_link(&parsed, Path::new("/work"), &aie_top_state()).expect("plan");
        let LinkPlan::Aie(job) = plan else {
            panic!("expected an AI Engine link");
        };
        assert!(job.emulation);
        assert_eq!(job.platform, "xilinx_vck190_base_202520_1");
        assert_eq!(job.libadf, "/work/link/libadf.a");
        assert_eq!(job.arguments, "/work/link/arguments.json");

        let error = plan_link(&parsed, Path::new("/work"), &state(Target::XilinxVitis))
            .expect_err("a PL top packs an .xo");
        assert!(error.to_string().contains("link its `.xo`"), "{error}");
    }

    /// The result line carries the clock the bitstream records, the request,
    /// and the routed kernel timing whenever the link produced one.
    #[test]
    fn link_summary_reports_the_recorded_clock_and_routed_timing() {
        let output = LinkOutput {
            bitstream: "Top.xclbin".into(),
            requested_mhz: 300,
            recorded_mhz: 260.9,
            timing: Some(tapa_xilinx::KernelTiming {
                reported_target_period_ns: 3.333,
                reported_target_mhz: 300.03,
                wns_ns: -0.5,
                achieved_period_ns: 3.833,
                fmax_mhz: 260.9,
            }),
            timing_report: Some("timing_summary.rpt".into()),
            stdout: String::new(),
            stderr: String::new(),
        };
        assert_eq!(
            link_summary("Vitis", &output),
            "Vitis link produced Top.xclbin at 260.900000 MHz (requested 300 MHz); \
             routed kernel WNS -0.500 ns, Fmax 260.900 MHz"
        );
        let simulation = LinkOutput {
            recorded_mhz: 300.0,
            timing: None,
            timing_report: None,
            ..output
        };
        assert_eq!(
            link_summary("SLASH", &simulation),
            "SLASH link produced Top.xclbin at 300.000000 MHz (requested 300 MHz)"
        );
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
