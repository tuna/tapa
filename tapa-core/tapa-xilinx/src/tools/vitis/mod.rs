//! Direct `v++ --link` orchestration and implementation timing extraction.

use crate::ToolValue;

use camino::{Utf8Path, Utf8PathBuf};
use typed_builder::TypedBuilder;

use crate::error::{Result, XilinxError};
use crate::runtime::process::{ToolInvocation, ToolRunner};

pub mod timing;

use self::timing::parse_kernel_timing_summary;
use super::LinkOutput;

const TIMING_REPORT_NAME: &str = "timing_summary.rpt";
const FINAL_TIMING_HOOK_NAME: &str = "final_timing.tcl";
const IMPLEMENTATION_STRATEGY: &str = "Explore";
const PLACEMENT_STRATEGY: &str = "EarlyBlockPlacement";

/// Vivado `run.impl_1` step properties passed to `v++ --link` as
/// `--vivado.prop=run.impl_1.STEPS.<key>=<value>` pairs, in argv order.
/// Tuning the implementation flow is a one-line table edit.
const VIVADO_IMPL_STEP_PROPS: &[(&str, &str)] = &[
    ("PHYS_OPT_DESIGN.IS_ENABLED", "1"),
    ("POST_ROUTE_PHYS_OPT_DESIGN.IS_ENABLED", "1"),
    ("OPT_DESIGN.ARGS.DIRECTIVE", IMPLEMENTATION_STRATEGY),
    ("PLACE_DESIGN.ARGS.DIRECTIVE", PLACEMENT_STRATEGY),
    ("PHYS_OPT_DESIGN.ARGS.DIRECTIVE", IMPLEMENTATION_STRATEGY),
    ("ROUTE_DESIGN.ARGS.DIRECTIVE", IMPLEMENTATION_STRATEGY),
];

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

/// How much of the Vitis implementation flow to run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum VitisLinkMode {
    /// Produce a complete hardware bitstream (and package AIE links).
    #[default]
    Full,
    /// Stop after post-route physical optimization and read kernel timing.
    PostRouteTiming,
}

#[derive(Debug)]
struct AieInputs {
    libadf: Utf8PathBuf,
    config: Utf8PathBuf,
}

/// One portable Vitis hardware-link job.
///
/// `platform` must be an installed platform name. Platform paths are rejected
/// because they cannot be transferred safely to a remote tool host without
/// also resolving the files referenced by the platform.
#[derive(Debug, Clone, TypedBuilder)]
pub struct VitisLinkJob {
    pub kernel_name: String,
    pub xo: Utf8PathBuf,
    pub platform: String,
    /// Whole MHz requested for the kernel; AIE links convert it to Hz. Zero is rejected.
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
    /// Parallel Vivado synthesis job count for this link, forwarded to
    /// `--vivado.synth.jobs`. Acts as the per-link thread budget.
    #[builder(default = 2)]
    pub vivado_threads: u32,
    #[builder(default)]
    pub mode: VitisLinkMode,
    /// Vitis linker/packager program, resolved by the ToolRunner on the tool host.
    #[builder(default = "v++".to_string())]
    pub vpp: String,
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
    aie: Option<AieInputs>,
    link_output: Utf8PathBuf,
    package_temp_dir: Utf8PathBuf,
    timing_hook: Utf8PathBuf,
    timing_report: Utf8PathBuf,
}

fn validate_job(job: &VitisLinkJob) -> Result<()> {
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
    if job.vivado_threads == 0 {
        return link_error("Vivado thread count must be greater than zero, got `0`");
    }
    if job.vpp.trim().is_empty() {
        return link_error("v++ program name must not be empty");
    }
    Ok(())
}

