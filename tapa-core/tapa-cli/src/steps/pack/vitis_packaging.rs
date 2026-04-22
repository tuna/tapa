//! Vitis (`xilinx-vitis`) packaging path for `tapa pack`.
//!
//! Holds [`pack_vitis`] which projects the top task's external ports
//! into a [`PackageXoInputs`] block and drives `tapa_xilinx::pack_xo`
//! against `<work_dir>/rtl` to produce the `.xo`. The runner picks
//! between local and remote dispatch based on `ctx.remote_config`.
//!
//! Also threads the three CLI-surface overlays:
//!
//! * `--custom-rtl` overlays via [`super::custom_rtl::apply_custom_rtl`]
//!   *before* Vivado scans `rtl_dir`.
//! * `--graphir-path` embedding via
//!   [`super::graphir_embed::embed_graphir`] *before* Vivado scans
//!   `rtl_dir` (so graphir-derived modules ship alongside the
//!   TAPA-generated ones).
//! * `--bitstream-script` emission via
//!   [`super::bitstream_script::write_vitis_script`] *after* the
//!   `.xo` is on disk, so the script points at a real artifact.

use camino::Utf8PathBuf;
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;
use tapa_task_graph::Design;
use tapa_xilinx::{
    pack_xo as xilinx_pack_xo, DeviceInfo, KernelXmlArgs, LocalToolRunner, PackageXoInputs,
    RemoteToolRunner, SshMuxOptions, SshSession,
};

use crate::context::CliContext;
use crate::error::{CliError, Result};
use crate::state::settings as settings_io;

use super::bitstream_script::write_vitis_script;
use super::custom_rtl::{apply_custom_rtl, load_templates_info};
use super::graphir_embed::embed_graphir;
use super::kernel_xml_ports::{build_kernel_xml_ports_for_rtl, m_axi_param_block_for_rtl};
use super::{enforce_xo_suffix, PackArgs};

pub(super) fn pack_vitis(
    args: &PackArgs,
    ctx: &CliContext,
    design: &Design,
    settings: &settings_io::Settings,
) -> Result<()> {
    let (part_num, clock_period) = resolve_device_settings(settings)?;
    let top_task = design.tasks.get(&design.top).ok_or_else(|| {
        CliError::InvalidArg(format!(
            "design.json does not contain the top task `{}`",
            design.top
        ))
    })?;

    let hdl_dir = ctx.work_dir.join("rtl");
    if !hdl_dir.is_dir() {
        return Err(CliError::InvalidArg(format!(
            "RTL directory `{}` does not exist; run `tapa synth` first \
             (or chain `tapa analyze synth pack` in one invocation) to \
             populate the RTL tree before pack runs.",
            hdl_dir.display(),
        )));
    }

    apply_pack_overlays(args, ctx, &hdl_dir)?;

    let top_m_axi_bases = top_rtl_m_axi_bases(&hdl_dir, &design.top)?;
    let kernel_ports = build_kernel_xml_ports_for_rtl(&top_task.ports, &top_m_axi_bases);
    if kernel_ports.is_empty() {
        return Err(CliError::InvalidArg(format!(
            "top task `{}` has no external ports; cannot emit kernel.xml",
            design.top,
        )));
    }
    let output_path = enforce_xo_suffix(args.output.as_ref());
    let inputs = build_package_xo_inputs(
        design,
        settings,
        &hdl_dir,
        &output_path,
        part_num,
        clock_period,
        kernel_ports,
        m_axi_param_block_for_rtl(&top_task.ports, &top_m_axi_bases),
        collect_hls_report_paths(&ctx.work_dir)?,
    );

    run_pack_xo(ctx, &inputs)?;

    // --bitstream-script: emit helper pointing at the just-packaged
    // `.xo`. Done after pack so the script text references a real
    // artifact path (did the same).
    if let Some(script_dest) = args.bitstream_script.as_deref() {
        emit_bitstream_script(settings, script_dest, &design.top, &output_path)?;
    }

    let mut flow = ctx.flow.borrow_mut();
    flow.pipelined.insert("pack".to_string(), true);
    drop(flow);

    Ok(())
}

