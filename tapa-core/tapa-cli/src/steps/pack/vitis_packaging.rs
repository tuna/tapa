//! Vitis (`xilinx-vitis`) packaging path for `tapa pack`.
//!
//! Holds [`pack_vitis`] which projects the top task's external ports
//! into a [`PackageXoInputs`] block and drives `tapa_xilinx::pack_xo`
//! against `<work_dir>/rtl` to produce the `.xo`. The runner picks
//! between local and remote dispatch based on `ctx.remote_config`.
//!
//! Also threads the `--custom-rtl` overlay via
//! [`super::custom_rtl::apply_custom_rtl`] *before* Vivado scans
//! `rtl_dir`.

use camino::Utf8PathBuf;
use std::path::{Path, PathBuf};
use tapa_ir::{ClockPeriod, Design, WorkState};
use tapa_xilinx::{
    emit_kernel_xml, emit_slash_hls_data, normalize_hls_report_text, pack_ip,
    pack_xo as xilinx_pack_xo, DeviceInfo, KernelXmlArgs, PackageXoInputs, ToolRunner,
};

use crate::context::CliContext;
use crate::error::{CliError, Result};
use crate::state::work::FlowSettings;

use super::custom_rtl::{apply_custom_rtl, load_templates_info, warn_unimplemented_templates};
use super::kernel_xml_ports::{build_kernel_xml_ports, m_axi_param_block};
use super::{enforce_xo_suffix, enforce_zip_suffix, PackArgs};

pub(super) fn pack_vitis(args: &PackArgs, ctx: &CliContext, state: &WorkState) -> Result<()> {
    let design = &state.graph;
    // Keep state validation ahead of custom-RTL overlays: malformed persisted
    // state must not mutate the canonical RTL tree before packaging fails.
    resolve_top_task(design)?;
    resolve_device_settings(&ctx.work_dir, &state.flow)?;

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
    write_aie_sidecars(&ctx.work_dir, design, state.flow.skip_aie, &output_path)?;

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
    let inputs = prepare_package_inputs(state, rtl_dir, xo_path, report_work_dir)?;
    Ok(xilinx_pack_xo(runner, &inputs)?)
}

/// Package SLASH directly from the prepared RTL, stopping Vivado after the
/// IP-XACT core is saved rather than manufacturing an intermediate XO.
pub(super) fn pack_slash(args: &PackArgs, ctx: &CliContext, state: &WorkState) -> Result<()> {
    validate_slash_top(state)?;
    let hdl_dir = ctx.work_dir.join("rtl");
    apply_pack_overlays(args, ctx, &hdl_dir)?;
    let output_path = enforce_zip_suffix(args.output.as_ref());
    ctx.with_tool_runner(|runner| {
        package_prepared_slash_zip(runner, state, &hdl_dir, &output_path, &ctx.work_dir)
    })?;
    Ok(())
}

/// Package an already prepared RTL tree with the K5 SLASH ZIP layout.
///
/// `report_work_dir` is read-only and supplies the synthesis report. This is
/// also the floorplan-candidate entry point: candidates must never fall back
/// to an XO merely because both flows share the IP-XACT Vivado helper.
pub fn package_prepared_slash_zip(
    runner: &dyn ToolRunner,
    state: &WorkState,
    rtl_dir: &Path,
    zip_path: &Path,
    report_work_dir: &Path,
) -> Result<Utf8PathBuf> {
    validate_slash_top(state)?;
    if !rtl_dir.is_dir() {
        return Err(CliError::MissingState {
            name: "RTL directory (run `tapa synth` first)".into(),
            path: rtl_dir.to_path_buf(),
        });
    }
    let report_yaml = report_work_dir.join("report.yaml");
    if !report_yaml.is_file() {
        return Err(CliError::MissingState {
            name: "TAPA report (run `tapa synth` first)".into(),
            path: report_yaml,
        });
    }
    if zip_path.extension().and_then(std::ffi::OsStr::to_str) != Some("zip") {
        return Err(CliError::InvalidArg(format!(
            "SLASH package output `{}` must use the `.zip` extension",
            zip_path.display()
        )));
    }

    let staging = tempfile::tempdir()?;
    let unused_xo = staging.path().join("unused.xo");
    let inputs = prepare_package_inputs(state, rtl_dir, &unused_xo, None)?;
    let hls_data = emit_slash_hls_data(&inputs.kernel_xml)?;
    let kernel_xml = emit_kernel_xml(&inputs.kernel_xml)?;
    let ip_dir = staging.path().join("impl/ip");
    fs_err::create_dir_all(ip_dir.parent().expect("IP parent"))?;
    pack_ip(runner, &inputs, &crate::util::utf8(&ip_dir))?;
    write_slash_zip(
        zip_path,
        &ip_dir,
        &hls_data,
        &kernel_xml,
        report_work_dir,
        state,
    )?;
    Ok(crate::util::utf8(zip_path))
}

