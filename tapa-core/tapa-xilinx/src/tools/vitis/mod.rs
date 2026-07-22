//! Direct `v++ --link` orchestration and implementation timing extraction.

use camino::{Utf8Path, Utf8PathBuf};
use typed_builder::TypedBuilder;

use crate::error::{Result, XilinxError};
use crate::runtime::process::{ToolInvocation, ToolRunner};

pub mod timing;

use self::timing::{parse_kernel_timing_summary, KernelTiming};

const TIMING_REPORT_NAME: &str = "timing_summary.rpt";
const FINAL_TIMING_HOOK_NAME: &str = "final_timing.tcl";
const IMPLEMENTATION_STRATEGY: &str = "Explore";
const PLACEMENT_STRATEGY: &str = "EarlyBlockPlacement";

/// Convert a requested clock period to the conservative whole-MHz value that
/// `v++ --kernel_frequency` accepts.
pub fn target_frequency_mhz(clock_period_ns: f64) -> Result<u32> {
    if !clock_period_ns.is_finite() || clock_period_ns <= 0.0 {
        return Err(XilinxError::InvalidFrequency(format!(
            "clock period must be finite and positive, got `{clock_period_ns}` ns"
        )));
    }
    let floored = (1000.0 / clock_period_ns).floor();
    if !floored.is_finite() || !(1.0..=f64::from(u32::MAX)).contains(&floored) {
        return Err(XilinxError::InvalidFrequency(format!(
            "clock period `{clock_period_ns}` ns produces unsupported target `{floored}` MHz"
        )));
    }
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the floored value was range-checked against positive u32 above"
    )]
    Ok(floored as u32)
}

/// One portable Alveo hardware-link job.
///
/// `platform` must be an installed platform name. Platform paths are rejected
/// because they cannot be transferred safely to a remote tool host without
/// also resolving the files referenced by the platform.
#[derive(Debug, Clone, TypedBuilder)]
pub struct VitisLinkJob {
    pub kernel_name: String,
    pub xo: Utf8PathBuf,
    pub platform: String,
    /// Whole MHz passed to `v++ --kernel_frequency`; zero is rejected.
    pub target_mhz: u32,
    /// Per-job directory used as the tool working directory.
    pub work_dir: Utf8PathBuf,
    /// Compact directory downloaded by a remote runner. It contains the
    /// timing hook/report, Vitis reports, and Vitis logs.
    pub artifacts_dir: Utf8PathBuf,
    pub output_xclbin: Utf8PathBuf,
    pub report_dir: Utf8PathBuf,
    pub log_dir: Utf8PathBuf,
    /// Vitis implementation project. This path must be outside
    /// `artifacts_dir`, and is deliberately never downloaded.
    pub temp_dir: Utf8PathBuf,
    #[builder(default)]
    pub floorplan_xdc: Option<Utf8PathBuf>,
    #[builder(default)]
    pub connectivity_config: Option<Utf8PathBuf>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VitisLinkOutput {
    pub timing_report: Utf8PathBuf,
    pub timing: KernelTiming,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug)]
struct ResolvedJob<'a> {
    job: &'a VitisLinkJob,
    xo: Utf8PathBuf,
    work_dir: Utf8PathBuf,
    artifacts_dir: Utf8PathBuf,
    output_xclbin: Utf8PathBuf,
    report_dir: Utf8PathBuf,
    log_dir: Utf8PathBuf,
    temp_dir: Utf8PathBuf,
    floorplan_xdc: Option<Utf8PathBuf>,
    connectivity_config: Option<Utf8PathBuf>,
    timing_hook: Utf8PathBuf,
    timing_report: Utf8PathBuf,
}

