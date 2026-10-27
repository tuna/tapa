//! Pure-Rust linking of one K5 package through the pinned SLASH linker.

mod hooks;
mod package;
mod vbin;

use camino::{Utf8Path, Utf8PathBuf};
use serde::Serialize;

use super::LinkOutput;
use crate::error::{Result, XilinxError};
use crate::runtime::process::{ToolInvocation, ToolOutput, ToolRunner};
use crate::tools::vitis::timing::{
    parse_named_clock_timing_summary, parse_named_clock_whs, parse_named_clock_wns, KernelTiming,
};
use crate::ToolValue;

const SLASH_COMMIT: &str = "26f9c55c";
const KERNEL_CLOCK: &str = "tapa_user_clk";
const FIXED_CLOCK: &str = "clk_wizard_0_clk_out1_1";
const METRICS_NAME: &str = "link-metrics.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlashLinkMode {
    Hardware,
    Simulation,
}

impl SlashLinkMode {
    const fn slashkit_name(self) -> &'static str {
        match self {
            Self::Hardware => "hw",
            Self::Simulation => "sim",
        }
    }

    const fn system_map_platform(self) -> &'static str {
        match self {
            Self::Hardware => "Hardware",
            Self::Simulation => "Simulation",
        }
    }
}

#[derive(Debug)]
pub struct SlashLinkJob {
    pub package: Utf8PathBuf,
    pub config: Utf8PathBuf,
    pub output: Utf8PathBuf,
    /// Per-link directory, conventionally `<work>/link`.
    pub work_dir: Utf8PathBuf,
    /// Program names are passed to ToolRunner verbatim and resolved on the
    /// tool host, never through the local process's PATH.
    pub slashkit: String,
    pub vivado: String,
    pub jobs: u32,
    pub target_mhz: u32,
    pub floorplan_xdc: Option<Utf8PathBuf>,
    pub mode: SlashLinkMode,
}

/// Link a K5 ZIP without Python, base-address relocation, or finalization.
/// Producing a vbin is success; a missed kernel period is recorded with a
/// warning, and fixed-shell timing never fails the link. A simulation link
/// records the requested clock unchanged: slashkit emits no routed timing
/// report in `sim` mode, so there is nothing to parse or derate from.
pub fn link(runner: &dyn ToolRunner, job: &SlashLinkJob) -> Result<LinkOutput> {
    let prepared = prepare_link(job)?;
    let vivado = resolve_tool_host_program(runner, &job.vivado)?;
    let tool_output = runner.run(&build_invocation(job, &prepared, &vivado))?;
    finish_link(job, prepared, tool_output)
}

fn resolve_tool_host_program(runner: &dyn ToolRunner, program: &str) -> Result<String> {
    if Utf8Path::new(program).is_absolute() {
        return Ok(program.to_string());
    }
    let output = runner.run(&ToolInvocation::new("which").arg(program))?;
    if output.exit_code != 0 {
        return Err(slash_error(format!(
            "cannot resolve `{program}` on the tool host: {}",
            output.stderr.trim()
        )));
    }
    let resolved = output.stdout.trim();
    if !Utf8Path::new(resolved).is_absolute() {
        return Err(slash_error(format!(
            "tool-host resolution of `{program}` did not return an absolute path: `{resolved}`"
        )));
    }
    Ok(resolved.to_string())
}

#[derive(Debug)]
struct PreparedLink {
    output: Utf8PathBuf,
    work: Utf8PathBuf,
    ip: Utf8PathBuf,
    hls_data: Utf8PathBuf,
    config: Utf8PathBuf,
    hook: Utf8PathBuf,
    clock_xdc: Utf8PathBuf,
    floorplan_xdc: Option<Utf8PathBuf>,
    /// Routed timing report to download; `None` in simulation mode, where
    /// slashkit produces no routed timing report.
    timing_report: Option<Utf8PathBuf>,
}