impl<'a> ResolvedJob<'a> {
    fn new(job: &'a VitisLinkJob) -> Result<Self> {
        validate_job(job)?;

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
        if job.mode == VitisLinkMode::PostRouteTiming {
            ensure_under(
                &output_xclbin,
                &artifacts_dir,
                "post-route timing output",
                "artifacts directory",
            )?;
        }
        if output_xclbin.extension() != Some("xclbin") {
            return link_error(format!(
                "Vitis link output `{output_xclbin}` must use the `.xclbin` extension"
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
        let aie = discover_aie_inputs(&xo)?;
        if aie.is_some() && job.mode == VitisLinkMode::PostRouteTiming {
            return link_error("AI Engine sidecars require a full Vitis link");
        }
        let link_output = if aie.is_some() {
            let stem = output_xclbin
                .file_stem()
                .ok_or_else(|| XilinxError::VitisLink("xclbin path has no file stem".into()))?;
            artifacts_dir.join(format!("{stem}.xsa"))
        } else {
            output_xclbin.clone()
        };
        let package_temp_dir = Utf8PathBuf::from(format!("{temp_dir}.package"));
        let timing_hook = artifacts_dir.join(FINAL_TIMING_HOOK_NAME);
        let timing_report = artifacts_dir.join(TIMING_REPORT_NAME);

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
            aie,
            link_output,
            package_temp_dir,
            timing_hook,
            timing_report,
        })
    }
}

fn discover_aie_inputs(xo: &Utf8Path) -> Result<Option<AieInputs>> {
    let parent = xo
        .parent()
        .ok_or_else(|| XilinxError::VitisLink("XO path has no parent directory".into()))?;
    let stem = xo
        .file_stem()
        .ok_or_else(|| XilinxError::VitisLink("XO path has no file stem".into()))?;
    let libadf = parent.join(format!("{stem}.libadf.a"));
    let config = parent.join(format!("{stem}.aie.cfg"));
    if !libadf.exists() && !config.exists() {
        return Ok(None);
    }
    validate_input(&libadf, "AI Engine graph (`<name>.libadf.a`)")?;
    validate_input(&config, "AI Engine connectivity (`<name>.aie.cfg`)")?;
    Ok(Some(AieInputs { libadf, config }))
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

fn build_link_invocation(resolved: &ResolvedJob<'_>) -> ToolInvocation {
    let job = resolved.job;
    let mut inv = ToolInvocation::new(job.vpp.clone())
        .arg("--link")
        .arg("--target")
        .arg("hw")
        .arg("--save-temps");
    if job.mode == VitisLinkMode::PostRouteTiming {
        inv = inv
            .arg("--to_step")
            .arg("vpl.impl.post_route_phys_opt_design");
    }
    inv = inv
        .arg("--platform")
        .arg(job.platform.clone())
        .arg("--output")
        .arg(ToolValue::path(&resolved.link_output))
        .arg("--report_dir")
        .arg(ToolValue::path(&resolved.report_dir))
        .arg("--log_dir")
        .arg(ToolValue::path(&resolved.log_dir))
        .arg("--temp_dir")
        .arg(ToolValue::path(&resolved.temp_dir))
        .arg("--optimize")
        .arg("3");
    if resolved.aie.is_some() {
        inv = inv
            .arg("--clock.default_freqhz")
            .arg((u64::from(job.target_mhz) * 1_000_000).to_string());
    } else {
        inv = inv
            .arg("--kernel_frequency")
            .arg(job.target_mhz.to_string());
    }
    inv = inv
        .arg("--connectivity.nk")
        .arg(format!("{}:1:{}", job.kernel_name, job.kernel_name))
        .arg("--vivado.synth.jobs")
        .arg(job.vivado_threads.to_string());
    for (key, value) in VIVADO_IMPL_STEP_PROPS {
        inv = inv.arg(format!("--vivado.prop=run.impl_1.STEPS.{key}={value}"));
    }
    if let Some(aie) = &resolved.aie {
        inv = inv.arg("--config").arg(ToolValue::path(&aie.config));
    }
    if let Some(config) = &resolved.connectivity_config {
        inv = inv.arg("--config").arg(ToolValue::path(config));
    }
    if let Some(xdc) = &resolved.floorplan_xdc {
        inv = inv.arg(ToolValue::prefixed_path(
            "--vivado.prop=run.impl_1.STEPS.OPT_DESIGN.TCL.PRE=",
            xdc,
        ));
    }
    inv = inv
        .arg(ToolValue::prefixed_path(
            "--vivado.prop=run.impl_1.STEPS.POST_ROUTE_PHYS_OPT_DESIGN.TCL.POST=",
            &resolved.timing_hook,
        ))
        .arg(ToolValue::path(&resolved.xo));
    if let Some(aie) = &resolved.aie {
        inv = inv.arg(ToolValue::path(&aie.libadf));
    }

    inv.cwd = Some(resolved.work_dir.clone());
    inv.env
        .insert("HOME".into(), ToolValue::path(&resolved.work_dir));
    inv.uploads.push(resolved.xo.clone());
    inv.uploads.push(resolved.timing_hook.clone());
    if let Some(xdc) = &resolved.floorplan_xdc {
        inv.uploads.push(xdc.clone());
    }
    if let Some(config) = &resolved.connectivity_config {
        inv.uploads.push(config.clone());
    }
    if let Some(aie) = &resolved.aie {
        inv.uploads.push(aie.libadf.clone());
        inv.uploads.push(aie.config.clone());
    }
    inv.downloads.push(resolved.artifacts_dir.clone());
    if job.mode == VitisLinkMode::Full && !resolved.link_output.starts_with(&resolved.artifacts_dir)
    {
        inv.downloads.push(resolved.link_output.clone());
    }
    inv
}

fn build_package_invocation(resolved: &ResolvedJob<'_>) -> ToolInvocation {
    let job = resolved.job;
    let aie = resolved
        .aie
        .as_ref()
        .expect("package invocation is only built for AIE links");
    let mut inv = ToolInvocation::new(job.vpp.clone())
        .arg("--package")
        .arg("--target")
        .arg("hw")
        .arg("--platform")
        .arg(job.platform.clone())
        .arg("--save-temps")
        .arg("--temp_dir")
        .arg(ToolValue::path(&resolved.package_temp_dir))
        .arg("--output")
        .arg(ToolValue::path(&resolved.output_xclbin))
        .arg(ToolValue::path(&resolved.link_output))
        .arg(ToolValue::path(&aie.libadf));
    inv.cwd = Some(resolved.work_dir.clone());
    inv.env
        .insert("HOME".into(), ToolValue::path(&resolved.work_dir));
    inv.uploads = vec![resolved.link_output.clone(), aie.libadf.clone()];
    inv.downloads = vec![resolved.artifacts_dir.clone()];
    if !resolved.output_xclbin.starts_with(&resolved.artifacts_dir) {
        inv.downloads.push(resolved.output_xclbin.clone());
    }
    inv
}

/// Run one hardware link and return its kernel-clock timing result.
pub fn run_vitis_link(runner: &dyn ToolRunner, job: &VitisLinkJob) -> Result<LinkOutput> {
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
    if resolved.link_output != resolved.output_xclbin {
        remove_stale_output(&resolved.link_output)?;
    }
    remove_stale_output(&resolved.timing_report)?;
    fs_err::write(&resolved.timing_hook, final_timing_tcl().as_bytes())?;

    let link_output = runner.run(&build_link_invocation(&resolved))?;
    if link_output.exit_code != 0 {
        return Err(super::tool_failure(&job.vpp, link_output));
    }
    if job.mode == VitisLinkMode::Full && !resolved.link_output.is_file() {
        return link_error(format!(
            "v++ --link exited successfully but did not produce `{}`",
            resolved.link_output
        ));
    }

    let (stdout, stderr) = if resolved.aie.is_some() {
        let package_output = runner.run(&build_package_invocation(&resolved))?;
        if package_output.exit_code != 0 {
            return Err(super::tool_failure(&job.vpp, package_output));
        }
        validate_input(&resolved.output_xclbin, "packaged xclbin")?;
        (
            merge_tool_text(link_output.stdout, package_output.stdout),
            merge_tool_text(link_output.stderr, package_output.stderr),
        )
    } else {
        (link_output.stdout, link_output.stderr)
    };
    if !resolved.timing_report.is_file() {
        return link_error(format!(
            "v++ exited successfully but did not produce timing report `{}`",
            resolved.timing_report
        ));
    }
    let timing_text = fs_err::read_to_string(&resolved.timing_report)?;
    let timing = parse_kernel_timing_summary(&timing_text, Some(job.target_mhz))?;

    Ok(LinkOutput {
        bitstream: resolved.output_xclbin,
        requested_mhz: job.target_mhz,
        recorded_mhz: f64::from(job.target_mhz),
        timing,
        timing_report: resolved.timing_report,
        stdout,
        stderr,
    })
}

fn merge_tool_text(first: String, second: String) -> String {
    match (first.is_empty(), second.is_empty()) {
        (true, _) => second,
        (_, true) => first,
        (false, false) => format!(
            "{first}
{second}"
        ),
    }
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
                .mode(VitisLinkMode::PostRouteTiming)
                .floorplan_xdc(Some(xdc))
                .connectivity_config(Some(config))
                .build();
            Self { _temp: temp, job }
        }

        fn timing_report(&self) -> Utf8PathBuf {
            self.job.work_dir.join("artifacts").join(TIMING_REPORT_NAME)
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
            call.local_args(),
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
                "--vivado.synth.jobs",
                "2",
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
        assert!(!call
            .local_args()
            .iter()
            .any(|arg| arg.contains("WRITE_BITSTREAM")));
        assert_eq!(call.cwd.as_ref(), Some(&fixture.job.work_dir));
        assert_eq!(
            call.env.get("HOME").map(ToString::to_string),
            Some(fixture.job.work_dir.as_str().to_string())
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
    fn full_link_omits_to_step_and_requires_the_xclbin() {
        let mut fixture = Fixture::new();
        fixture.job.mode = VitisLinkMode::Full;
        fixture.job.vpp = "custom-vpp".into();
        fixture.job.output_xclbin = fixture
            .job
            .work_dir
            .parent()
            .expect("work parent")
            .join("published/Top.xclbin");
        let runner = MockToolRunner::new();
        runner.push_ok("custom-vpp", ToolOutput::default());
        runner.attach_download(&fixture.job.output_xclbin, b"xclbin".to_vec());
        runner.attach_download(fixture.timing_report(), timing_report());

        let output = run_vitis_link(&runner, &fixture.job).expect("full link succeeds");

        assert_eq!(output.bitstream, fixture.job.output_xclbin);
        let calls = runner.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].program, "custom-vpp");
        assert!(!calls[0].local_args().iter().any(|arg| arg == "--to_step"));
        assert!(calls[0].downloads.contains(&fixture.job.output_xclbin));
        assert_eq!(output.timing_report, fixture.timing_report());
    }

    #[test]
    fn aie_full_link_produces_xsa_then_packages_xclbin() {
        let mut fixture = Fixture::new();
        fixture.job.mode = VitisLinkMode::Full;
        let parent = fixture.job.xo.parent().expect("XO parent");
        let libadf = parent.join("kernel.libadf.a");
        let aie_config = parent.join("kernel.aie.cfg");
        fs_err::write(&libadf, b"adf").expect("write libadf");
        fs_err::write(&aie_config, b"aie config").expect("write AIE config");
        let xsa = fixture
            .job
            .output_xclbin
            .parent()
            .expect("output parent")
            .join("Top.xsa");
        let runner = MockToolRunner::new();
        runner.push_ok(
            "v++",
            ToolOutput {
                exit_code: 0,
                stdout: "linked".into(),
                stderr: String::new(),
            },
        );
        runner.attach_download(&xsa, b"xsa".to_vec());
        runner.attach_download(fixture.timing_report(), timing_report());
        runner.push_ok(
            "v++",
            ToolOutput {
                exit_code: 0,
                stdout: "packaged".into(),
                stderr: String::new(),
            },
        );
        runner.attach_download(&fixture.job.output_xclbin, b"xclbin".to_vec());

        let output = run_vitis_link(&runner, &fixture.job).expect("AIE link succeeds");

        assert_eq!(
            output.stdout,
            "linked
packaged"
        );
        let calls = runner.calls();
        assert_eq!(calls.len(), 2);
        let link_args = calls[0].local_args();
        assert!(link_args
            .windows(2)
            .any(|args| args == ["--link", "--target"]));
        assert!(!link_args.iter().any(|arg| arg == "--to_step"));
        assert!(link_args
            .windows(2)
            .any(|args| args == ["--output", xsa.as_str()]));
        assert!(link_args
            .windows(2)
            .any(|args| args == ["--clock.default_freqhz", "300000000"]));
        assert!(!link_args.iter().any(|arg| arg == "--kernel_frequency"));
        assert!(link_args
            .windows(2)
            .any(|args| args == ["--config", aie_config.as_str()]));
        assert_eq!(link_args.last(), Some(&libadf.to_string()));
        let package_args = calls[1].local_args();
        assert_eq!(package_args[0], "--package");
        assert!(package_args
            .windows(2)
            .any(|args| args == ["--output", fixture.job.output_xclbin.as_str()]));
        assert!(package_args.contains(&xsa.to_string()));
        assert!(package_args.contains(&libadf.to_string()));
        assert!(!package_args.iter().any(|arg| arg.contains("boot_mode")));
        assert!(!package_args.iter().any(|arg| arg.contains("kernel_image")));
        assert!(!package_args.iter().any(|arg| arg.contains("rootfs")));
        assert!(!package_args.iter().any(|arg| arg.contains("sd_file")));
    }

    #[test]
    fn incomplete_aie_sidecars_fail_before_invocation() {
        let fixture = Fixture::new();
        let libadf = fixture
            .job
            .xo
            .parent()
            .expect("XO parent")
            .join("kernel.libadf.a");
        fs_err::write(libadf, b"adf").expect("write libadf");
        let runner = MockToolRunner::new();
        let error = run_vitis_link(&runner, &fixture.job).expect_err("missing AIE config");
        assert!(error.to_string().contains("kernel.aie.cfg"));
        assert!(runner.calls().is_empty());
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
        assert!(!call.local_args().iter().any(|arg| arg == "--config"));
        assert!(!call
            .local_args()
            .iter()
            .any(|arg| arg.contains("OPT_DESIGN.TCL.PRE")));
        assert_eq!(call.uploads.len(), 2);
    }

    #[test]
    fn vivado_threads_defaults_to_two_and_is_forwarded_to_synth_jobs() {
        let fixture = Fixture::new();
        let runner = MockToolRunner::new();
        fixture.attach_success(&runner);
        run_vitis_link(&runner, &fixture.job).expect("link succeeds");
        let args = &runner.calls()[0].local_args();
        let pos = args
            .iter()
            .position(|arg| *arg == "--vivado.synth.jobs")
            .expect("--vivado.synth.jobs is emitted");
        assert_eq!(args[pos + 1], "2");

        // A caller-provided value is forwarded verbatim.
        let mut fixture = Fixture::new();
        fixture.job.vivado_threads = 4;
        let runner = MockToolRunner::new();
        fixture.attach_success(&runner);
        run_vitis_link(&runner, &fixture.job).expect("link succeeds");
        let args = &runner.calls()[0].local_args();
        let pos = args
            .iter()
            .position(|arg| *arg == "--vivado.synth.jobs")
            .expect("--vivado.synth.jobs is emitted");
        assert_eq!(args[pos + 1], "4");
    }

    #[test]
    fn rejects_zero_vivado_threads_before_invocation() {
        let mut fixture = Fixture::new();
        fixture.job.vivado_threads = 0;
        let runner = MockToolRunner::new();
        let error = run_vitis_link(&runner, &fixture.job).expect_err("zero threads must fail");
        assert!(error.to_string().contains("greater than zero"));
        assert!(runner.calls().is_empty());
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
        let error = run_vitis_link(&runner, &fixture.job).expect_err("non-xclbin output must fail");
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
