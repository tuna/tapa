//! Vitis (`xilinx-vitis`) packaging path for `tapa pack`.
//!
//! Holds [`pack_vitis`] which projects the top task's external ports
//! into a [`PackageXoInputs`] block and drives `tapa_xilinx::pack_xo`
//! against `<work_dir>/rtl` to produce the `.xo`. The runner picks
//! between local and remote dispatch based on `ctx.remote_config`.
//!
//! Also threads the two CLI-surface overlays:
//!
//! * `--custom-rtl` overlays via [`super::custom_rtl::apply_custom_rtl`]
//!   *before* Vivado scans `rtl_dir`.
//! * `--bitstream-script` emission via
//!   [`super::bitstream_script::write_vitis_script`] *after* the
//!   `.xo` is on disk, so the script points at a real artifact.

use camino::Utf8PathBuf;
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;
use tapa_ir::{ClockPeriod, Design, WorkState};
use tapa_xilinx::{
    pack_xo as xilinx_pack_xo, DeviceInfo, KernelXmlArgs, PackageXoInputs, ToolRunner,
};

use crate::context::CliContext;
use crate::error::{CliError, Result};
use crate::state::work::FlowSettings;

use super::bitstream_script::write_vitis_script;
use super::custom_rtl::{apply_custom_rtl, load_templates_info};
use super::kernel_xml_ports::{build_kernel_xml_ports_for_rtl, m_axi_param_block_for_rtl};
use super::{enforce_xo_suffix, PackArgs};

/// Fallback target clock for a kernel XML written before `synth` recorded
/// one: 3.33 ns, the historical default.
const DEFAULT_CLOCK_PERIOD: ClockPeriod = ClockPeriod::from_picoseconds(3330);

pub(super) fn pack_vitis(args: &PackArgs, ctx: &CliContext, state: &WorkState) -> Result<()> {
    let design = &state.graph;
    let flow = &state.flow;
    // Keep state validation ahead of custom-RTL overlays: malformed persisted
    // state must not mutate the canonical RTL tree before packaging fails.
    resolve_top_task(design)?;
    let link_inputs = resolve_link_inputs(&ctx.work_dir, state, args.connectivity.as_deref())?;
    resolve_device_settings(&ctx.work_dir, flow)?;

    let hdl_dir = ctx.work_dir.join("rtl");
    if !hdl_dir.is_dir() {
        return Err(CliError::MissingState {
            name: "RTL directory (run `tapa synth` first, or chain \
                 `tapa analyze synth pack` in one invocation)"
                .to_string(),
            path: hdl_dir,
        });
    }

    apply_pack_overlays(args, ctx, &hdl_dir)?;

    let output_path = enforce_xo_suffix(args.output.as_ref());
    ctx.with_tool_runner(|runner| {
        package_prepared_vitis_rtl(runner, state, &hdl_dir, &output_path, Some(&ctx.work_dir))
    })?;

    // Emit the bitstream helper after packaging so it points at the
    // completed `.xo`.
    if let Some(script_dest) = args.bitstream_script.as_deref() {
        emit_bitstream_script(flow, script_dest, &design.top, &output_path, &link_inputs)?;
    }

    Ok(())
}

/// Package an already prepared RTL tree into an explicit `.xo` path.
///
/// Passing `Some(work_dir)` preserves the normal `pack` behavior by bundling
/// its sanitized HLS reports. Passing `None` is the isolated-candidate path:
/// it reads only `state` and `rtl_dir`, and does not stage or modify anything
/// in a canonical work directory.
pub fn package_prepared_vitis_rtl(
    runner: &dyn ToolRunner,
    state: &WorkState,
    rtl_dir: &Path,
    xo_path: &Path,
    report_work_dir: Option<&Path>,
) -> Result<Utf8PathBuf> {
    let design = &state.graph;
    let flow = &state.flow;
    let top_task = resolve_top_task(design)?;
    let state_root = report_work_dir
        .or_else(|| rtl_dir.parent())
        .unwrap_or(rtl_dir);
    let (part_num, clock_period) = resolve_device_settings(state_root, flow)?;
    let top_m_axi_bases = top_rtl_m_axi_bases(rtl_dir, &design.top)?;
    let kernel_ports = build_kernel_xml_ports_for_rtl(&top_task.ports, &top_m_axi_bases);
    if kernel_ports.is_empty() {
        return Err(CliError::InvalidArg(format!(
            "top task `{}` has no external ports; cannot emit kernel.xml",
            design.top,
        )));
    }
    let report_paths = report_work_dir
        .map(collect_hls_report_paths)
        .transpose()?
        .unwrap_or_default();
    let inputs = build_package_xo_inputs(
        design,
        flow,
        rtl_dir,
        xo_path,
        part_num,
        clock_period,
        kernel_ports,
        m_axi_param_block_for_rtl(&top_task.ports, &top_m_axi_bases),
        report_paths,
    );

    Ok(xilinx_pack_xo(runner, &inputs)?)
}

