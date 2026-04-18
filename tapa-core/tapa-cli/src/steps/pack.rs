//! `tapa pack` — native Rust port of `tapa/steps/pack.py`.
//!
//! Reloads `<work_dir>/{graph,design,settings}.json`, projects the top
//! task's external ports into a [`PackageXoInputs`] block, and drives
//! `tapa_xilinx::pack_xo` against `<work_dir>/rtl` to produce the
//! `.xo`. The Python bridge remains reachable behind
//! `TAPA_STEP_PACK_PYTHON=1` for parity with paths the native code
//! does not yet cover (HLS-target `.zip`, `--custom-rtl` overlays,
//! `GraphIR` embedding, the Vitis bitstream-script emission).

use std::path::PathBuf;

use clap::Parser;
use serde_json::Value;
use tapa_task_graph::{
    port::{ArgCategory, Port},
    Design,
};
use tapa_xilinx::{
    pack_xo as xilinx_pack_xo, DeviceInfo, KernelXmlArgs, KernelXmlPort, LocalToolRunner,
    PackageXoInputs, PortCategory,
};

use crate::context::CliContext;
use crate::error::{CliError, Result};
use crate::state::{design as design_io, settings as settings_io};
use crate::steps::python_bridge;

#[derive(Debug, Clone, Parser)]
#[command(name = "pack", about = "Pack the generated RTL into a Xilinx object file.")]
pub struct PackArgs {
    /// Output `.xo` (Vitis target) or `.zip` (HLS target).
    #[arg(short = 'o', long = "output", value_name = "FILE")]
    pub output: Option<PathBuf>,

    /// Bitstream-generation script path.
    #[arg(short = 's', long = "bitstream-script", value_name = "FILE")]
    pub bitstream_script: Option<PathBuf>,

    /// Custom RTL files / folders (may repeat).
    #[arg(long = "custom-rtl", value_name = "PATH")]
    pub custom_rtl: Vec<PathBuf>,

    /// `GraphIR` file to embed in the `.xo`.
    #[arg(long = "graphir-path", value_name = "FILE")]
    pub graphir_path: Option<PathBuf>,
}

pub fn to_python_argv(args: &PackArgs) -> Vec<String> {
    let mut out = Vec::<String>::new();
    if let Some(p) = &args.output {
        out.push("--output".to_string());
        out.push(p.display().to_string());
    }
    if let Some(p) = &args.bitstream_script {
        out.push("--bitstream-script".to_string());
        out.push(p.display().to_string());
    }
    for c in &args.custom_rtl {
        out.push("--custom-rtl".to_string());
        out.push(c.display().to_string());
    }
    if let Some(p) = &args.graphir_path {
        out.push("--graphir-path".to_string());
        out.push(p.display().to_string());
    }
    out
}

/// Top-level dispatcher: route to the Python bridge when explicitly
/// opted in, otherwise execute the native packaging path.
pub fn run(args: &PackArgs, ctx: &mut CliContext) -> Result<()> {
    if python_bridge::is_enabled("pack") {
        return python_bridge::run("pack", &to_python_argv(args), ctx);
    }
    run_native(args, ctx)
}

fn run_native(args: &PackArgs, ctx: &CliContext) -> Result<()> {
    reject_unsupported_flags(args)?;

    let design = design_io::load_design(&ctx.work_dir)?;
    let settings = settings_io::load_settings(&ctx.work_dir)?;
    let target = settings
        .get("target")
        .and_then(Value::as_str)
        .unwrap_or(&design.target)
        .to_string();

    match target.as_str() {
        "xilinx-vitis" => pack_vitis(args, ctx, &design, &settings),
        "xilinx-aie" => Ok(()),
        other => Err(CliError::InvalidArg(format!(
            "native pack only supports the `xilinx-vitis` target; got `{other}`. \
             Rerun with `TAPA_STEP_PACK_PYTHON=1` to use the Python fallback."
        ))),
    }
}

fn pack_vitis(
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

    let runner = LocalToolRunner::new();
    let _ = xilinx_pack_xo(&runner, &inputs)?;

    let mut flow = ctx.flow.borrow_mut();
    flow.pipelined.insert("pack".to_string(), true);
    drop(flow);

    Ok(())
}

/// Project a `tapa_task_graph::Port` list into the `KernelXmlPort`
/// shape `tapa_xilinx::emit_kernel_xml` expects. Mirrors the Python
/// `print_kernel_xml` logic in `tapa/verilog/xilinx/pack.py` plus the
/// `range_or_none` channel-fan-out unrolling.
fn build_kernel_xml_ports(ports: &[Port]) -> Vec<KernelXmlPort> {
    let mut out = Vec::<KernelXmlPort>::new();
    for port in ports {
        let chan_count = port.chan_count.unwrap_or(0);
        let names: Vec<String> = if chan_count == 0 {
            vec![port.name.clone()]
        } else {
            (0..chan_count)
                .map(|i| format!("{}_{i}", port.name))
                .collect()
        };
        let category = match port.cat {
            ArgCategory::Scalar => Some(PortCategory::Scalar),
            ArgCategory::Mmap | ArgCategory::Immap | ArgCategory::Ommap | ArgCategory::AsyncMmap => {
                Some(PortCategory::MAxi)
            }
            ArgCategory::Istream | ArgCategory::Istreams => Some(PortCategory::IStream),
            ArgCategory::Ostream | ArgCategory::Ostreams => Some(PortCategory::OStream),
        };
        let Some(cat) = category else { continue };
        for name in names {
            out.push(KernelXmlPort {
                name,
                category: cat,
                width: port.width,
                port: String::new(),
                ctype: port.ctype.clone(),
            });
        }
    }
    out
}