fn prepare_link(job: &SlashLinkJob) -> Result<PreparedLink> {
    validate_job(job)?;
    let package = crate::util::absolutize(&job.package);
    let input_config = crate::util::absolutize(&job.config);
    let output = crate::util::absolutize(&job.output);
    let work = crate::util::absolutize(&job.work_dir);
    let floorplan_xdc = job.floorplan_xdc.as_deref().map(crate::util::absolutize);
    validate_input(&package, "K5 package")?;
    validate_input(&input_config, "SLASH connectivity config")?;
    if let Some(xdc) = &floorplan_xdc {
        validate_input(xdc, "floorplan XDC")?;
    }
    if output.exists() {
        return Err(slash_error(format!(
            "SLASH output `{output}` already exists; use a fresh output path"
        )));
    }

    fs_err::create_dir_all(&work)?;
    if let Some(parent) = output.parent() {
        fs_err::create_dir_all(parent)?;
    }
    let ip = work.join("ip");
    let solution = work
        .parent()
        .ok_or_else(|| slash_error(format!("SLASH link work directory `{work}` has no parent")))?;
    let hls_data = solution.join("hls_data.json");
    let metadata = package::extract_package(&package, &ip, &hls_data)?;

    let input_config_text = fs_err::read_to_string(input_config)?;
    let mut config_text = crate::connectivity::prepare_slash_config(
        &input_config_text,
        &metadata.top_name,
        &metadata.ports,
        requested_hz(job.target_mhz),
    )
    .map_err(XilinxError::SlashLink)?;
    if !config_text.ends_with('\n') {
        config_text.push('\n');
    }
    config_text.push_str("\n[user_region]\npre_synth=tapa_pre_synth.tcl\n");

    let config = work.join("config.cfg");
    let hook = work.join(hooks::HOOK_NAME);
    let clock_xdc = work.join(hooks::CLOCK_XDC_NAME);
    fs_err::write(&config, config_text)?;
    fs_err::write(&hook, hooks::pre_synth_hook(floorplan_xdc.is_some()))?;
    fs_err::write(&clock_xdc, hooks::user_clock_xdc(job.target_mhz))?;
    let floorplan_xdc = floorplan_xdc
        .as_ref()
        .map(|source| {
            let destination = work.join(hooks::FLOORPLAN_XDC_NAME);
            fs_err::copy(source, &destination)?;
            Ok::<_, XilinxError>(destination)
        })
        .transpose()?;
    let timing_report = match job.mode {
        SlashLinkMode::Hardware => Some(routed_timing_report(&output)?),
        SlashLinkMode::Simulation => None,
    };
    Ok(PreparedLink {
        output,
        work,
        ip,
        hls_data,
        config,
        hook,
        clock_xdc,
        floorplan_xdc,
        timing_report,
    })
}

fn build_invocation(job: &SlashLinkJob, prepared: &PreparedLink, vivado: &str) -> ToolInvocation {
    let mut invocation = ToolInvocation::new(job.slashkit.clone())
        .arg("link")
        .arg("-p")
        .arg(job.mode.slashkit_name())
        .arg("--vivado")
        .arg(vivado)
        .arg("--jobs")
        .arg(job.jobs.to_string())
        .arg("-c")
        .arg(ToolValue::path(&prepared.config))
        .arg("-k")
        .arg(ToolValue::path(prepared.ip.join("component.xml")))
        .arg("-o")
        .arg(ToolValue::path(&prepared.output))
        .arg("--clock-hz")
        .arg(requested_hz(job.target_mhz).to_string())
        .arg("--pre-synth-tcls")
        .arg(ToolValue::path(&prepared.hook));
    invocation.cwd = Some(prepared.work.clone());
    invocation.uploads = vec![
        prepared.ip.clone(),
        prepared.hls_data.clone(),
        prepared.config.clone(),
        prepared.hook.clone(),
        prepared.clock_xdc.clone(),
    ];
    if let Some(xdc) = &prepared.floorplan_xdc {
        invocation.uploads.push(xdc.clone());
    }
    invocation.downloads = vec![prepared.output.clone()];
    if let Some(timing_report) = &prepared.timing_report {
        invocation.downloads.push(timing_report.clone());
    }
    invocation
}