fn validate_slash_top(state: &WorkState) -> Result<()> {
    let top_task = resolve_top_task(&state.graph)?;
    let host_streams: Vec<&str> = top_task
        .ports
        .iter()
        .filter(|port| port.cat.is_stream())
        .map(|port| port.name.as_str())
        .collect();
    if !host_streams.is_empty() {
        return Err(CliError::InvalidArg(format!(
            "the xilinx-slash target does not support host streams at pack time; remove top-level stream port(s) {}",
            host_streams.join(", ")
        )));
    }
    if !top_task
        .ports
        .iter()
        .any(|port| port.cat.is_scalar() || port.cat.is_mmap_like())
    {
        return Err(CliError::InvalidArg(
            "the xilinx-slash target requires the ap_ctrl_hs control protocol; the top task must have a scalar or mmap argument"
                .into(),
        ));
    }
    Ok(())
}

fn prepare_package_inputs(
    state: &WorkState,
    rtl_dir: &Path,
    output_path: &Path,
    report_work_dir: Option<&Path>,
) -> Result<PackageXoInputs> {
    let design = &crate::aie::pl_graph(&state.graph)?;
    let flow = &state.flow;
    let top_task = resolve_top_task(design)?;
    let state_root = report_work_dir
        .or_else(|| rtl_dir.parent())
        .unwrap_or(rtl_dir);
    let (part_num, clock_period) = resolve_device_settings(state_root, flow)?;
    let top_m_axi_bases = top_rtl_m_axi_bases(rtl_dir, &design.top)?;
    let kernel_ports = build_kernel_xml_ports(&top_task.ports, &top_m_axi_bases);
    if kernel_ports.is_empty() {
        return Err(CliError::InvalidArg(format!(
            "top task `{}` has no external ports; cannot emit kernel.xml",
            design.top
        )));
    }
    let report_paths = report_work_dir
        .map(collect_hls_report_paths)
        .transpose()?
        .unwrap_or_default();
    Ok(build_package_xo_inputs(
        design,
        rtl_dir,
        output_path,
        part_num,
        clock_period,
        kernel_ports,
        m_axi_param_block(&top_task.ports, &top_m_axi_bases),
        report_paths,
    ))
}