/// Python `pack` adds two bus parameters per `m_axi` port:
/// `HAS_BURST=0`, `SUPPORTS_NARROW_BURST=0`. Mirror that here so the
/// emitted `.xo` matches the Python output.
fn m_axi_param_block(ports: &[Port]) -> Vec<(String, Vec<(String, String)>)> {
    let mut out = Vec::<(String, Vec<(String, String)>)>::new();
    let kv = vec![
        ("HAS_BURST".to_string(), "0".to_string()),
        ("SUPPORTS_NARROW_BURST".to_string(), "0".to_string()),
    ];
    for port in ports {
        let is_mmap = matches!(
            port.cat,
            ArgCategory::Mmap | ArgCategory::Immap | ArgCategory::Ommap | ArgCategory::AsyncMmap
        );
        if !is_mmap {
            continue;
        }
        let chan_count = port.chan_count.unwrap_or(0);
        let names: Vec<String> = if chan_count == 0 {
            vec![port.name.clone()]
        } else {
            (0..chan_count)
                .map(|i| format!("{}_{i}", port.name))
                .collect()
        };
        for name in names {
            out.push((name, kv.clone()));
        }
    }
    out
}

fn reject_unsupported_flags(args: &PackArgs) -> Result<()> {
    if !args.custom_rtl.is_empty() {
        return Err(CliError::InvalidArg(
            "`--custom-rtl` overlay is not supported by the native packager; \
             rerun with `TAPA_STEP_PACK_PYTHON=1`."
                .to_string(),
        ));
    }
    if args.graphir_path.is_some() {
        return Err(CliError::InvalidArg(
            "`--graphir-path` embedding is not supported by the native packager; \
             rerun with `TAPA_STEP_PACK_PYTHON=1`."
                .to_string(),
        ));
    }
    if args.bitstream_script.is_some() {
        return Err(CliError::InvalidArg(
            "`--bitstream-script` v++ emission is not yet ported; \
             rerun with `TAPA_STEP_PACK_PYTHON=1`."
                .to_string(),
        ));
    }
    Ok(())
}