fn finish_link(
    job: &SlashLinkJob,
    prepared: PreparedLink,
    tool_output: ToolOutput,
) -> Result<LinkOutput> {
    if tool_output.exit_code != 0 {
        return Err(super::tool_failure(&job.slashkit, tool_output));
    }
    validate_input(&prepared.output, "SLASH vbin output")?;

    let system_map = vbin::read_system_map(&prepared.output)?;
    if system_map.platform != job.mode.system_map_platform() {
        return Err(slash_error(format!(
            "SLASH {} vbin has Platform `{}`, expected `{}`",
            job.mode.slashkit_name(),
            system_map.platform,
            job.mode.system_map_platform(),
        )));
    }
    let requested_hz = requested_hz(job.target_mhz);
    let (timing, timing_report, fixed_wns_ns, recorded_mhz) = match job.mode {
        SlashLinkMode::Hardware => {
            let timing_report = prepared.timing_report.clone().ok_or_else(|| {
                slash_error("hardware link preparation produced no routed timing report path")
            })?;
            validate_input(&timing_report, "SLASH routed timing report")?;
            let report = fs_err::read_to_string(&timing_report)?;
            let timing = parse_named_clock_timing_summary(&report, KERNEL_CLOCK)?;
            let fixed_wns_ns = parse_named_clock_wns(&report, FIXED_CLOCK)?;
            let kernel_whs_ns = parse_named_clock_whs(&report, KERNEL_CLOCK)?;
            let clock_hz = recorded_clock_hz(requested_hz, &timing)?;
            if system_map.clock_hz != clock_hz {
                vbin::rewrite_clock(&prepared.output, clock_hz)?;
            }
            if clock_hz < requested_hz {
                log::warn!(
                    "SLASH routed kernel timing did not meet {} MHz; recording {:.6} MHz in `{}`",
                    job.target_mhz,
                    hz_to_mhz(clock_hz),
                    prepared.output,
                );
            }
            if kernel_whs_ns < 0.0 {
                log::warn!(
                    "SLASH kernel clock `{KERNEL_CLOCK}` has hold WHS {kernel_whs_ns:.3} ns; \
                     the vbin is retained without clock derating",
                );
            }
            if fixed_wns_ns < 0.0 {
                log::warn!(
                    "SLASH fixed 400 MHz domain `{FIXED_CLOCK}` has WNS {fixed_wns_ns:.3} ns; \
                     leaving shell timing unchanged",
                );
            }
            (
                Some(timing),
                Some(timing_report),
                Some(fixed_wns_ns),
                hz_to_mhz(clock_hz),
            )
        }
        SlashLinkMode::Simulation => (None, None, None, hz_to_mhz(requested_hz)),
    };

    write_metrics(
        &prepared.work.join(METRICS_NAME),
        job.target_mhz,
        recorded_mhz,
        timing.as_ref(),
        fixed_wns_ns,
        timing_report.as_deref(),
    )?;
    Ok(LinkOutput {
        bitstream: prepared.output,
        requested_mhz: job.target_mhz,
        recorded_mhz,
        timing,
        timing_report,
        stdout: tool_output.stdout,
        stderr: tool_output.stderr,
    })
}

fn requested_hz(target_mhz: u32) -> u64 {
    u64::from(target_mhz) * 1_000_000
}

fn validate_job(job: &SlashLinkJob) -> Result<()> {
    if job.output.extension() != Some("vbin") {
        return Err(slash_error(
            "SLASH link output must use the .vbin extension",
        ));
    }
    if job.target_mhz == 0 {
        return Err(XilinxError::InvalidFrequency(
            "SLASH target frequency must be positive, got `0` MHz".into(),
        ));
    }
    if job.jobs == 0 {
        return Err(slash_error("SLASH job count must be positive"));
    }
    if job.slashkit.trim().is_empty() || job.vivado.trim().is_empty() {
        return Err(slash_error("SLASH tool names must not be empty"));
    }
    Ok(())
}

fn validate_input(path: &Utf8Path, label: &str) -> Result<()> {
    if path.is_file() {
        Ok(())
    } else {
        Err(slash_error(format!("{label} `{path}` is not a file")))
    }
}

