//! Vitis (`xilinx-vitis`) packaging path for `tapa pack`.
//!
//! Holds [`pack_vitis`] which projects the top task's external ports
//! into a [`PackageXoInputs`] block and drives `tapa_xilinx::pack_xo`
//! against `<work_dir>/rtl` to produce the `.xo`. The runner picks
//! between local and remote dispatch based on `ctx.remote_config`.
//!
//! Also threads the three click-surface overlays:
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

use std::path::{Path, PathBuf};

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
use super::kernel_xml_ports::{build_kernel_xml_ports, m_axi_param_block};
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

    let kernel_ports = build_kernel_xml_ports(&top_task.ports);
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
        m_axi_param_block(&top_task.ports),
    );

    run_pack_xo(ctx, &inputs)?;

    // --bitstream-script: emit helper pointing at the just-packaged
    // `.xo`. Done after pack so the script text references a real
    // artifact path (Python did the same).
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
) -> PackageXoInputs {
    PackageXoInputs {
        top_name: design.top.clone(),
        hdl_dir: hdl_dir.to_path_buf(),
        device_info: DeviceInfo {
            part_num,
            clock_period: clock_period.clone(),
        },
        clock_period,
        kernel_xml: KernelXmlArgs {
            top_name: design.top.clone(),
            clock_period: settings
                .get("clock_period")
                .and_then(Value::as_str)
                .unwrap_or("3.33")
                .to_string(),
            ports: kernel_ports,
        },
        kernel_out_path: output_path.to_path_buf(),
        cpp_kernels: Vec::new(),
        m_axi_params,
        s_axi_ifaces: PackageXoInputs::default_s_axi(),
    }
}

fn run_pack_xo(ctx: &CliContext, inputs: &PackageXoInputs) -> Result<PathBuf> {
    // Mirror synth: use RemoteToolRunner when ~/.taparc / --remote-host
    // is configured so the .xo packaging step actually runs on the
    // remote Xilinx host. Codex Round 2 finding: native pack used to
    // always force LocalToolRunner, ignoring `ctx.remote_config`.
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