impl<'a> ResolvedJob<'a> {
    fn new(job: &'a VitisLinkJob) -> Result<Self> {
        if job.kernel_name.trim().is_empty() {
            return link_error("kernel name must not be empty");
        }
        if job.platform.trim().is_empty() {
            return link_error("platform must not be empty");
        }
        if platform_looks_like_path(&job.platform) {
            return link_error(format!(
                "platform `{}` must be an installed platform name, not a path",
                job.platform
            ));
        }
        if job.target_mhz == 0 {
            return Err(XilinxError::InvalidFrequency(
                "target frequency must be positive, got `0` MHz".into(),
            ));
        }

        let xo = crate::util::absolutize(&job.xo);
        let work_dir = crate::util::absolutize(&job.work_dir);
        let artifacts_dir = crate::util::absolutize(&job.artifacts_dir);
        let output_xclbin = crate::util::absolutize(&job.output_xclbin);
        let report_dir = crate::util::absolutize(&job.report_dir);
        let log_dir = crate::util::absolutize(&job.log_dir);
        let temp_dir = crate::util::absolutize(&job.temp_dir);
        let floorplan_xdc = job.floorplan_xdc.as_deref().map(crate::util::absolutize);
        let connectivity_config = job
            .connectivity_config
            .as_deref()
            .map(crate::util::absolutize);

        ensure_under(
            &artifacts_dir,
            &work_dir,
            "artifacts directory",
            "work directory",
        )?;
        ensure_under(
            &output_xclbin,
            &artifacts_dir,
            "xclbin",
            "artifacts directory",
        )?;
        if output_xclbin.extension() != Some("xclbin") {
            return link_error(format!(
                "Alveo link output `{output_xclbin}` must use the `.xclbin` extension"
            ));
        }
        ensure_under(
            &report_dir,
            &artifacts_dir,
            "report directory",
            "artifacts directory",
        )?;
        ensure_under(
            &log_dir,
            &artifacts_dir,
            "log directory",
            "artifacts directory",
        )?;
        ensure_under(&temp_dir, &work_dir, "temp directory", "work directory")?;
        if temp_dir.starts_with(&artifacts_dir) {
            return link_error(
                "temp directory must be outside the downloadable artifacts directory",
            );
        }
        let output_parent = output_xclbin
            .parent()
            .ok_or_else(|| XilinxError::VitisLink("xclbin path has no parent directory".into()))?;
        let timing_hook = output_parent.join(FINAL_TIMING_HOOK_NAME);
        let timing_report = output_parent.join(TIMING_REPORT_NAME);

        Ok(Self {
            job,
            xo,
            work_dir,
            artifacts_dir,
            output_xclbin,
            report_dir,
            log_dir,
            temp_dir,
            floorplan_xdc,
            connectivity_config,
            timing_hook,
            timing_report,
        })
    }
}

fn platform_looks_like_path(platform: &str) -> bool {
    platform.contains(['/', '\\'])
        || Utf8Path::new(platform)
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("xpfm"))
}

fn ensure_under(path: &Utf8Path, root: &Utf8Path, item: &str, root_name: &str) -> Result<()> {
    if path != root && path.starts_with(root) {
        Ok(())
    } else {
        link_error(format!(
            "{item} `{path}` must be below {root_name} `{root}`"
        ))
    }
}

fn link_error<T>(detail: impl Into<String>) -> Result<T> {
    Err(XilinxError::VitisLink(detail.into()))
}

fn validate_input(path: &Utf8Path, label: &str) -> Result<()> {
    if path.is_file() {
        Ok(())
    } else {
        link_error(format!("{label} `{path}` is not a file"))
    }
}

fn final_timing_tcl() -> &'static str {
    "set artifact_dir [file dirname [file normalize [info script]]]\n\
report_timing_summary -file [file join $artifact_dir timing_summary.rpt]\n"
}