fn write_slash_zip(
    output_path: &Path,
    ip_dir: &Path,
    hls_data: &str,
    kernel_xml: &str,
    work_dir: &Path,
    state: &WorkState,
) -> Result<()> {
    use path_slash::PathExt as _;

    if let Some(parent) = output_path.parent() {
        if !parent.as_os_str().is_empty() {
            fs_err::create_dir_all(parent)?;
        }
    }
    let file = fs_err::File::create(output_path)?;
    let mut zip = zip::ZipWriter::new(std::io::BufWriter::new(file));
    let options: zip::write::FileOptions<'_, '_, ()> =
        zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let mut ip_files = Vec::new();
    for entry in walkdir::WalkDir::new(ip_dir) {
        let entry = entry.map_err(std::io::Error::other)?;
        if entry.file_type().is_file() {
            ip_files.push(entry.into_path());
        }
    }
    ip_files.sort();
    for path in ip_files {
        let relative = path.strip_prefix(ip_dir).map_err(|_| {
            CliError::Archive(format!(
                "IP file `{}` is not under `{}`",
                path.display(),
                ip_dir.display()
            ))
        })?;
        let name = format!("impl/ip/{}", relative.to_slash_lossy());
        super::add_zip_entry(&mut zip, options, &name, &fs_err::read(path)?)?;
    }
    super::add_zip_entry(&mut zip, options, "hls_data.json", hls_data.as_bytes())?;
    super::add_zip_entry(&mut zip, options, "kernel.xml", kernel_xml.as_bytes())?;
    super::add_zip_entry(
        &mut zip,
        options,
        "report.yaml",
        &fs_err::read(work_dir.join("report.yaml"))?,
    )?;
    super::add_zip_entry(
        &mut zip,
        options,
        crate::state::work::FILE_NAME,
        &crate::state::work::to_bytes(&super::stamp_cosim_port_metadata(state))?,
    )?;
    zip.finish()
        .map_err(|error| CliError::Archive(format!("cannot finalize the archive: {error}")))?;
    Ok(())
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
    let templates = load_templates_info(&ctx.work_dir)?;
    if !args.custom_rtl.is_empty() {
        apply_custom_rtl(hdl_dir, &args.custom_rtl, &templates)?;
    }
    warn_unimplemented_templates(&ctx.work_dir, hdl_dir, &templates);
    Ok(())
}

// The one M-AXI suffix vocabulary lives in tapa-protocol; a private
// copy here once drifted from it (`_AWREGION`/`_ARREGION`).
use tapa_protocol::m_axi_suffixes;

