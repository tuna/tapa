//! TAPA XO to SLASH IP-XACT adaptation and V80 hardware linking.

use camino::Utf8PathBuf;
use regex::Regex;
use std::path::Path;

use crate::tools::vitis::timing::KernelTiming;

use crate::error::{Result, XilinxError};
use crate::runtime::process::{ToolInvocation, ToolRunner};

#[derive(Debug)]
pub struct SlashLinkJob {
    pub xo: Utf8PathBuf,
    pub config: Utf8PathBuf,
    pub output: Utf8PathBuf,
    pub work_dir: Utf8PathBuf,
    pub slashkit: String,
    pub vivado: String,
    pub jobs: u32,
    pub clock_hz: u64,
    pub floorplan_xdc: Option<Utf8PathBuf>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SlashLinkOutput {
    pub timing_report: Utf8PathBuf,
    pub timing: KernelTiming,
    pub stdout: String,
    pub stderr: String,
}

/// Link a single TAPA kernel for the service shell. A missed target is an
/// error even if SLASH emitted a VBIN with a derated clock.
pub fn link(runner: &dyn ToolRunner, job: &SlashLinkJob) -> Result<SlashLinkOutput> {
    if job.output.extension() != Some("vbin") || job.clock_hz == 0 || job.jobs == 0 {
        return Err(XilinxError::SlashLink(
            "link requires a .vbin output, a positive clock and a positive job count".into(),
        ));
    }
    let xo = crate::util::absolutize(&job.xo);
    let config = crate::util::absolutize(&job.config);
    let output = crate::util::absolutize(&job.output);
    let work = crate::util::absolutize(&job.work_dir);
    let ip = work.join("slash-ip");
    let slashkit = resolve_executable(&job.slashkit)?;
    let vivado = resolve_executable(&job.vivado)?;
    if output.exists() || ip.exists() {
        return Err(XilinxError::SlashLink(
            "SLASH output or export directory already exists; use a fresh link work directory"
                .into(),
        ));
    }
    fs_err::create_dir_all(&work)?;
    let adapter = work.join("slash_export.py");
    fs_err::write(&adapter, include_str!("export.py"))?;
    let mut export = ToolInvocation::new("python3")
        .arg(adapter.as_str())
        .arg("export")
        .arg(xo.as_str())
        .arg(ip.as_str());
    export.uploads = vec![adapter.clone(), xo];
    export.downloads = vec![ip.clone()];
    run(runner, &export)?;

    let mut invocation = ToolInvocation::new(slashkit.as_str())
        .arg("link")
        .arg("--vivado")
        .arg(vivado.as_str())
        .arg("--jobs")
        .arg(job.jobs.to_string())
        .arg("-c")
        .arg(config.as_str())
        .arg("-k")
        .arg(ip.join("impl/ip/component.xml").as_str())
        .arg("-o")
        .arg(output.as_str())
        .arg("-p")
        .arg("hw")
        .arg("--clock-hz")
        .arg(job.clock_hz.to_string());
    invocation.uploads = vec![ip, config];
    // The implementation project is retained on the tool host. Verification
    // runs there too, so remote linking need not download the Vivado project.
    invocation.downloads = vec![output.clone(), Utf8PathBuf::from(format!("{output}.prj"))];
    if let Some(xdc) = &job.floorplan_xdc {
        let xdc = crate::util::absolutize(xdc);
        let hook = work.join("slash_floorplan.tcl");
        fs_err::write(&hook, pre_synth_hook(&xdc))?;
        invocation
            .args
            .extend(["--pre-synth-tcls".into(), hook.to_string().into()]);
        invocation.uploads.extend([xdc, hook]);
    }
    let link_output = run_output(runner, &invocation)?;

    // SLASH's BD address assignment drives the hardware. Keep the exported
    // system map in sync so VRT's BAR lookup names that same AXI-Lite window.
    let project = Utf8PathBuf::from(format!("{output}.prj"));
    let mut finalize = ToolInvocation::new("python3")
        .arg(adapter.as_str())
        .arg("finalize")
        .arg(output.as_str());
    finalize.uploads = vec![adapter.clone(), output.clone(), project.clone()];
    finalize.downloads = vec![output.clone(), project.clone()];
    run(runner, &finalize)?;

    let mut verify = ToolInvocation::new("python3")
        .arg(adapter.as_str())
        .arg("verify")
        .arg(output.as_str())
        .arg(job.clock_hz.to_string());
    verify.uploads = vec![adapter, output.clone(), project];
    run(runner, &verify)?;
    let timing_report = find_timing_report(&Utf8PathBuf::from(format!("{output}.prj")))?;
    let timing_text = fs_err::read_to_string(&timing_report)?;
    let timing = parse_timing(&timing_text, job.clock_hz)?;
    Ok(SlashLinkOutput {
        timing_report,
        timing,
        stdout: link_output.stdout,
        stderr: link_output.stderr,
    })
}

fn pre_synth_hook(floorplan_xdc: &Utf8PathBuf) -> String {
    format!(
        "# TAPA floorplan.xdc is a Tcl implementation hook, not a plain XDC file.\n\
         # Vivado's XDC parser rejects the control-flow diagnostics it contains.\n\
         # Static OPT_DESIGN sees the reconfigurable module only as a black box.\n\
         # Apply cell constraints after the RM is linked and before placement.\n\
         set_property STEPS.PLACE_DESIGN.TCL.PRE [list {{{floorplan_xdc}}}] [get_runs impl_1]\n\
         # VRT reserves the first 64 KiB of the service-shell control aperture.\n\
         set tapa_kernel_cells [get_bd_cells -hier -filter {{VLNV =~ \"tapa:xrtl:*\"}}]\n\
         if {{[llength $tapa_kernel_cells] != 1}} {{ error \"TAPA V80 link requires exactly one TAPA kernel, found [llength $tapa_kernel_cells]\" }}\n\
         set tapa_kernel_cell [lindex $tapa_kernel_cells 0]\n\
         set tapa_control_seg [get_bd_addr_segs -quiet \"$tapa_kernel_cell/s_axi_control/reg0\"]\n\
         if {{[llength $tapa_control_seg] != 1}} {{ error \"TAPA V80 link could not resolve the kernel AXI-Lite register segment\" }}\n\
         set tapa_control_range [get_property RANGE $tapa_control_seg]\n\
         set tapa_control_space [get_bd_addr_spaces S_AXILITE_INI]\n\
         if {{[llength $tapa_control_space] != 1}} {{ error \"TAPA V80 link could not resolve the service-shell control address space\" }}\n\
         assign_bd_address -offset 0x020200010000 -range $tapa_control_range -target_address_space $tapa_control_space $tapa_control_seg -force\n\
         set tapa_mapped_name \"SEG_[get_property NAME $tapa_kernel_cell]_reg0\"\n\
         set tapa_mapped_segments [list]\n\
         foreach tapa_mapped_candidate [get_bd_addr_segs -quiet -of_objects $tapa_control_space] {{\n\
             if {{[get_property NAME $tapa_mapped_candidate] eq $tapa_mapped_name}} {{ lappend tapa_mapped_segments $tapa_mapped_candidate }}\n\
         }}\n\
         if {{[llength $tapa_mapped_segments] != 1}} {{ error \"TAPA V80 link could not resolve the mapped kernel AXI-Lite segment\" }}\n\
         set tapa_actual_offset [get_property OFFSET [lindex $tapa_mapped_segments 0]]\n\
         if {{$tapa_actual_offset != 0x020200010000}} {{ error \"TAPA V80 kernel AXI-Lite segment did not move past the service-shell reservation (got $tapa_actual_offset)\" }}\n"
    )
}

fn resolve_executable(program: &str) -> Result<Utf8PathBuf> {
    let path = Path::new(program);
    if path.is_absolute() || program.contains(std::path::MAIN_SEPARATOR) {
        let path = crate::util::absolutize(camino::Utf8Path::new(program));
        if path.is_file() {
            return Ok(path);
        }
    } else if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            let candidate = dir.join(program);
            if candidate.is_file() {
                return Ok(crate::util::utf8(candidate));
            }
        }
    }
    Err(XilinxError::SlashLink(format!(
        "SLASH tool `{program}` is not an executable file or PATH entry"
    )))
}