fn resolve_top_task(design: &Design) -> Result<&tapa_ir::Task> {
    design.tasks.get(&design.top).ok_or_else(|| {
        CliError::Codegen(format!(
            "tapa.json does not contain the top task `{}`",
            design.top
        ))
    })
}

fn resolve_device_settings(work_dir: &Path, flow: &FlowSettings) -> Result<(String, ClockPeriod)> {
    let state_path = work_dir.join("tapa.json");
    let missing = |field: &str| CliError::MissingState {
        name: format!("`{field}` (run `synth` first to populate it)"),
        path: state_path.clone(),
    };
    let part_num = flow.part_num.clone().ok_or_else(|| missing("part_num"))?;
    let clock_period = flow.clock_period.ok_or_else(|| missing("clock_period"))?;
    // The type rules out everything but zero, which no device can run at.
    if clock_period == ClockPeriod::ZERO {
        return Err(CliError::InvalidArg(
            "synthesized target clock period is zero; rerun `tapa synth`".to_string(),
        ));
    }
    Ok((part_num, clock_period))
}

fn apply_pack_overlays(args: &PackArgs, ctx: &CliContext, hdl_dir: &Path) -> Result<()> {
    // --custom-rtl: apply user overlays before Vivado scans `rtl_dir`.
    if !args.custom_rtl.is_empty() {
        let templates = load_templates_info(&ctx.work_dir)?;
        apply_custom_rtl(hdl_dir, &args.custom_rtl, &templates)?;
    }
    Ok(())
}

// The one M-AXI suffix vocabulary lives in tapa-protocol; a private
// copy here once drifted from it (`_AWREGION`/`_ARREGION`).
use tapa_protocol::M_AXI_SUFFIXES;

static TAPA_LIB_RUNFILES_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?:(?:\.\./)*/?)[^\s"<>\|,]*tapa\.runfiles/_main/tapa-lib/"#).unwrap()
});
static HLS_XML_ROW_ID_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"<row id="\d+""#).unwrap());

fn top_rtl_m_axi_bases(
    hdl_dir: &Path,
    top_name: &str,
) -> Result<std::collections::BTreeSet<String>> {
    let rtl_path = hdl_dir.join(format!("{top_name}.v"));
    let source = std::fs::read_to_string(&rtl_path).map_err(|e| {
        CliError::Codegen(format!(
            "failed to read top RTL `{}` for kernel.xml port projection: {e}",
            rtl_path.display(),
        ))
    })?;
    let module = tapa_rtl::VerilogModule::parse(&source).map_err(|e| {
        CliError::Codegen(format!(
            "failed to parse top RTL `{}` for kernel.xml port projection: {e}",
            rtl_path.display(),
        ))
    })?;
    let mut out = std::collections::BTreeSet::new();
    for port in &module.ports {
        let Some(rest) = port.name.strip_prefix("m_axi_") else {
            continue;
        };
        // Every emitted M-AXI port has to be one the protocol crate knows:
        // an unrecognized one would drop out of the projection silently and
        // leave kernel.xml describing fewer interfaces than the RTL has.
        let base = M_AXI_SUFFIXES
            .iter()
            .find_map(|suffix| rest.strip_suffix(suffix))
            .ok_or_else(|| {
                CliError::Codegen(format!(
                    "top RTL `{}` has M-AXI port `{}`, whose suffix is not in                      tapa_protocol::M_AXI_SUFFIXES; add it there so every reader                      of the vocabulary agrees",
                    rtl_path.display(),
                    port.name,
                ))
            })?;
        out.insert(base.to_owned());
    }
    Ok(out)
}