fn build_invocation(resolved: &ResolvedJob<'_>) -> ToolInvocation {
    let job = resolved.job;
    let mut inv = ToolInvocation::new("v++")
        .arg("--link")
        .arg("--target")
        .arg("hw")
        .arg("--save-temps")
        .arg("--to_step")
        .arg("vpl.impl.post_route_phys_opt_design")
        .arg("--platform")
        .arg(job.platform.clone())
        .arg("--output")
        .arg(resolved.output_xclbin.as_str())
        .arg("--report_dir")
        .arg(resolved.report_dir.as_str())
        .arg("--log_dir")
        .arg(resolved.log_dir.as_str())
        .arg("--temp_dir")
        .arg(resolved.temp_dir.as_str())
        .arg("--optimize")
        .arg("3")
        .arg("--kernel_frequency")
        .arg(job.target_mhz.to_string())
        .arg("--connectivity.nk")
        .arg(format!("{}:1:{}", job.kernel_name, job.kernel_name))
        .arg("--vivado.prop=run.impl_1.STEPS.PHYS_OPT_DESIGN.IS_ENABLED=1")
        .arg("--vivado.prop=run.impl_1.STEPS.POST_ROUTE_PHYS_OPT_DESIGN.IS_ENABLED=1")
        .arg(format!(
            "--vivado.prop=run.impl_1.STEPS.OPT_DESIGN.ARGS.DIRECTIVE={IMPLEMENTATION_STRATEGY}"
        ))
        .arg(format!(
            "--vivado.prop=run.impl_1.STEPS.PLACE_DESIGN.ARGS.DIRECTIVE={PLACEMENT_STRATEGY}"
        ))
        .arg(format!(
            "--vivado.prop=run.impl_1.STEPS.PHYS_OPT_DESIGN.ARGS.DIRECTIVE={IMPLEMENTATION_STRATEGY}"
        ))
        .arg(format!(
            "--vivado.prop=run.impl_1.STEPS.ROUTE_DESIGN.ARGS.DIRECTIVE={IMPLEMENTATION_STRATEGY}"
        ));
    if let Some(config) = &resolved.connectivity_config {
        inv = inv.arg("--config").arg(config.as_str());
    }
    if let Some(xdc) = &resolved.floorplan_xdc {
        inv = inv.arg(format!(
            "--vivado.prop=run.impl_1.STEPS.OPT_DESIGN.TCL.PRE={xdc}"
        ));
    }
    inv = inv
        .arg(format!(
            "--vivado.prop=run.impl_1.STEPS.POST_ROUTE_PHYS_OPT_DESIGN.TCL.POST={}",
            resolved.timing_hook
        ))
        .arg(resolved.xo.as_str());

    inv.cwd = Some(resolved.work_dir.clone());
    inv.env
        .insert("HOME".into(), resolved.work_dir.as_str().to_string());
    inv.uploads.push(resolved.xo.clone());
    inv.uploads.push(resolved.timing_hook.clone());
    if let Some(xdc) = &resolved.floorplan_xdc {
        inv.uploads.push(xdc.clone());
    }
    if let Some(config) = &resolved.connectivity_config {
        inv.uploads.push(config.clone());
    }
    inv.downloads.push(resolved.artifacts_dir.clone());
    inv
}

/// Run one hardware link and return its kernel-clock timing result.
pub fn run_vitis_link(runner: &dyn ToolRunner, job: &VitisLinkJob) -> Result<VitisLinkOutput> {
    let resolved = ResolvedJob::new(job)?;
    validate_input(&resolved.xo, "XO")?;
    if let Some(xdc) = &resolved.floorplan_xdc {
        validate_input(xdc, "floorplan XDC")?;
    }
    if let Some(config) = &resolved.connectivity_config {
        validate_input(config, "connectivity config")?;
    }

    fs_err::create_dir_all(&resolved.work_dir)?;
    let output_parent = resolved
        .output_xclbin
        .parent()
        .ok_or_else(|| XilinxError::VitisLink("xclbin path has no parent directory".into()))?;
    fs_err::create_dir_all(output_parent)?;
    fs_err::create_dir_all(&resolved.report_dir)?;
    fs_err::create_dir_all(&resolved.log_dir)?;
    remove_stale_output(&resolved.output_xclbin)?;
    remove_stale_output(&resolved.timing_report)?;
    fs_err::write(&resolved.timing_hook, final_timing_tcl().as_bytes())?;

    let invocation = build_invocation(&resolved);
    let tool_output = runner.run(&invocation)?;
    if tool_output.exit_code != 0 {
        return Err(XilinxError::ToolFailure {
            program: "v++".into(),
            code: tool_output.exit_code,
            stderr: super::merged_failure_output(tool_output.stdout, tool_output.stderr),
        });
    }
    if !resolved.timing_report.is_file() {
        return link_error(format!(
            "v++ exited successfully but did not produce timing report `{}`",
            resolved.timing_report
        ));
    }
    let timing_text = fs_err::read_to_string(&resolved.timing_report)?;
    let timing = parse_kernel_timing_summary(&timing_text)?;

    Ok(VitisLinkOutput {
        timing_report: resolved.timing_report,
        timing,
        stdout: tool_output.stdout,
        stderr: tool_output.stderr,
    })
}