fn resolve_device_settings(settings: &settings_io::Settings) -> Result<(String, String)> {
    let part_num = settings
        .get("part_num")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            CliError::InvalidArg(
                "settings.json is missing `part_num`; run `synth` first to populate it."
                    .to_string(),
            )
        })?
        .to_string();
    let clock_period = settings
        .get("clock_period")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            CliError::InvalidArg(
                "settings.json is missing `clock_period`; run `synth` first to populate it."
                    .to_string(),
            )
        })?
        .to_string();
    Ok((part_num, clock_period))
}

fn apply_pack_overlays(args: &PackArgs, ctx: &CliContext, hdl_dir: &Path) -> Result<()> {
    // --custom-rtl: apply user overlays before Vivado scans `rtl_dir`.
    if !args.custom_rtl.is_empty() {
        let templates = load_templates_info(&ctx.work_dir)?;
        apply_custom_rtl(hdl_dir, &args.custom_rtl, &templates)?;
    }
    // --graphir-path: splice graphir-derived modules into `rtl_dir`.
    if let Some(graphir) = args.graphir_path.as_deref() {
        embed_graphir(&ctx.work_dir, hdl_dir, graphir)?;
    }
    Ok(())
}

const M_AXI_SUFFIXES: &[&str] = &[
    "_AWVALID",
    "_AWREADY",
    "_AWADDR",
    "_AWID",
    "_AWLEN",
    "_AWSIZE",
    "_AWBURST",
    "_AWLOCK",
    "_AWCACHE",
    "_AWPROT",
    "_AWQOS",
    "_AWREGION",
    "_WVALID",
    "_WREADY",
    "_WDATA",
    "_WSTRB",
    "_WLAST",
    "_BVALID",
    "_BREADY",
    "_BID",
    "_BRESP",
    "_ARVALID",
    "_ARREADY",
    "_ARADDR",
    "_ARID",
    "_ARLEN",
    "_ARSIZE",
    "_ARBURST",
    "_ARLOCK",
    "_ARCACHE",
    "_ARPROT",
    "_ARQOS",
    "_ARREGION",
    "_RVALID",
    "_RREADY",
    "_RDATA",
    "_RLAST",
    "_RID",
    "_RRESP",
];

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
        CliError::InvalidArg(format!(
            "failed to read top RTL `{}` for kernel.xml port projection: {e}",
            rtl_path.display(),
        ))
    })?;
    let module = tapa_rtl::VerilogModule::parse(&source).map_err(|e| {
        CliError::InvalidArg(format!(
            "failed to parse top RTL `{}` for kernel.xml port projection: {e}",
            rtl_path.display(),
        ))
    })?;
    let mut out = std::collections::BTreeSet::new();
    for port in &module.ports {
        let Some(rest) = port.name.strip_prefix("m_axi_") else {
            continue;
        };
        for suffix in M_AXI_SUFFIXES {
            if let Some(base) = rest.strip_suffix(suffix) {
                out.insert(base.to_owned());
                break;
            }
        }
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
    settings: &settings_io::Settings,
    hdl_dir: &Path,
    output_path: &Path,
    part_num: String,
    clock_period: String,
    kernel_ports: Vec<tapa_xilinx::KernelXmlPort>,
    m_axi_params: Vec<(String, Vec<(String, String)>)>,
    report_paths: Vec<(Utf8PathBuf, String)>,
) -> PackageXoInputs {
    PackageXoInputs::builder()
        .top_name(design.top.clone())
        .hdl_dir(Utf8PathBuf::from_path_buf(hdl_dir.to_path_buf()).unwrap_or_else(|p| Utf8PathBuf::from(p.to_string_lossy().into_owned())))
        .device_info(DeviceInfo {
            part_num,
            clock_period: clock_period.clone(),
        })
        .clock_period(clock_period)
        .kernel_xml(KernelXmlArgs {
            top_name: design.top.clone(),
            clock_period: settings
                .get("clock_period")
                .and_then(Value::as_str)
                .unwrap_or("3.33")
                .to_string(),
            ports: kernel_ports,
        })
        .kernel_out_path(Utf8PathBuf::from_path_buf(output_path.to_path_buf()).unwrap_or_else(|p| Utf8PathBuf::from(p.to_string_lossy().into_owned())))
        .m_axi_params(m_axi_params)
        .report_paths(report_paths)
        .build()
}