fn run(runner: &dyn ToolRunner, invocation: &ToolInvocation) -> Result<()> {
    let output = run_output(runner, invocation)?;
    if output.exit_code != 0 {
        return Err(super::tool_failure(&invocation.program, output));
    }
    Ok(())
}

fn run_output(
    runner: &dyn ToolRunner,
    invocation: &ToolInvocation,
) -> Result<crate::runtime::process::ToolOutput> {
    let output = runner.run(invocation)?;
    if output.exit_code != 0 {
        return Err(super::tool_failure(&invocation.program, output));
    }
    Ok(output)
}

fn find_timing_report(project: &Utf8PathBuf) -> Result<Utf8PathBuf> {
    let root = Path::new(project.as_str()).join("slash_rm");
    let mut reports = fs_err::read_dir(&root)?
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("report_timing_") && name.ends_with(".txt"))
        })
        .map(crate::util::utf8)
        .collect::<Vec<_>>();
    reports.sort();
    match reports.as_slice() {
        [report] => Ok(report.clone()),
        [] => Err(XilinxError::SlashLink(format!(
            "SLASH produced no routed timing report under `{project}`"
        ))),
        _ => Err(XilinxError::SlashLink(format!(
            "SLASH produced multiple routed timing reports under `{project}`"
        ))),
    }
}

