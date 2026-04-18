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

use crate::context::CliContext;
use crate::error::{CliError, Result};
use crate::state::{design as design_io, settings as settings_io};

mod kernel_xml_ports;
mod vitis_packaging;

use vitis_packaging::pack_vitis;

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

/// Top-level dispatcher.
///
/// Per AC-6, `TAPA_STEP_PACK_PYTHON=1` is a no-op for ported steps;
/// the native packaging path is the only path the dispatcher takes.
/// The bridge shim is kept only for composite forwarding of un-ported
/// step branches.
pub fn run(args: &PackArgs, ctx: &mut CliContext) -> Result<()> {
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
    use tapa_task_graph::{
        port::{ArgCategory, Port},
        Design, TaskTopology,
    };

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