fn routed_timing_report(output: &Utf8Path) -> Result<Utf8PathBuf> {
    let stem = output
        .file_stem()
        .ok_or_else(|| slash_error(format!("SLASH output `{output}` has no file stem")))?;
    let mut project = stem
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '_' {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    if project.is_empty() {
        project.push_str("proj");
    }
    if project.starts_with(|character: char| character.is_ascii_digit()) {
        project.insert(0, '_');
    }
    Ok(Utf8PathBuf::from(format!("{output}.prj"))
        .join("slash_rm")
        .join(format!("report_timing_{project}.txt")))
}

fn recorded_clock_hz(requested_hz: u64, timing: &KernelTiming) -> Result<u64> {
    #[allow(
        clippy::cast_precision_loss,
        reason = "SLASH requests are u32 MHz, so their Hz values remain below 2^53"
    )]
    let requested = requested_hz as f64;
    let recorded = (1_000_000_000.0 / timing.achieved_period_ns)
        .floor()
        .min(requested);
    if !recorded.is_finite() || recorded < 1.0 {
        return Err(slash_error(format!(
            "SLASH achieved clock is invalid for period {} ns and WNS {} ns",
            timing.achieved_period_ns, timing.wns_ns,
        )));
    }
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "recorded was floored and checked to be in 1..=requested_hz"
    )]
    Ok(recorded as u64)
}

fn hz_to_mhz(hz: u64) -> f64 {
    #[allow(
        clippy::cast_precision_loss,
        reason = "recorded SLASH clocks are capped by a u32-MHz request"
    )]
    (hz as f64 / 1_000_000.0)
}

/// Link record shared by both modes: the pinned commit and the clock request
/// versus recording are always written; the routed-timing fields exist only
/// for hardware links, which are the only links with a routed timing report.
#[derive(Debug, Serialize)]
struct LinkMetrics<'a> {
    slash_commit: &'static str,
    requested_mhz: u32,
    recorded_mhz: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    kernel_wns_ns: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    kernel_fmax_mhz: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    timing_closed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    fixed_domain_wns_ns: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    timing_report: Option<&'a Utf8Path>,
}

fn write_metrics(
    path: &Utf8Path,
    requested_mhz: u32,
    recorded_mhz: f64,
    timing: Option<&KernelTiming>,
    fixed_domain_wns_ns: Option<f64>,
    timing_report: Option<&Utf8Path>,
) -> Result<()> {
    let metrics = LinkMetrics {
        slash_commit: SLASH_COMMIT,
        requested_mhz,
        recorded_mhz,
        kernel_wns_ns: timing.map(|timing| timing.wns_ns),
        kernel_fmax_mhz: timing.map(|timing| timing.fmax_mhz),
        timing_closed: timing.map(|timing| timing.wns_ns >= 0.0),
        fixed_domain_wns_ns,
        timing_report,
    };
    let mut bytes = serde_json::to_vec_pretty(&metrics)?;
    bytes.push(b'\n');
    fs_err::write(path, bytes)?;
    Ok(())
}