/// Collect the HLS reports that `PackageXo.__init__`
/// bundles into the `.xo` under `report/`. Walks
/// `<work_dir>/hls/<task>/report/` for `*_csynth.xml` (the primary
/// schema downstream tooling reads) plus any `.rpt` sibling files.
/// Returns `(source, archive_name)` pairs so the bundler can keep
/// the per-task layout — without the task subdir, multiple tasks'
/// `csynth.rpt` / `csynth.xml` files would collapse into a single
/// archive entry and overwrite each other.
fn collect_hls_report_paths(work_dir: &Path) -> Result<Vec<(Utf8PathBuf, String)>> {
    let hls_root = work_dir.join("hls");
    let staged_root = work_dir.join("pack_reports");
    if staged_root.exists() {
        fs_err::remove_dir_all(&staged_root)?;
    }
    let mut reports = Vec::<(Utf8PathBuf, String)>::new();
    for file in ["report.json", "report.yaml"] {
        let path = work_dir.join(file);
        if path.is_file() {
            reports.push((Utf8PathBuf::from_path_buf(path).unwrap_or_else(|p| Utf8PathBuf::from(p.to_string_lossy().into_owned())), file.to_owned()));
        }
    }
    if !hls_root.is_dir() {
        return Ok(reports);
    }
    let Ok(task_dirs) = fs_err::read_dir(&hls_root) else {
        return Ok(reports);
    };
    for task_entry in task_dirs.flatten() {
        let task_dir = task_entry.path();
        let report_dir = task_dir.join("report");
        if !report_dir.is_dir() {
            continue;
        }
        let Some(task_name) = task_dir
            .file_name()
            .and_then(|s| s.to_str())
            .map(str::to_owned)
        else {
            continue;
        };
        let Ok(entries) = fs_err::read_dir(&report_dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(ext) = path.extension().and_then(|s| s.to_str()) else {
                continue;
            };
            if !matches!(ext, "xml" | "rpt") {
                continue;
            }
            let Some(file) = path.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            let staged_path = staged_root.join(&task_name).join(file);
            if let Some(parent) = staged_path.parent() {
                fs_err::create_dir_all(parent)?;
            }
            let text = fs_err::read_to_string(&path)?;
            fs_err::write(&staged_path, sanitize_hls_report_text(&text, work_dir))?;
            let arcname = format!("report/{task_name}/{file}");
            reports.push((Utf8PathBuf::from_path_buf(staged_path).unwrap_or_else(|p| Utf8PathBuf::from(p.to_string_lossy().into_owned())), arcname));
        }
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

fn run_pack_xo(ctx: &CliContext, inputs: &PackageXoInputs) -> Result<Utf8PathBuf> {
    // Mirror synth: use RemoteToolRunner when ~/.taparc / --remote-host
    // is configured so the .xo packaging step actually runs on the
    // remote Xilinx host. Native pack used to always force
    // LocalToolRunner, ignoring `ctx.remote_config`.
    if let Some(cfg) = ctx.remote_config.as_ref() {
        let session = std::sync::Arc::new(SshSession::new(cfg.clone(), SshMuxOptions::default()));
        let runner = RemoteToolRunner::new(session);
        Ok(xilinx_pack_xo(&runner, inputs)?)
    } else {
        let runner = LocalToolRunner::new();
        Ok(xilinx_pack_xo(&runner, inputs)?)
    }
}

fn emit_bitstream_script(
    settings: &settings_io::Settings,
    script_dest: &Path,
    top: &str,
    output_path: &Path,
) -> Result<()> {
    let platform = settings.get("platform").and_then(Value::as_str);
    let clock = settings.get("clock_period").and_then(Value::as_str);
    let connectivity = settings
        .get("connectivity")
        .and_then(Value::as_str)
        .map(Path::new);
    write_vitis_script(script_dest, top, output_path, platform, clock, connectivity)?;
    log::info!("generate the v++ script at {}", script_dest.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
