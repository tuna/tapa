//! Vitis (`xilinx-vitis`) packaging path for `tapa pack`.
//!
//! Holds [`pack_vitis`] which projects the top task's external ports
//! into a [`PackageXoInputs`] block and drives `tapa_xilinx::pack_xo`
//! against `<work_dir>/rtl` to produce the `.xo`. The runner picks
//! between local and remote dispatch based on `ctx.remote_config`.

use serde_json::Value;
use tapa_task_graph::Design;
use tapa_xilinx::{
    pack_xo as xilinx_pack_xo, DeviceInfo, KernelXmlArgs, LocalToolRunner, PackageXoInputs,
    RemoteToolRunner, SshMuxOptions, SshSession,
};

use crate::context::CliContext;
use crate::error::{CliError, Result};
use crate::state::settings as settings_io;

use super::kernel_xml_ports::{build_kernel_xml_ports, m_axi_param_block};
use super::{enforce_xo_suffix, PackArgs};

pub(super) fn pack_vitis(
    args: &PackArgs,
    ctx: &CliContext,
    design: &Design,
    settings: &settings_io::Settings,
) -> Result<()> {
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

    let top_task = design.tasks.get(&design.top).ok_or_else(|| {
        CliError::InvalidArg(format!(
            "design.json does not contain the top task `{}`",
            design.top
        ))
    })?;

    let hdl_dir = ctx.work_dir.join("rtl");
    if !hdl_dir.is_dir() {
        return Err(CliError::InvalidArg(format!(
            "RTL directory `{}` does not exist; run `synth` first to generate it. \
             If you only need parity with the Python flow, rerun with \
             `TAPA_STEP_PACK_PYTHON=1`.",
            hdl_dir.display(),
        )));
    }

    let kernel_ports = build_kernel_xml_ports(&top_task.ports);
    if kernel_ports.is_empty() {
        return Err(CliError::InvalidArg(format!(
            "top task `{}` has no external ports; cannot emit kernel.xml",
            design.top,
        )));
    }
    let m_axi_params = m_axi_param_block(&top_task.ports);
    let output_path = enforce_xo_suffix(args.output.as_ref());

    let inputs = PackageXoInputs {
        top_name: design.top.clone(),
        hdl_dir,
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
        kernel_out_path: output_path,
        cpp_kernels: Vec::new(),
        m_axi_params,
        s_axi_ifaces: PackageXoInputs::default_s_axi(),
    };

    // Mirror synth: use RemoteToolRunner when ~/.taparc / --remote-host
    // is configured so the .xo packaging step actually runs on the
    // remote Xilinx host. Codex Round 2 finding: native pack used to
    // always force LocalToolRunner, ignoring `ctx.remote_config`.
    if let Some(cfg) = ctx.remote_config.as_ref() {
        let session = std::sync::Arc::new(SshSession::new(
            cfg.clone(),
            SshMuxOptions::default(),
        ));
        let runner = RemoteToolRunner::new(session);
        let _ = xilinx_pack_xo(&runner, &inputs)?;
    } else {
        let runner = LocalToolRunner::new();
        let _ = xilinx_pack_xo(&runner, &inputs)?;
    }

    let mut flow = ctx.flow.borrow_mut();
    flow.pipelined.insert("pack".to_string(), true);
    drop(flow);

    Ok(())
}