fn remove_stale_output(path: &Utf8Path) -> Result<()> {
    match fs_err::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::process::{MockToolRunner, ToolOutput};

    struct Fixture {
        _temp: tempfile::TempDir,
        job: VitisLinkJob,
    }

    impl Fixture {
        fn new() -> Self {
            let temp = tempfile::tempdir().expect("tempdir");
            let root = crate::util::utf8(temp.path());
            let work = root.join("work");
            let artifacts = work.join("artifacts");
            let xo = root.join("kernel.xo");
            let xdc = root.join("floorplan.xdc");
            let config = root.join("connectivity.ini");
            fs_err::write(&xo, b"xo").expect("write XO");
            fs_err::write(&xdc, b"xdc").expect("write XDC");
            fs_err::write(&config, b"config").expect("write config");
            let job = VitisLinkJob::builder()
                .kernel_name("Top".into())
                .xo(xo)
                .platform("xilinx_test_platform".into())
                .target_mhz(300)
                .work_dir(work.clone())
                .artifacts_dir(artifacts.clone())
                .output_xclbin(artifacts.join("Top.xclbin"))
                .report_dir(artifacts.join("reports"))
                .log_dir(artifacts.join("logs"))
                .temp_dir(work.join("vitis.tmp"))
                .floorplan_xdc(Some(xdc))
                .connectivity_config(Some(config))
                .build();
            Self { _temp: temp, job }
        }

        fn timing_report(&self) -> Utf8PathBuf {
            self.job
                .output_xclbin
                .parent()
                .unwrap()
                .join(TIMING_REPORT_NAME)
        }

        fn attach_success(&self, runner: &MockToolRunner) {
            runner.push_ok(
                "v++",
                ToolOutput {
                    exit_code: 0,
                    stdout: "linked".into(),
                    stderr: "warning".into(),
                },
            );
            runner.attach_download(self.timing_report(), timing_report());
        }
    }

    fn timing_report() -> Vec<u8> {
        b"
| Clock Summary
| -------------

Clock   Waveform(ns)         Period(ns)      Frequency(MHz)
-----   ------------         ----------      --------------
ap_clk  {0.000 1.667}        3.333           300.030

| Intra Clock Table
| -----------------

Clock             WNS(ns)      TNS(ns)
-----             -------      -------
ap_clk             -0.100       -1.000

"
        .to_vec()
    }

    #[test]
    fn conservative_frequency_conversion_never_rounds_up() {
        let period = 1000.0 / 300.75;
        assert_eq!(target_frequency_mhz(period).unwrap(), 300);
        assert_eq!(target_frequency_mhz(2.5).unwrap(), 400);
    }

    #[test]
    fn frequency_conversion_rejects_invalid_and_out_of_range_values() {
        for period in [0.0, -1.0, f64::NAN, f64::INFINITY, 1001.0, 1e-300] {
            assert!(
                target_frequency_mhz(period).is_err(),
                "period {period} must fail",
            );
        }
    }

    #[test]
    fn run_builds_exact_direct_invocation_and_transfer_contract() {
        let fixture = Fixture::new();
        let runner = MockToolRunner::new();
        fixture.attach_success(&runner);

        let output = run_vitis_link(&runner, &fixture.job).expect("link succeeds");
        assert_eq!(output.stdout, "linked");
        assert_eq!(output.stderr, "warning");
        assert_eq!(output.timing_report, fixture.timing_report());
        assert!(!fixture.job.output_xclbin.exists());

        let calls = runner.calls();
        assert_eq!(calls.len(), 1);
        let call = &calls[0];
        let hook = fixture
            .job
            .output_xclbin
            .parent()
            .unwrap()
            .join(FINAL_TIMING_HOOK_NAME);
        assert_eq!(call.program, "v++");
        assert_eq!(
            call.args,
            vec![
                "--link",
                "--target",
                "hw",
                "--save-temps",
                "--to_step",
                "vpl.impl.post_route_phys_opt_design",
                "--platform",
                "xilinx_test_platform",
                "--output",
                fixture.job.output_xclbin.as_str(),
                "--report_dir",
                fixture.job.report_dir.as_str(),
                "--log_dir",
                fixture.job.log_dir.as_str(),
                "--temp_dir",
                fixture.job.temp_dir.as_str(),
                "--optimize",
                "3",
                "--kernel_frequency",
                "300",
                "--connectivity.nk",
                "Top:1:Top",
                "--vivado.prop=run.impl_1.STEPS.PHYS_OPT_DESIGN.IS_ENABLED=1",
                "--vivado.prop=run.impl_1.STEPS.POST_ROUTE_PHYS_OPT_DESIGN.IS_ENABLED=1",
                "--vivado.prop=run.impl_1.STEPS.OPT_DESIGN.ARGS.DIRECTIVE=Explore",
                "--vivado.prop=run.impl_1.STEPS.PLACE_DESIGN.ARGS.DIRECTIVE=EarlyBlockPlacement",
                "--vivado.prop=run.impl_1.STEPS.PHYS_OPT_DESIGN.ARGS.DIRECTIVE=Explore",
                "--vivado.prop=run.impl_1.STEPS.ROUTE_DESIGN.ARGS.DIRECTIVE=Explore",
                "--config",
                fixture.job.connectivity_config.as_ref().unwrap().as_str(),
                &format!(
                    "--vivado.prop=run.impl_1.STEPS.OPT_DESIGN.TCL.PRE={}",
                    fixture.job.floorplan_xdc.as_ref().unwrap()
                ),
                &format!(
                    "--vivado.prop=run.impl_1.STEPS.POST_ROUTE_PHYS_OPT_DESIGN.TCL.POST={hook}"
                ),
                fixture.job.xo.as_str(),
            ]
        );
        assert!(!call.args.iter().any(|arg| arg.contains("WRITE_BITSTREAM")));
        assert_eq!(call.cwd.as_ref(), Some(&fixture.job.work_dir));
        assert_eq!(
            call.env.get("HOME"),
            Some(&fixture.job.work_dir.as_str().to_string())
        );
        assert_eq!(
            call.uploads,
            vec![
                fixture.job.xo.clone(),
                hook.clone(),
                fixture.job.floorplan_xdc.clone().unwrap(),
                fixture.job.connectivity_config.clone().unwrap(),
            ]
        );
        assert_eq!(call.downloads, vec![fixture.job.artifacts_dir.clone()]);
        assert!(!call.downloads.contains(&fixture.job.temp_dir));

        let hook_text = fs_err::read_to_string(hook).expect("read hook");
        assert!(hook_text.contains("[info script]"));
        assert!(hook_text.contains("timing_summary.rpt"));
        assert!(!hook_text.contains(fixture.job.work_dir.as_str()));
    }

    #[test]
    fn optional_hooks_are_omitted_from_args_and_uploads() {
        let mut fixture = Fixture::new();
        fixture.job.floorplan_xdc = None;
        fixture.job.connectivity_config = None;
        let runner = MockToolRunner::new();
        fixture.attach_success(&runner);
        run_vitis_link(&runner, &fixture.job).expect("link succeeds");
        let call = &runner.calls()[0];
        assert!(!call.args.iter().any(|arg| arg == "--config"));
        assert!(!call
            .args
            .iter()
            .any(|arg| arg.contains("OPT_DESIGN.TCL.PRE")));
        assert_eq!(call.uploads.len(), 2);
    }

    #[test]
    fn rejects_zero_target_before_invocation() {
        let mut fixture = Fixture::new();
        fixture.job.target_mhz = 0;
        let runner = MockToolRunner::new();
        let error = run_vitis_link(&runner, &fixture.job).expect_err("zero target must fail");
        assert!(matches!(error, XilinxError::InvalidFrequency(_)));
        assert!(runner.calls().is_empty());
    }

    #[test]
    fn rejects_missing_inputs_before_invocation() {
        let mut fixture = Fixture::new();
        fixture.job.xo = fixture.job.work_dir.join("missing.xo");
        let runner = MockToolRunner::new();
        let error = run_vitis_link(&runner, &fixture.job).expect_err("missing XO must fail");
        assert!(matches!(error, XilinxError::VitisLink(_)));
        assert!(runner.calls().is_empty());
    }

    #[test]
    fn rejects_platform_paths_and_non_xclbin_outputs_before_invocation() {
        for platform in ["/opt/platform.xpfm", "platforms/test", "test.xpfm"] {
            let mut fixture = Fixture::new();
            fixture.job.platform = platform.to_string();
            let runner = MockToolRunner::new();
            let error = run_vitis_link(&runner, &fixture.job).expect_err("path must fail");
            assert!(error.to_string().contains("installed platform name"));
            assert!(runner.calls().is_empty());
        }

        let mut fixture = Fixture::new();
        fixture.job.output_xclbin = fixture.job.artifacts_dir.join("Top.xsa");
        let runner = MockToolRunner::new();
        let error = run_vitis_link(&runner, &fixture.job).expect_err("Versal output must fail");
        assert!(error.to_string().contains(".xclbin"));
        assert!(runner.calls().is_empty());
    }

    #[test]
    fn stale_timing_report_is_removed_before_invocation() {
        let fixture = Fixture::new();
        fs_err::create_dir_all(&fixture.job.artifacts_dir).expect("create artifacts");
        fs_err::write(&fixture.job.output_xclbin, b"old xclbin").expect("seed xclbin");
        fs_err::write(fixture.timing_report(), b"old timing").expect("seed timing");
        let runner = MockToolRunner::new();
        runner.push_ok("v++", ToolOutput::default());

        let error = run_vitis_link(&runner, &fixture.job).expect_err("stale output must not pass");
        assert!(error.to_string().contains("did not produce timing report"));
        assert!(!fixture.job.output_xclbin.exists());
        assert!(!fixture.timing_report().exists());
    }

    #[test]
    fn nonzero_exit_fails_even_with_a_parseable_timing_report() {
        let fixture = Fixture::new();
        let runner = MockToolRunner::new();
        runner.push_ok(
            "v++",
            ToolOutput {
                exit_code: 7,
                stdout: "link phase context".into(),
                stderr: "implementation failed".into(),
            },
        );
        runner.attach_download(fixture.timing_report(), timing_report());
        let error = run_vitis_link(&runner, &fixture.job).expect_err("nonzero exit must fail");
        let XilinxError::ToolFailure {
            code,
            stderr: output,
            ..
        } = error
        else {
            panic!("expected tool failure");
        };
        assert_eq!(code, 7);
        assert!(output.contains("stdout:\nlink phase context"));
        assert!(output.contains("stderr:\nimplementation failed"));
    }

    #[test]
    fn zero_exit_requires_a_fresh_parseable_timing_report() {
        let fixture = Fixture::new();
        let runner = MockToolRunner::new();
        runner.push_ok("v++", ToolOutput::default());
        let error = run_vitis_link(&runner, &fixture.job).expect_err("missing report must fail");
        assert!(error.to_string().contains("did not produce timing report"));

        let fixture = Fixture::new();
        let runner = MockToolRunner::new();
        runner.push_ok("v++", ToolOutput::default());
        runner.attach_download(fixture.timing_report(), b"not a timing report".to_vec());
        run_vitis_link(&runner, &fixture.job).expect_err("unparseable report must fail");
    }

    #[test]
    fn rejects_temp_directory_inside_downloaded_artifacts() {
        let mut fixture = Fixture::new();
        fixture.job.temp_dir = fixture.job.artifacts_dir.join("large-project");
        let runner = MockToolRunner::new();
        let error = run_vitis_link(&runner, &fixture.job).expect_err("temp download must fail");
        assert!(error.to_string().contains("outside the downloadable"));
        assert!(runner.calls().is_empty());
    }
}