fn slash_error(detail: impl Into<String>) -> XilinxError {
    XilinxError::SlashLink(detail.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::process::{MockToolRunner, ToolOutput};

    fn fixture(name: &str) -> Utf8PathBuf {
        Utf8PathBuf::from(format!(
            "{}/testdata/slash/{name}",
            env!("CARGO_MANIFEST_DIR")
        ))
    }

    #[test]
    fn parses_real_kernel_and_fixed_rows_without_global_timing_failure() {
        let vadd = fs_err::read_to_string(fixture("vadd-routed-timing.rpt")).expect("vadd report");
        let timing = parse_named_clock_timing_summary(&vadd, KERNEL_CLOCK).expect("kernel row");
        let fixed_wns_ns = parse_named_clock_wns(&vadd, FIXED_CLOCK).expect("fixed row");
        assert_eq!(timing.reported_target_period_ns, 3.333);
        assert_eq!(timing.wns_ns, 0.006);
        assert!(timing.fmax_mhz > 300.0);
        assert_eq!(fixed_wns_ns, -0.592);
        assert!(vadd.contains("Timing constraints are not met"));

        let callipepla = fs_err::read_to_string(fixture("callipepla-routed-timing.rpt"))
            .expect("Callipepla report");
        let timing = parse_named_clock_timing_summary(&callipepla, KERNEL_CLOCK)
            .expect("negative kernel slack is data");
        let fixed_wns_ns =
            parse_named_clock_wns(&callipepla, FIXED_CLOCK).expect("negative fixed slack is data");
        assert_eq!(timing.wns_ns, -5.529);
        assert_eq!(fixed_wns_ns, -1.023);
    }

    #[test]
    fn pure_rust_link_transfers_only_required_files_and_derates_vbin() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = crate::util::utf8(temp.path());
        let work = root.join("work").join("link");
        let output_path = root.join("vadd.hw.vbin");
        let report_path = routed_timing_report(&output_path).expect("report path");
        let floorplan = root.join("floorplan-input.xdc");
        fs_err::write(&floorplan, "# floorplan\n").expect("write floorplan");
        let job = SlashLinkJob {
            package: fixture("vadd-package.zip"),
            config: fixture("vadd-config.cfg"),
            output: output_path.clone(),
            work_dir: work.clone(),
            slashkit: "/tool-host/bin/slashkit".into(),
            vivado: "vivado-on-tool-host".into(),
            jobs: 3,
            target_mhz: 300,
            floorplan_xdc: Some(floorplan),
            mode: SlashLinkMode::Hardware,
        };
        let runner = MockToolRunner::new();
        runner.push_ok(
            "which",
            ToolOutput {
                exit_code: 0,
                stdout: "/tool-host/bin/vivado\n".into(),
                stderr: String::new(),
            },
        );
        runner.push_ok(
            &job.slashkit,
            ToolOutput {
                exit_code: 0,
                stdout: "linked".into(),
                stderr: "warning".into(),
            },
        );
        runner.attach_download(
            &output_path,
            fs_err::read(fixture("vadd-gzip.vbin")).expect("gzip fixture"),
        );
        runner.attach_download(
            &report_path,
            fs_err::read(fixture("callipepla-routed-timing.rpt")).expect("timing fixture"),
        );

        let result = link(&runner, &job).expect("negative kernel/fixed slack still links");
        assert_eq!(result.timing_report, Some(report_path.clone()));
        let timing = result.hardware_timing().expect("hardware timing");
        let expected_recorded = recorded_clock_hz(300_000_000, timing).expect("recorded clock");
        assert_eq!(result.bitstream, output_path);
        assert_eq!(result.recorded_mhz, hz_to_mhz(expected_recorded));
        assert_eq!(result.stdout, "linked");
        assert_eq!(result.stderr, "warning");
        assert_eq!(
            vbin::read_system_map(&result.bitstream)
                .expect("rewritten map")
                .clock_hz,
            expected_recorded,
        );

        let calls = runner.calls();
        assert_eq!(
            calls.len(),
            2,
            "tool-host resolution and slashkit; no Python/finalize/verify"
        );
        assert_eq!(calls[0].program, "which");
        let call = &calls[1];
        assert_eq!(call.program, "/tool-host/bin/slashkit");
        let args = call.local_args();
        assert!(args.windows(2).any(|pair| pair == ["-p", "hw"]));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--pre-synth-tcls", work.join("tapa_pre_synth.tcl").as_str()]));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--vivado", "/tool-host/bin/vivado"]));
        assert_eq!(call.downloads, vec![output_path, report_path.clone()]);
        assert_eq!(call.uploads.len(), 6);
        assert!(call.uploads.contains(&work.join("floorplan.xdc")));
        assert!(call.uploads.contains(&work.join("ip")));
        assert!(call
            .uploads
            .contains(&root.join("work").join("hls_data.json")));
        assert!(!args.iter().any(|arg| arg.contains("python")));

        let config = fs_err::read_to_string(work.join("config.cfg")).expect("staged config");
        assert!(config.contains("[user_region]\npre_synth=tapa_pre_synth.tcl"));
        assert!(config.contains("freqhz=300000000"));
        assert!(work.join("ip").join("component.xml").is_file());
        let hook = fs_err::read_to_string(work.join("tapa_pre_synth.tcl")).expect("hook");
        assert!(hook.contains("STEPS.PLACE_DESIGN.TCL.PRE"));
        let metrics: serde_json::Value = serde_json::from_slice(
            &fs_err::read(work.join(METRICS_NAME)).expect("read link metrics"),
        )
        .expect("metrics JSON");
        assert_eq!(metrics["slash_commit"], SLASH_COMMIT);
        assert_eq!(metrics["requested_mhz"], 300);
        assert_eq!(metrics["recorded_mhz"], result.recorded_mhz);
        assert_eq!(metrics["kernel_wns_ns"], -5.529);
        assert_eq!(metrics["fixed_domain_wns_ns"], -1.023);
        assert_eq!(metrics["timing_closed"], false);
        assert_eq!(metrics["timing_report"], report_path.as_str());
    }

    #[test]
    fn simulation_mode_is_forwarded_to_the_tool_host() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = crate::util::utf8(temp.path());
        let output = root.join("vadd.sim.vbin");
        let job = SlashLinkJob {
            package: fixture("vadd-package.zip"),
            config: fixture("vadd-config.cfg"),
            output: output.clone(),
            work_dir: root.join("work").join("link"),
            slashkit: "slashkit-not-on-local-path".into(),
            vivado: "vivado-not-on-local-path".into(),
            jobs: 1,
            target_mhz: 300,
            floorplan_xdc: None,
            mode: SlashLinkMode::Simulation,
        };
        let prepared = prepare_link(&job).expect("prepare simulation link");
        let invocation = build_invocation(&job, &prepared, &job.vivado);
        assert_eq!(invocation.program, "slashkit-not-on-local-path");
        assert!(invocation
            .local_args()
            .windows(2)
            .any(|pair| pair == ["-p", "sim"]));
        assert!(invocation
            .local_args()
            .windows(2)
            .any(|pair| pair == ["--vivado", "vivado-not-on-local-path"]));
        assert_eq!(invocation.downloads, vec![output]);
    }

    #[test]
    fn simulation_finish_link_needs_no_timing_report() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = crate::util::utf8(temp.path());
        let work = root.join("work").join("link");
        fs_err::create_dir_all(&work).expect("link work directory");
        let output = root.join("vadd.sim.vbin");
        fs_err::copy(fixture("vadd-sim.vbin"), &output).expect("stage simulation vbin");
        let job = SlashLinkJob {
            package: root.join("vadd-package.zip"),
            config: root.join("vadd-config.cfg"),
            output: output.clone(),
            work_dir: work.clone(),
            slashkit: "/tool-host/bin/slashkit".into(),
            vivado: "vivado".into(),
            jobs: 1,
            target_mhz: 300,
            floorplan_xdc: None,
            mode: SlashLinkMode::Simulation,
        };
        let prepared = PreparedLink {
            output: output.clone(),
            work: work.clone(),
            ip: work.join("ip"),
            hls_data: root.join("work").join("hls_data.json"),
            config: work.join("config.cfg"),
            hook: work.join(hooks::HOOK_NAME),
            clock_xdc: work.join(hooks::CLOCK_XDC_NAME),
            floorplan_xdc: None,
            timing_report: None,
        };

        let result = finish_link(
            &job,
            prepared,
            ToolOutput {
                exit_code: 0,
                stdout: "simulated".into(),
                stderr: String::new(),
            },
        )
        .expect("simulation link needs no routed timing report");

        assert_eq!(result.bitstream, output);
        assert_eq!(result.requested_mhz, 300);
        assert_eq!(result.recorded_mhz, 300.0);
        assert_eq!(result.timing, None);
        assert_eq!(result.timing_report, None);
        assert_eq!(result.stdout, "simulated");
        assert_eq!(
            vbin::read_system_map(&result.bitstream)
                .expect("simulation map")
                .clock_hz,
            300_000_000,
            "the simulation vbin is recorded at the requested clock, not derated",
        );

        let metrics: serde_json::Value = serde_json::from_slice(
            &fs_err::read(work.join(METRICS_NAME)).expect("read link metrics"),
        )
        .expect("metrics JSON");
        assert_eq!(metrics["slash_commit"], SLASH_COMMIT);
        assert_eq!(metrics["requested_mhz"], 300);
        assert_eq!(metrics["recorded_mhz"], 300.0);
        for field in [
            "kernel_wns_ns",
            "kernel_fmax_mhz",
            "timing_closed",
            "fixed_domain_wns_ns",
            "timing_report",
        ] {
            assert!(
                metrics.get(field).is_none(),
                "simulation metrics must omit `{field}`",
            );
        }
    }

    #[test]
    fn report_path_matches_pinned_slashkit_project_sanitization() {
        assert_eq!(
            routed_timing_report(Utf8Path::new("/tmp/9-vadd.hw.vbin"))
                .expect("report path")
                .as_str(),
            "/tmp/9-vadd.hw.vbin.prj/slash_rm/report_timing__9_vadd_hw.txt",
        );
    }
}