fn top_rtl_m_axi_bases(
    hdl_dir: &Path,
    top_name: &str,
) -> Result<std::collections::BTreeSet<String>> {
    let rtl_path = hdl_dir.join(format!("{top_name}.v"));
    let source = std::fs::read_to_string(&rtl_path).map_err(|e| {
        CliError::Codegen(format!(
            "cannot read top RTL `{}` for kernel.xml port projection: {e}",
            rtl_path.display(),
        ))
    })?;
    let module = tapa_rtl::VerilogModule::parse(&source).map_err(|e| {
        CliError::Codegen(format!(
            "cannot parse top RTL `{}` for kernel.xml port projection: {e}",
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
        let base = m_axi_suffixes()
            .find_map(|suffix| rest.strip_suffix(suffix))
            .ok_or_else(|| {
                CliError::Codegen(format!(
                    "top RTL `{}` has M-AXI port `{}`, whose suffix is not in \
                     tapa_protocol::m_axi_suffixes; add it there so every \
                     reader of the vocabulary agrees",
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
        // resolve_device_settings already rejected a missing clock period,
        // so the validated one is the one the kernel XML records.
        .kernel_xml(KernelXmlArgs {
            top_name: design.top.clone(),
            clock_period: clock_period.to_string(),
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
    let state_path = work_dir.join(crate::state::work::FILE_NAME);
    if !state_path.is_file() {
        return Err(CliError::MissingState {
            name: "typed flow state for the link package".into(),
            path: state_path,
        });
    }
    reports.push((
        crate::util::utf8(&state_path),
        crate::state::work::FILE_NAME.to_owned(),
    ));
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
    let mut out = normalize_hls_report_text(text);
    for base in [abs_work_dir.as_path(), work_dir] {
        let base = base.to_string_lossy();
        if let Some(stripped) = base.strip_prefix('/') {
            replace_report_rewritten_prefixes(&mut out, stripped);
        }
        replace_report_rewritten_prefixes(&mut out, &base);
    }
    canonicalize_report_tables(&out)
}

/// Collapse `<work_dir>/rewritten/` source paths (absolute or
/// `../`-relative) to tree-relative `rewritten/…` so archives built in
/// different work dirs stay byte-identical.
fn replace_report_rewritten_prefixes(text: &mut String, base: &str) {
    if base.is_empty() {
        return;
    }
    let normalized = base.replace('\\', "/");
    let prefix = format!("{normalized}/rewritten/");
    if normalized.starts_with('/') {
        *text = text.replace(&prefix, "rewritten/");
    } else {
        for ups in (1..=8).rev() {
            let relative_prefix = format!("{}{}", "../".repeat(ups), prefix);
            *text = text.replace(&relative_prefix, "rewritten/");
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

/// Copy the AI Engine graph `tapa synth` compiled next to the `.xo` as
/// `<name>.libadf.a`, with its link connectivity as `<name>.aie.cfg`.
fn remove_aie_sidecars(xo: &Path) -> Result<()> {
    let (libadf, cfg) = crate::aie::sidecars(xo);
    if libadf.exists() {
        fs_err::remove_file(libadf)?;
    }
    if cfg.exists() {
        fs_err::remove_file(cfg)?;
    }
    Ok(())
}

fn write_aie_sidecars(
    work_dir: &Path,
    design: &Design,
    skip_aie: bool,
    xo: &Path,
) -> Result<Option<(PathBuf, PathBuf)>> {
    let Some(partition) = tapa_ir::AiePartition::derive(design)? else {
        remove_aie_sidecars(xo)?;
        return Ok(None);
    };
    let libadf = crate::aie::dir(work_dir, "hw").join("libadf.a");
    if !libadf.is_file() {
        remove_aie_sidecars(xo)?;
        if skip_aie {
            log::warn!(
                "no AI Engine graph to package (synthesized with `--skip-aie`): the .xo serves \
                 hybrid cosimulation only"
            );
            return Ok(None);
        }
        return Err(CliError::MissingState {
            name: "AI Engine graph (run `tapa synth` without `--skip-aie`)".to_string(),
            path: libadf.into(),
        });
    }
    let (libadf_out, cfg_out) = crate::aie::sidecars(xo);
    fs_err::copy(&libadf, &libadf_out)?;
    fs_err::write(&cfg_out, crate::aie::link_config(&design.top, &partition))?;
    log::info!(
        "AI Engine graph: {} and {}",
        libadf_out.display(),
        cfg_out.display()
    );
    Ok(Some((libadf_out, cfg_out)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read as _, Write as _};
    use tapa_xilinx::{MockToolRunner, ToolOutput};

    fn pl_only_state() -> WorkState {
        let mut state = crate::testutil::state_from_json(
            r#"{
                "schema_version": 3,
                "cflags": [], "top": "Top", "target": "xilinx-vitis",
                "tasks": {"Top": {"readable_name": "Top", "srcs": ["Top.cpp"], "include_dirs": [], "defines": [], "level": "upper",
                    "synth": "hls", "ports": [], "tasks": {}, "fifos": {}}}
            }"#,
        );
        state.flow.synthed = true;
        state
    }

    fn packaging_state() -> WorkState {
        let mut state = crate::testutil::state_from_json(
            r#"{
                "schema_version": 3,
                "cflags": [], "top": "Top", "target": "xilinx-vitis",
                "tasks": {"Top": {"readable_name": "Top", "srcs": ["Top.cpp"], "include_dirs": [], "defines": [], "level": "lower",
                    "synth": "hls",
                    "ports": [{"cat": "scalar", "name": "n", "type": "int", "width": 32}],
                    "tasks": {}, "fifos": {}}}
            }"#,
        );
        state.flow.synthed = true;
        state.flow.part_num = Some("xcvu37p-fsvh2892-2L-e".to_string());
        state.flow.clock_period = Some(ClockPeriod::from_picoseconds(3330));
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
        let options: zip::write::FileOptions<'_, '_, ()> = zip::write::FileOptions::default();
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

    fn aie_state(skip_aie: bool) -> WorkState {
        let mut state = crate::testutil::state_from_json(
            r#"{
                "schema_version": 3,
                "cflags": [], "top": "Top", "target": "xilinx-vitis",
                "tasks": {
                    "Top": {"readable_name": "Top", "srcs": ["Top.cpp"], "include_dirs": [], "defines": [], "level": "upper",
                        "synth": "hls", "ports": [{"cat": "scalar", "name": "n", "type": "int", "width": 32}],
                        "tasks": {"Add": [{"args": {"n": {"arg": "n", "cat": "scalar"}}}]}, "fifos": {}},
                    "Add": {"readable_name": "Add", "srcs": ["Add.cpp"], "include_dirs": [], "defines": [], "level": "lower",
                        "synth": "aie", "ports": [{"cat": "scalar", "name": "n", "type": "int", "width": 32}],
                        "tasks": {}, "fifos": {}}
                }
            }"#,
        );
        state.flow.skip_aie = skip_aie;
        state
    }

    fn write_stale_sidecars(xo: &Path) {
        let (libadf, cfg) = crate::aie::sidecars(xo);
        fs_err::write(libadf, "stale graph").expect("write stale graph");
        fs_err::write(cfg, "stale connectivity").expect("write stale connectivity");
    }

    #[test]
    fn aie_sidecars_require_a_current_graph_and_remove_stale_outputs() {
        for skip_aie in [false, true] {
            let dir = tempfile::tempdir().expect("tempdir");
            let xo = dir.path().join("k.xo");
            write_stale_sidecars(&xo);
            let state = aie_state(skip_aie);
            let result = write_aie_sidecars(dir.path(), &state.graph, skip_aie, &xo);
            if skip_aie {
                assert!(result.expect("intentional skip").is_none());
            } else {
                assert!(matches!(result, Err(CliError::MissingState { .. })));
            }
            let (libadf, cfg) = crate::aie::sidecars(&xo);
            assert!(!libadf.exists() && !cfg.exists(), "stale sidecars remain");
        }

        let dir = tempfile::tempdir().expect("tempdir");
        let xo = dir.path().join("k.xo");
        write_stale_sidecars(&xo);
        let state = pl_only_state();
        assert!(write_aie_sidecars(dir.path(), &state.graph, false, &xo)
            .expect("PL-only package")
            .is_none());
        let (libadf, cfg) = crate::aie::sidecars(&xo);
        assert!(!libadf.exists() && !cfg.exists(), "stale sidecars remain");
    }

    #[test]
    fn aie_sidecars_copy_the_current_graph() {
        let dir = tempfile::tempdir().expect("tempdir");
        let graph_dir = dir.path().join("aie").join("hw");
        fs_err::create_dir_all(&graph_dir).expect("create graph directory");
        fs_err::write(graph_dir.join("libadf.a"), "current graph").expect("write graph");
        let xo = dir.path().join("k.xo");
        let state = aie_state(false);
        let (libadf, cfg) = write_aie_sidecars(dir.path(), &state.graph, false, &xo)
            .expect("package graph")
            .expect("sidecars");
        assert_eq!(
            fs_err::read_to_string(libadf).expect("read graph"),
            "current graph"
        );
        assert_eq!(
            fs_err::read_to_string(cfg).expect("read connectivity"),
            "[connectivity]
"
        );
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
        assert!(msg.contains("m_axi_suffixes"), "{msg}");
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
            .local_args()
            .iter()
            .any(|arg| arg == rtl_dir.to_str().unwrap()));
        assert!(calls[0]
            .local_args()
            .iter()
            .any(|arg| arg == xo_path.to_str().unwrap()));
    }

    #[test]
    fn normal_packaging_bundles_reports_when_requested() {
        let dir = tempfile::tempdir().expect("tempdir");
        let rtl_dir = write_prepared_rtl(dir.path());
        fs_err::write(dir.path().join("report.yaml"), "normal: report\n").expect("write report");
        fs_err::write(
            dir.path().join(crate::state::work::FILE_NAME),
            crate::state::work::to_bytes(&packaging_state()).expect("serialize state"),
        )
        .expect("write state");
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
        assert!(xo_entry(&xo_path, crate::state::work::FILE_NAME).is_some());
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
SOURCE=\"/home/tapa/execroot/_main/bazel-out/bin/tests/apps/vadd/vadd-xo.tapa/rewritten/Add.cpp:39\"\n\
| ../../home/tapa/execroot/_main/bazel-out/bin/tests/apps/vadd/vadd-xo.tapa/rewritten/Add.cpp:17 in add |\n";

        let sanitized = sanitize_hls_report_text(text, work_dir);
        assert!(!sanitized.contains("vadd-xo.tapa"));
        assert!(sanitized.contains("SOURCE=\"rewritten/Add.cpp:39\""));
        assert!(sanitized.contains("| rewritten/Add.cpp:17 in add |"));
    }

    #[test]
    fn sanitize_hls_report_text_removes_tool_runfiles_and_table_padding() {
        let work_dir = Path::new("/work/vadd-xo.tapa");
        let text = "\
+--------------+------------------------------------------------------------------------------------------------------------------------------------+\n\
| Location     | Access Location                                                                                                                    |\n\
+--------------+------------------------------------------------------------------------------------------------------------------------------------+\n\
| rewritten/Add.cpp  | /home/tapa/.cache/bazel/x/sandbox/processwrapper-sandbox/8697/execroot/_main/bazel-out/k8-opt-exec/bin/tapa/tapa.runfiles/_main/tapa-lib/tapa/xilinx/hls/stream.h:150:11     |\n\
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

    #[test]
    fn slash_zip_has_the_link_and_cosim_layout() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ip_dir = dir.path().join("unpacked/impl/ip");
        fs_err::create_dir_all(ip_dir.join("hdl")).expect("hdl");
        fs_err::create_dir_all(ip_dir.join("xgui")).expect("xgui");
        fs_err::write(ip_dir.join("component.xml"), "<component/>").expect("component");
        fs_err::write(ip_dir.join("hdl/Top.v"), "module Top; endmodule").expect("RTL");
        fs_err::write(ip_dir.join("xgui/Top.tcl"), "# xgui").expect("xgui");
        fs_err::write(dir.path().join("report.yaml"), "clock: 3.33\n").expect("report");
        let mut state = packaging_state();
        state.graph.target = tapa_ir::Target::XilinxSlash;
        let top = state.graph.tasks.get_mut("Top").expect("top");
        top.ports[0].cat = tapa_ir::ArgCategory::Mmap;
        top.ports[0].ctype = "int*".into();
        let output = dir.path().join("vadd.zip");

        write_slash_zip(
            &output,
            &ip_dir,
            "{\"Args\":{}}\n",
            "<root/>\n",
            dir.path(),
            &state,
        )
        .expect("write SLASH zip");

        let mut archive =
            zip::ZipArchive::new(fs_err::File::open(output).expect("open zip")).expect("parse zip");
        let names: Vec<String> = (0..archive.len())
            .map(|index| {
                archive
                    .by_index(index)
                    .expect("entry")
                    .name()
                    .expect("name")
                    .into_owned()
            })
            .collect();
        assert_eq!(
            names,
            [
                "impl/ip/component.xml",
                "impl/ip/hdl/Top.v",
                "impl/ip/xgui/Top.tcl",
                "hls_data.json",
                "kernel.xml",
                "report.yaml",
                "tapa.json",
            ]
        );
        let mut packed_state = String::new();
        archive
            .by_name("tapa.json")
            .expect("state entry")
            .read_to_string(&mut packed_state)
            .expect("read state");
        let packed_state = WorkState::from_json(&packed_state).expect("parse packed state");
        assert_eq!(packed_state.graph.target, tapa_ir::Target::XilinxSlash);
        assert_eq!(
            packed_state.graph.tasks["Top"].ports[0].mmap_addr_width,
            Some(64),
            "the state entry carries the cosim mmap metadata"
        );
    }

    #[test]
    fn slash_pack_rejects_host_streams_and_non_handshake_tops() {
        let dir = tempfile::tempdir().expect("tempdir");
        let args = PackArgs {
            output: None,
            custom_rtl: vec![],
        };
        let ctx = crate::testutil::ctx_at(dir.path());
        let mut state = packaging_state();
        state.graph.target = tapa_ir::Target::XilinxSlash;
        let mut stream = state.graph.tasks["Top"].ports[0].clone();
        stream.name = "input".into();
        stream.cat = tapa_ir::ArgCategory::Istream;
        state.graph.tasks.get_mut("Top").unwrap().ports.push(stream);
        let error = pack_slash(&args, &ctx, &state).expect_err("host stream must fail");
        assert!(error.to_string().contains("host streams"), "{error}");
        assert!(error.to_string().contains("input"), "{error}");

        state.graph.tasks.get_mut("Top").unwrap().ports.clear();
        let error = pack_slash(&args, &ctx, &state).expect_err("ap_ctrl_none must fail");
        assert!(error.to_string().contains("ap_ctrl_hs"), "{error}");
    }
}