#[allow(
    clippy::too_many_arguments,
    reason = "aggregating these into a struct would bounce values through \
              another builder without adding clarity"
)]
fn build_package_xo_inputs(
    design: &Design,
    flow: &FlowSettings,
    hdl_dir: &Path,
    output_path: &Path,
    part_num: String,
    clock_period: ClockPeriod,
    kernel_ports: Vec<tapa_xilinx::KernelXmlPort>,
    m_axi_params: Vec<(String, Vec<(String, String)>)>,
    report_paths: Vec<(Utf8PathBuf, String)>,
) -> PackageXoInputs {
    PackageXoInputs::builder()
        .top_name(design.top.clone())
        .hdl_dir(crate::util::utf8(hdl_dir))
        .device_info(DeviceInfo {
            part_num,
            clock_period: clock_period.to_string(),
        })
        .clock_period(clock_period.to_string())
        .kernel_xml(KernelXmlArgs {
            top_name: design.top.clone(),
            clock_period: flow
                .clock_period
                .unwrap_or(DEFAULT_CLOCK_PERIOD)
                .to_string(),
            ports: kernel_ports,
        })
        .kernel_out_path(crate::util::utf8(output_path))
        .m_axi_params(m_axi_params)
        .report_paths(report_paths)
        .build()
}

/// Collect HLS reports for the `.xo` under `report/`: the work-dir-level
/// `report.{json,yaml}` at the archive root, plus each task's `.xml` (the
/// primary schema downstream tooling reads) and `.rpt` files. The `.xml` /
/// `.rpt` sources are staged under `<work_dir>/pack_reports/` with work-dir
/// paths scrubbed by [`sanitize_hls_report_text`] before they are bundled;
/// [`super::collect_hls_reports`] owns the walk and the per-task
/// `report/<task>/` archive layout.
fn collect_hls_report_paths(work_dir: &Path) -> Result<Vec<(Utf8PathBuf, String)>> {
    let staged_root = work_dir.join("pack_reports");
    if staged_root.exists() {
        fs_err::remove_dir_all(&staged_root)?;
    }
    let mut reports = Vec::<(Utf8PathBuf, String)>::new();
    for file in ["report.json", "report.yaml"] {
        let path = work_dir.join(file);
        if path.is_file() {
            reports.push((crate::util::utf8(path), file.to_owned()));
        }
    }
    for (source, arcname) in super::collect_hls_reports(work_dir, |path| {
        matches!(
            path.extension().and_then(|s| s.to_str()),
            Some("xml" | "rpt")
        )
    })? {
        let task_rel = arcname
            .strip_prefix("report/")
            .expect("collect_hls_reports names are report/-prefixed");
        let staged_path = staged_root.join(task_rel);
        if let Some(parent) = staged_path.parent() {
            fs_err::create_dir_all(parent)?;
        }
        let text = fs_err::read_to_string(&source)?;
        fs_err::write(&staged_path, sanitize_hls_report_text(&text, work_dir))?;
        reports.push((crate::util::utf8(staged_path), arcname));
    }
    reports.sort();
    Ok(reports)
}