fn parse_timing(report: &str, clock_hz: u64) -> Result<KernelTiming> {
    let requested_mhz = clock_hz / 1_000_000;
    if requested_mhz == 0 || requested_mhz > u64::from(u32::MAX) {
        return Err(XilinxError::InvalidFrequency(format!(
            "invalid SLASH clock `{clock_hz}` Hz"
        )));
    }
    let requested_mhz = f64::from(u32::try_from(requested_mhz).expect("checked above"));
    let row = Regex::new(
        r"(?m)WNS\(ns\)\s+TNS\(ns\).*?\n\s*-+\s*\n\s*([-+0-9.]+)\s+([-+0-9.]+)\s+\d+\s+\d+\s+([-+0-9.]+)\s+([-+0-9.]+)",
    )
    .expect("static SLASH timing expression");
    let captures = row.captures(report).ok_or_else(|| {
        XilinxError::TimingSummaryParse(
            "SLASH routed timing report has no WNS/TNS/WHS/THS summary".into(),
        )
    })?;
    let parse = |index: usize, name: &str| -> Result<f64> {
        let value = captures[index]
            .parse::<f64>()
            .map_err(|_| XilinxError::TimingSummaryParse(format!("invalid SLASH {name} value")))?;
        if value.is_finite() {
            Ok(value)
        } else {
            Err(XilinxError::TimingSummaryParse(format!(
                "SLASH {name} value is not finite"
            )))
        }
    };
    let wns = parse(1, "WNS")?;
    let tns = parse(2, "TNS")?;
    let whs = parse(3, "WHS")?;
    let ths = parse(4, "THS")?;
    if [wns, tns, whs, ths].iter().any(|value| *value < 0.0) {
        return Err(XilinxError::TimingSummaryParse(format!(
            "SLASH timing failed at {requested_mhz:.0} MHz: WNS={wns}, TNS={tns}, WHS={whs}, THS={ths}"
        )));
    }
    let period = 1000.0 / requested_mhz;
    let achieved_period = period - wns;
    if achieved_period <= 0.0 {
        return Err(XilinxError::TimingSummaryParse(
            "SLASH achieved period is not positive".into(),
        ));
    }
    Ok(KernelTiming {
        reported_target_period_ns: period,
        reported_target_mhz: requested_mhz,
        wns_ns: wns,
        achieved_period_ns: achieved_period,
        fmax_mhz: 1000.0 / achieved_period,
    })
}