/// Match Python's `_enforce_path_suffix(...).xo`. When no `--output`
/// was provided, default to `work.xo` in the current directory.
fn enforce_xo_suffix(output: Option<&PathBuf>) -> PathBuf {
    match output {
        None => PathBuf::from("work.xo"),
        Some(p) => {
            if p.extension().and_then(|s| s.to_str()) == Some("xo") {
                p.clone()
            } else {
                let mut s = p.as_os_str().to_owned();
                s.push(".xo");
                PathBuf::from(s)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::similar_names,
        reason = "the `args`/`argv` pair appears throughout the dispatcher; \
                  matching the production names keeps tests legible"
    )]

    use super::*;

    use std::path::Path;

    use indexmap::IndexMap;
    use serde_json::json;
    use tapa_task_graph::TaskTopology;

    use crate::globals::GlobalArgs;

    fn parse_pack(extra: &[&str]) -> PackArgs {
        let mut argv = vec!["pack"];
        argv.extend_from_slice(extra);
        PackArgs::try_parse_from(argv).expect("parse pack args")
    }

    fn ctx_with_work_dir(work_dir: &Path) -> CliContext {
        let globals = GlobalArgs::try_parse_from([
            "tapa",
            "--work-dir",
            work_dir.to_str().expect("utf-8 work dir"),
        ])
        .expect("parse globals");
        CliContext::from_globals(&globals)
    }

    fn write_state(work_dir: &Path, target: &str) {
        std::fs::create_dir_all(work_dir).expect("mkdir work");
        let mut tasks = IndexMap::new();
        tasks.insert(
            "Top".to_string(),
            TaskTopology {
                name: "Top".to_string(),
                level: "upper".to_string(),
                code: "void Top() {}".to_string(),
                ports: vec![Port {
                    cat: ArgCategory::Mmap,
                    name: "gmem0".to_string(),
                    ctype: "int*".to_string(),
                    width: 512,
                    chan_count: None,
                    chan_size: None,
                }],
                tasks: IndexMap::new(),
                fifos: IndexMap::new(),
                target: Some("hls".to_string()),
                is_slot: false,
                self_area: IndexMap::new(),
                total_area: IndexMap::new(),
                clock_period: "3.33".to_string(),
            },
        );
        let design = Design {
            top: "Top".to_string(),
            target: target.to_string(),
            tasks,
            slot_task_name_to_fp_region: None,
        };
        design_io::store_design(work_dir, &design).expect("store design");
        let mut settings = settings_io::Settings::new();
        settings.insert("target".to_string(), json!(target));
        settings.insert("part_num".to_string(), json!("xcu250-figd2104-2L-e"));
        settings.insert("clock_period".to_string(), json!("3.33"));
        settings_io::store_settings(work_dir, &settings).expect("store settings");
    }

    #[test]
    fn argv_round_trips_python_shape() {
        let args = parse_pack(&["--output", "vadd.xo"]);
        let argv = to_python_argv(&args);
        assert!(argv.contains(&"--output".to_string()));
        assert!(argv.contains(&"vadd.xo".to_string()));
    }

    #[test]
    fn enforce_xo_suffix_appends_when_missing() {
        assert_eq!(enforce_xo_suffix(None), PathBuf::from("work.xo"));
        assert_eq!(
            enforce_xo_suffix(Some(&PathBuf::from("artifact"))),
            PathBuf::from("artifact.xo"),
        );
        assert_eq!(
            enforce_xo_suffix(Some(&PathBuf::from("ok.xo"))),
            PathBuf::from("ok.xo"),
        );
    }

    #[test]
    fn build_kernel_xml_ports_translates_categories() {
        let ports = vec![
            Port {
                cat: ArgCategory::Scalar,
                name: "n".into(),
                ctype: "int".into(),
                width: 32,
                chan_count: None,
                chan_size: None,
            },
            Port {
                cat: ArgCategory::Mmap,
                name: "gmem".into(),
                ctype: "int*".into(),
                width: 512,
                chan_count: None,
                chan_size: None,
            },
            Port {
                cat: ArgCategory::Istream,
                name: "i0".into(),
                ctype: "tapa::istream<int>".into(),
                width: 32,
                chan_count: None,
                chan_size: None,
            },
        ];
        let out = build_kernel_xml_ports(&ports);
        assert_eq!(out.len(), 3);
        assert!(matches!(out[0].category, PortCategory::Scalar));
        assert!(matches!(out[1].category, PortCategory::MAxi));
        assert!(matches!(out[2].category, PortCategory::IStream));
    }

    #[test]
    fn build_kernel_xml_ports_unrolls_chan_count() {
        let ports = vec![Port {
            cat: ArgCategory::Mmap,
            name: "gmem".into(),
            ctype: "int*".into(),
            width: 64,
            chan_count: Some(3),
            chan_size: None,
        }];
        let out = build_kernel_xml_ports(&ports);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].name, "gmem_0");
        assert_eq!(out[1].name, "gmem_1");
        assert_eq!(out[2].name, "gmem_2");
    }

    #[test]
    fn m_axi_param_block_emits_default_burst_params_for_mmap_only() {
        let ports = vec![
            Port {
                cat: ArgCategory::Scalar,
                name: "n".into(),
                ctype: "int".into(),
                width: 32,
                chan_count: None,
                chan_size: None,
            },
            Port {
                cat: ArgCategory::Mmap,
                name: "gmem".into(),
                ctype: "int*".into(),
                width: 512,
                chan_count: None,
                chan_size: None,
            },
        ];
        let block = m_axi_param_block(&ports);
        assert_eq!(block.len(), 1);
        assert_eq!(block[0].0, "gmem");
        assert!(block[0].1.iter().any(|(k, v)| k == "HAS_BURST" && v == "0"));
        assert!(block[0]
            .1
            .iter()
            .any(|(k, v)| k == "SUPPORTS_NARROW_BURST" && v == "0"));
    }

    #[test]
    fn unsupported_target_surfaces_invalid_arg() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_state(dir.path(), "xilinx-hls");
        let ctx = ctx_with_work_dir(dir.path());
        let err = run_native(&parse_pack(&[]), &ctx).expect_err("HLS target must reject");
        assert!(matches!(err, CliError::InvalidArg(ref m) if m.contains("xilinx-vitis")));
    }

    #[test]
    fn aie_target_is_a_no_op() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_state(dir.path(), "xilinx-aie");
        let ctx = ctx_with_work_dir(dir.path());
        run_native(&parse_pack(&[]), &ctx).expect("AIE pack is a no-op");
    }

    #[test]
    fn missing_rtl_dir_surfaces_invalid_arg() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_state(dir.path(), "xilinx-vitis");
        let ctx = ctx_with_work_dir(dir.path());
        let err = run_native(&parse_pack(&[]), &ctx).expect_err("missing rtl dir must fail");
        assert!(matches!(err, CliError::InvalidArg(ref m) if m.contains("rtl")));
    }

    #[test]
    fn custom_rtl_surfaces_invalid_arg() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_state(dir.path(), "xilinx-vitis");
        let ctx = ctx_with_work_dir(dir.path());
        let err = run_native(
            &parse_pack(&["--custom-rtl", "extra.v"]),
            &ctx,
        )
        .expect_err("custom-rtl must reject");
        assert!(matches!(err, CliError::InvalidArg(ref m) if m.contains("--custom-rtl")));
    }
}