fn sanitize_hls_report_text(text: &str, work_dir: &Path) -> String {
    let abs_work_dir = fs_err::canonicalize(work_dir).unwrap_or_else(|_| work_dir.to_path_buf());
    let mut out = text.to_owned();
    for base in [abs_work_dir.as_path(), work_dir] {
        let base = base.to_string_lossy();
        if let Some(stripped) = base.strip_prefix('/') {
            replace_report_cpp_prefixes(&mut out, stripped);
        }
        replace_report_cpp_prefixes(&mut out, &base);
    }
    out = TAPA_LIB_RUNFILES_RE
        .replace_all(&out, "tapa-lib/")
        .into_owned();
    out = HLS_XML_ROW_ID_RE
        .replace_all(&out, r#"<row id="0""#)
        .into_owned();
    canonicalize_report_tables(&out)
}

fn replace_report_cpp_prefixes(text: &mut String, base: &str) {
    if base.is_empty() {
        return;
    }
    let normalized = base.replace('\\', "/");
    let prefix = format!("{normalized}/cpp/");
    if normalized.starts_with('/') {
        *text = text.replace(&prefix, "cpp/");
    } else {
        for ups in (1..=8).rev() {
            let relative_prefix = format!("{}{}", "../".repeat(ups), prefix);
            *text = text.replace(&relative_prefix, "cpp/");
        }
    }
}

fn canonicalize_report_tables(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        let trimmed = line.trim();
        if is_report_table_rule(trimmed) {
            let cols = trimmed.matches('+').count().saturating_sub(1).max(1);
            for _ in 0..cols {
                out.push_str("+---");
            }
            out.push_str("+\n");
        } else if trimmed.starts_with('|') && trimmed.ends_with('|') {
            let cells: Vec<_> = trimmed
                .trim_matches('|')
                .split('|')
                .map(str::trim)
                .collect();
            out.push('|');
            for cell in cells {
                out.push(' ');
                out.push_str(cell);
                out.push(' ');
                out.push('|');
            }
            out.push('\n');
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    if !text.ends_with('\n') {
        out.pop();
    }
    out
}

fn is_report_table_rule(line: &str) -> bool {
    !line.is_empty() && line.starts_with('+') && line.chars().all(|c| c == '+' || c == '-')
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LinkInputs {
    floorplan_xdc: Option<std::path::PathBuf>,
    connectivity: Option<std::path::PathBuf>,
}

/// Resolve the implementation inputs represented by the current work state.
///
/// A fixed staged connectivity file is part of a published floorplan. Once a
/// floorplan exists, `pack --connectivity` is only a consistency check: it may
/// repeat the same bytes, but it cannot silently change the memory topology
/// after placement and routing.
fn resolve_link_inputs(
    work_dir: &Path,
    state: &WorkState,
    connectivity_override: Option<&Path>,
) -> Result<LinkInputs> {
    let Some(floorplan_xdc) = super::published_floorplan_xdc(work_dir, state)? else {
        return Ok(LinkInputs {
            floorplan_xdc: None,
            connectivity: connectivity_override.map(Path::to_path_buf),
        });
    };

    let staged_connectivity = work_dir.join(crate::steps::floorplan::FLOORPLAN_CONNECTIVITY);
    let staged_connectivity = staged_connectivity.is_file().then_some(staged_connectivity);
    let has_direct_m_axi = state
        .graph
        .tasks
        .get(&state.graph.top)
        .is_some_and(|top| top.ports.iter().any(|port| port.cat.is_direct_mmap()));

    if has_direct_m_axi && staged_connectivity.is_none() {
        return Err(CliError::MissingState {
            name: "staged floorplan connectivity for direct top-level M-AXI ports (rerun `tapa \
                   floorplan --connectivity FILE`)"
                .to_string(),
            path: work_dir.join(crate::steps::floorplan::FLOORPLAN_CONNECTIVITY),
        });
    }

    if let Some(override_path) = connectivity_override {
        let Some(staged_path) = staged_connectivity.as_ref() else {
            return Err(CliError::InvalidArg(format!(
                "this floorplan was created without connectivity, so `{}` cannot be applied at \
                 pack time; rerun `tapa floorplan --connectivity {}`",
                override_path.display(),
                override_path.display(),
            )));
        };
        let staged_bytes = fs_err::read(staged_path)?;
        let override_bytes = fs_err::read(override_path).map_err(|error| {
            CliError::InvalidArg(format!(
                "cannot read connectivity override `{}`: {error}",
                override_path.display(),
            ))
        })?;
        if override_bytes != staged_bytes {
            return Err(CliError::InvalidArg(format!(
                "connectivity override `{}` differs from the configuration used by the active \
                 floorplan; omit the override or rerun `tapa floorplan --connectivity {}`",
                override_path.display(),
                override_path.display(),
            )));
        }
    }

    Ok(LinkInputs {
        floorplan_xdc: Some(floorplan_xdc),
        connectivity: staged_connectivity,
    })
}

fn emit_bitstream_script(
    flow: &FlowSettings,
    script_dest: &Path,
    top: &str,
    output_path: &Path,
    link_inputs: &LinkInputs,
) -> Result<()> {
    write_vitis_script(
        script_dest,
        top,
        output_path,
        flow.platform.as_deref(),
        flow.clock_period,
        link_inputs.floorplan_xdc.as_deref(),
        link_inputs.connectivity.as_deref(),
    )?;
    log::info!("generate the v++ script at {}", script_dest.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read as _, Write as _};
    use tapa_xilinx::{MockToolRunner, ToolOutput};

    fn link_state(floorplanned: bool, has_direct_m_axi: bool) -> WorkState {
        let ports = if has_direct_m_axi {
            r#"[{"cat":"mmap","name":"mem","type":"int*","width":32}]"#
        } else {
            "[]"
        };
        let mut state = crate::testutil::state_from_json(&format!(
            r#"{{
                "cflags": [], "top": "Top", "target": "xilinx-vitis",
                "tasks": {{"Top": {{"readable_name": "Top", "code": "", "level": "upper",
                    "synth": "hls", "ports": {ports}, "tasks": {{}}, "fifos": {{}}}}}}
            }}"#,
        ));
        state.flow.synthed = true;
        if floorplanned {
            state.floorplan = Some(crate::testutil::mock_floorplan_result("u280", (2, 3)));
        }
        state
    }

    fn write_published_floorplan(work_dir: &Path, connectivity: Option<&[u8]>) {
        fs_err::write(
            work_dir.join(crate::steps::floorplan::FLOORPLAN_XDC),
            "constraints",
        )
        .expect("write xdc");
        if let Some(bytes) = connectivity {
            fs_err::write(
                work_dir.join(crate::steps::floorplan::FLOORPLAN_CONNECTIVITY),
                bytes,
            )
            .expect("write connectivity");
        }
    }

    fn packaging_state() -> WorkState {
        let mut state = crate::testutil::state_from_json(
            r#"{
                "cflags": [], "top": "Top", "target": "xilinx-vitis",
                "tasks": {"Top": {"readable_name": "Top", "code": "", "level": "lower",
                    "synth": "hls",
                    "ports": [{"cat": "scalar", "name": "n", "type": "int", "width": 32}],
                    "tasks": {}, "fifos": {}}}
            }"#,
        );
        state.flow.synthed = true;
        state.flow.part_num = Some("xcvu37p-fsvh2892-2L-e".to_string());
        state.flow.clock_period = Some(DEFAULT_CLOCK_PERIOD);
        state
    }

    fn write_prepared_rtl(root: &Path) -> std::path::PathBuf {
        let rtl_dir = root.join("rtl");
        fs_err::create_dir_all(&rtl_dir).expect("create RTL directory");
        fs_err::write(
            rtl_dir.join("Top.v"),
            "module Top(input wire ap_clk, input wire [31:0] n); endmodule\n",
        )
        .expect("write top RTL");
        rtl_dir
    }

    fn mock_xo_bytes(root: &Path, name: &str) -> Vec<u8> {
        let path = root.join(name);
        let file = fs_err::File::create(&path).expect("create mock XO");
        let mut archive = zip::ZipWriter::new(file);
        let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
        archive
            .start_file("ip/stub.txt", options)
            .expect("start mock XO entry");
        archive.write_all(b"stub").expect("write mock XO entry");
        archive.finish().expect("finish mock XO");
        fs_err::read(path).expect("read mock XO")
    }

    fn xo_entry(path: &Path, name: &str) -> Option<Vec<u8>> {
        let bytes = fs_err::read(path).expect("read XO");
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("open XO");
        let mut entry = archive.by_name(name).ok()?;
        let mut out = Vec::new();
        entry.read_to_end(&mut out).expect("read XO entry");
        Some(out)
    }

    fn mock_pack_runner(root: &Path, xo_path: &Path, seed: &str) -> MockToolRunner {
        let runner = MockToolRunner::new();
        runner.push_ok("vivado", ToolOutput::default());
        runner.attach_download(crate::util::utf8(xo_path), mock_xo_bytes(root, seed));
        runner
    }

    /// The base projection reads the shared protocol suffix vocabulary;
    /// a base whose only surviving port carries the optional REGION
    /// attribute must still be recognized (a private copy of the table
    /// once disagreed with the shared one exactly here).
    #[test]
    fn m_axi_base_projection_recognizes_region_suffixes() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs_err::write(
            dir.path().join("Top.v"),
            "module Top(input [3:0] m_axi_a_ARREGION, output [3:0] m_axi_b_AWREGION); endmodule",
        )
        .expect("write rtl");
        let bases = top_rtl_m_axi_bases(dir.path(), "Top").expect("project bases");
        assert_eq!(
            bases.into_iter().collect::<Vec<_>>(),
            vec!["a".to_owned(), "b".to_owned()]
        );
    }

    /// An M-AXI port the shared vocabulary does not know is a drift signal,
    /// not something to skip: kernel.xml would describe fewer interfaces
    /// than the RTL actually has.
    #[test]
    fn an_unknown_m_axi_suffix_fails_the_projection() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs_err::write(
            dir.path().join("Top.v"),
            "module Top(input m_axi_a_ARVALID, input m_axi_a_ARFUTURE); endmodule",
        )
        .expect("write rtl");
        let err = top_rtl_m_axi_bases(dir.path(), "Top").expect_err("unknown suffix");
        let msg = err.to_string();
        assert!(msg.contains("m_axi_a_ARFUTURE"), "{msg}");
        assert!(msg.contains("M_AXI_SUFFIXES"), "{msg}");
    }

    #[test]
    fn candidate_packaging_uses_explicit_paths_and_omits_reports() {
        let dir = tempfile::tempdir().expect("tempdir");
        let rtl_dir = write_prepared_rtl(dir.path());
        fs_err::write(dir.path().join("report.yaml"), "candidate: report\n").expect("write report");
        let xo_path = dir.path().join("artifacts/candidate.xo");
        let runner = mock_pack_runner(dir.path(), &xo_path, "candidate-seed.xo");

        let output =
            package_prepared_vitis_rtl(&runner, &packaging_state(), &rtl_dir, &xo_path, None)
                .expect("package candidate");

        assert_eq!(output.as_std_path(), xo_path);
        assert!(xo_entry(&xo_path, "report.yaml").is_none());
        assert!(!dir.path().join("pack_reports").exists());
        let calls = runner.calls();
        assert_eq!(calls.len(), 1);
        assert!(calls[0]
            .args
            .iter()
            .any(|arg| arg == rtl_dir.to_str().unwrap()));
        assert!(calls[0]
            .args
            .iter()
            .any(|arg| arg == xo_path.to_str().unwrap()));
    }

    #[test]
    fn normal_packaging_bundles_reports_when_requested() {
        let dir = tempfile::tempdir().expect("tempdir");
        let rtl_dir = write_prepared_rtl(dir.path());
        fs_err::write(dir.path().join("report.yaml"), "normal: report\n").expect("write report");
        let xo_path = dir.path().join("normal.xo");
        let runner = mock_pack_runner(dir.path(), &xo_path, "normal-seed.xo");

        package_prepared_vitis_rtl(
            &runner,
            &packaging_state(),
            &rtl_dir,
            &xo_path,
            Some(dir.path()),
        )
        .expect("package normal output");

        assert_eq!(
            xo_entry(&xo_path, "report.yaml"),
            Some(b"normal: report\n".to_vec()),
        );
    }

    #[test]
    fn invalid_state_is_rejected_before_custom_rtl_overlay() {
        let dir = tempfile::tempdir().expect("tempdir");
        let rtl_dir = write_prepared_rtl(dir.path());
        let top_rtl = rtl_dir.join("Top.v");
        let original = fs_err::read(&top_rtl).expect("read original RTL");
        let replacement = dir.path().join("replacement.v");
        fs_err::write(&replacement, "module Top; endmodule\n").expect("write replacement");
        let mut state = packaging_state();
        state.graph.tasks.remove("Top");
        let args = PackArgs {
            output: None,
            bitstream_script: None,
            connectivity: None,
            custom_rtl: vec![replacement],
        };
        let ctx = crate::testutil::ctx_at(dir.path());

        let error = pack_vitis(&args, &ctx, &state)
            .expect_err("invalid state must fail before applying an RTL overlay");

        assert!(matches!(error, CliError::Codegen(_)));
        assert_eq!(
            fs_err::read(top_rtl).expect("read RTL after failure"),
            original
        );
    }

    #[test]
    fn active_floorplan_reuses_staged_connectivity() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_published_floorplan(dir.path(), Some(b"[connectivity]\n"));

        let inputs =
            resolve_link_inputs(dir.path(), &link_state(true, false), None).expect("resolve");
        assert_eq!(
            inputs.floorplan_xdc,
            Some(dir.path().join(crate::steps::floorplan::FLOORPLAN_XDC)),
        );
        assert_eq!(
            inputs.connectivity,
            Some(
                dir.path()
                    .join(crate::steps::floorplan::FLOORPLAN_CONNECTIVITY)
            ),
        );
    }

    #[test]
    fn floorplanned_connectivity_override_must_be_byte_identical() {
        let dir = tempfile::tempdir().expect("tempdir");
        let bytes = b"[connectivity]\nsp=Top.mem:HBM[0]\n";
        write_published_floorplan(dir.path(), Some(bytes));
        let same = dir.path().join("same.ini");
        let different = dir.path().join("different.ini");
        fs_err::write(&same, bytes).expect("write same");
        fs_err::write(&different, b"[connectivity]\nsp=Top.mem:HBM[1]\n").expect("write different");

        let inputs = resolve_link_inputs(dir.path(), &link_state(true, false), Some(&same))
            .expect("identical override");
        assert_eq!(
            inputs.connectivity,
            Some(
                dir.path()
                    .join(crate::steps::floorplan::FLOORPLAN_CONNECTIVITY)
            ),
            "the stable staged path is used even when an override is repeated",
        );

        let error = resolve_link_inputs(dir.path(), &link_state(true, false), Some(&different))
            .expect_err("different bytes must fail");
        assert!(matches!(error, CliError::InvalidArg(_)));
    }

    #[test]
    fn work_state_controls_floorplan_artifact_activation() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_published_floorplan(dir.path(), Some(b"stale"));

        let inactive =
            resolve_link_inputs(dir.path(), &link_state(false, false), None).expect("resolve");
        assert_eq!(
            inactive,
            LinkInputs {
                floorplan_xdc: None,
                connectivity: None,
            },
            "stale files cannot activate a floorplan absent from WorkState",
        );

        fs_err::remove_file(dir.path().join(crate::steps::floorplan::FLOORPLAN_XDC))
            .expect("remove xdc");
        let error = resolve_link_inputs(dir.path(), &link_state(true, false), None)
            .expect_err("an active state requires its publication marker");
        assert!(matches!(error, CliError::MissingState { .. }));
    }

    #[test]
    fn floorplan_without_staged_connectivity_rejects_pack_override() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_published_floorplan(dir.path(), None);
        let inputs = resolve_link_inputs(dir.path(), &link_state(true, false), None)
            .expect("a floorplan without memory interfaces needs no connectivity");
        assert!(inputs.connectivity.is_none());

        let override_path = dir.path().join("late.ini");
        fs_err::write(&override_path, "[connectivity]\n").expect("write override");

        let error = resolve_link_inputs(dir.path(), &link_state(true, false), Some(&override_path))
            .expect_err("pack cannot add connectivity after floorplanning");
        assert!(matches!(error, CliError::InvalidArg(_)));
    }

    #[test]
    fn floorplan_with_direct_m_axi_requires_staged_connectivity() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_published_floorplan(dir.path(), None);

        let error = resolve_link_inputs(dir.path(), &link_state(true, true), None)
            .expect_err("a direct M-AXI floorplan must publish its connectivity");
        assert!(
            matches!(error, CliError::MissingState { ref name, ref path }
                if name.contains("M-AXI")
                    && path.ends_with(crate::steps::floorplan::FLOORPLAN_CONNECTIVITY)),
            "got {error}",
        );
    }

    #[test]
    fn direct_m_axi_override_cannot_replace_missing_staged_connectivity() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_published_floorplan(dir.path(), None);
        let override_path = dir.path().join("late.ini");
        fs_err::write(&override_path, "[connectivity]\nsp=Top.mem:HBM[0]\n")
            .expect("write override");

        let error = resolve_link_inputs(dir.path(), &link_state(true, true), Some(&override_path))
            .expect_err("a late override cannot replace the staged floorplan input");

        assert!(
            matches!(error, CliError::MissingState { .. }),
            "got {error}"
        );
    }

    /// Negative, infinite, and non-numeric periods cannot reach here — the
    /// state file will not deserialize into a [`ClockPeriod`]. Zero is the
    /// one invalid value the type still admits.
    #[test]
    fn packaging_rejects_a_zero_persisted_clock_period() {
        let dir = tempfile::tempdir().expect("tempdir");
        let flow = FlowSettings {
            part_num: Some("xcvu37p-fsvh2892-2L-e".to_string()),
            clock_period: Some(ClockPeriod::ZERO),
            ..FlowSettings::default()
        };
        let error =
            resolve_device_settings(dir.path(), &flow).expect_err("a zero period must fail");
        assert!(
            matches!(error, CliError::InvalidArg(ref message)
                if message.contains("clock period") && message.contains("tapa synth")),
            "got {error}",
        );
    }

    #[test]
    fn sanitize_hls_report_text_removes_work_dir_paths() {
        let work_dir =
            Path::new("/home/tapa/execroot/_main/bazel-out/bin/tests/apps/vadd/vadd-xo.tapa");
        let text = "\
SOURCE=\"/home/tapa/execroot/_main/bazel-out/bin/tests/apps/vadd/vadd-xo.tapa/cpp/Add.cpp:39\"\n\
| ../../home/tapa/execroot/_main/bazel-out/bin/tests/apps/vadd/vadd-xo.tapa/cpp/Add.cpp:17 in add |\n";

        let sanitized = sanitize_hls_report_text(text, work_dir);
        assert!(!sanitized.contains("vadd-xo.tapa"));
        assert!(sanitized.contains("SOURCE=\"cpp/Add.cpp:39\""));
        assert!(sanitized.contains("| cpp/Add.cpp:17 in add |"));
    }

    #[test]
    fn sanitize_hls_report_text_removes_tool_runfiles_and_table_padding() {
        let work_dir = Path::new("/work/vadd-xo.tapa");
        let text = "\
+--------------+------------------------------------------------------------------------------------------------------------------------------------+\n\
| Location     | Access Location                                                                                                                    |\n\
+--------------+------------------------------------------------------------------------------------------------------------------------------------+\n\
| cpp/Add.cpp  | /home/tapa/.cache/bazel/x/sandbox/processwrapper-sandbox/8697/execroot/_main/bazel-out/k8-opt-exec/bin/tapa/tapa.runfiles/_main/tapa-lib/tapa/xilinx/hls/stream.h:150:11     |\n\
+--------------+------------------------------------------------------------------------------------------------------------------------------------+\n";

        let sanitized = sanitize_hls_report_text(text, work_dir);
        assert!(!sanitized.contains("processwrapper-sandbox"));
        assert!(sanitized.contains("tapa-lib/tapa/xilinx/hls/stream.h:150:11"));
        assert!(sanitized.contains("+---+---+"));
        assert!(sanitized.contains("| Location | Access Location |"));
    }

    #[test]
    fn sanitize_hls_report_text_normalizes_xml_row_ids() {
        let work_dir = Path::new("/work/vadd-xo.tapa");
        let text = r#"<row id="2" col0="operator&gt;&gt;">
  <row id="1" col0="read"/>
</row>
"#;

        let sanitized = sanitize_hls_report_text(text, work_dir);
        assert!(!sanitized.contains(r#"row id="1""#));
        assert!(!sanitized.contains(r#"row id="2""#));
        assert_eq!(sanitized.matches(r#"row id="0""#).count(), 2);
    }
}
