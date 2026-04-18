//! `tapa pack` — native Rust port of `tapa/steps/pack.py`.
//!
//! Reloads `<work_dir>/{graph,design,settings}.json`, projects the top
//! task's external ports into a [`PackageXoInputs`] block, and drives
//! `tapa_xilinx::pack_xo` against `<work_dir>/rtl` to produce the
//! `.xo`. Three optional overlays are applied around the core pack:
//!
//! * `--custom-rtl <PATH>` (may repeat) — validate user-supplied
//!   Verilog files against `<work_dir>/templates_info.json` and copy
//!   them into `<work_dir>/rtl` before Vivado runs.
//! * `--graphir-path <FILE>` — parse the `GraphIR` JSON, export it via
//!   `tapa-graphir-export`, and splice the generated Verilog into
//!   `<work_dir>/rtl` alongside the TAPA-generated modules.
//! * `--bitstream-script <FILE>` — after `.xo` emission, render the
//!   Python `get_vitis_script` helper and drop it at the requested
//!   path (executable on Unix).
//!
//! The HLS-target `.zip` packer is still unported and surfaces a
//! typed [`CliError::InvalidArg`].

use std::path::PathBuf;

use clap::Parser;
use serde_json::Value;

use crate::context::CliContext;
use crate::error::{CliError, Result};
use crate::state::{design as design_io, graph as graph_io, settings as settings_io};

mod bitstream_script;
mod custom_rtl;
mod graphir_embed;
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

/// Top-level dispatcher. Always runs the native packaging path (the
/// Python bridge target was retired in AC-8 / AC-6).
pub fn run(args: &PackArgs, ctx: &mut CliContext) -> Result<()> {
    run_native(args, ctx)
}

fn run_native(args: &PackArgs, ctx: &CliContext) -> Result<()> {
    let design = design_io::load_design(&ctx.work_dir)?;
    let settings = settings_io::load_settings(&ctx.work_dir)?;
    let target = settings
        .get("target")
        .and_then(Value::as_str)
        .unwrap_or(&design.target)
        .to_string();

    match target.as_str() {
        "xilinx-vitis" => pack_vitis(args, ctx, &design, &settings),
        "xilinx-hls" => pack_hls_zip(args, ctx, &settings),
        other => Err(CliError::InvalidArg(format!(
            "native pack only supports `xilinx-vitis` and `xilinx-hls`; \
             got `{other}`. (AIE was retired with the Python `program.run_aie` \
             port; rerun `analyze` with a supported target.)"
        ))),
    }
}

/// Native port of `tapa.program.pack::pack_zip` for the `xilinx-hls`
/// target. Bundles the synthesized RTL tree under `rtl/`, every HLS
/// `_csynth.rpt` under `report/` (with timestamp redaction so the
/// archive is reproducible), the TAPA report yaml at the archive root
/// when the synth step emitted one, plus `graph.yaml` and
/// `settings.yaml` snapshots of the persistent contexts that the
/// Python flow used to ship. Output path defaults to `work.zip` in the
/// caller's CWD and is always normalized to a `.zip` suffix to match
/// Python's `_enforce_path_suffix(suffix=".zip")`.
fn pack_hls_zip(args: &PackArgs, ctx: &CliContext, settings: &settings_io::Settings) -> Result<()> {
    use std::io::Write as _;
    let work_dir = ctx.work_dir.as_path();
    let rtl_dir = work_dir.join("rtl");
    if !rtl_dir.is_dir() {
        return Err(CliError::InvalidArg(format!(
            "RTL directory `{}` does not exist; run `tapa synth` first.",
            rtl_dir.display(),
        )));
    }
    let output_path = enforce_zip_suffix(args.output.as_ref());
    if let Some(parent) = output_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let file = std::fs::File::create(&output_path)?;
    let mut z = zip::ZipWriter::new(std::io::BufWriter::new(file));
    let opts: zip::write::FileOptions<'_, ()> =
        zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    let mut walk = vec![rtl_dir.clone()];
    let mut rtl_files: Vec<std::path::PathBuf> = Vec::new();
    while let Some(dir) = walk.pop() {
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                walk.push(path);
            } else if path.is_file() {
                rtl_files.push(path);
            }
        }
    }
    rtl_files.sort();
    for rtl_file in &rtl_files {
        let rel = rtl_file
            .strip_prefix(&rtl_dir)
            .map_err(|e| CliError::InvalidArg(format!("rtl strip_prefix: {e}")))?;
        let name = format!("rtl/{}", rel.to_string_lossy());
        z.start_file(name, opts)
            .map_err(|e| CliError::InvalidArg(format!("zip entry: {e}")))?;
        z.write_all(&std::fs::read(rtl_file)?)?;
    }

    // TAPA report yaml at archive root, only if the synth step wrote
    // one. Python always writes it; the Rust port has not ported the
    // emitter yet, so the file may be absent — silently skip in that
    // case rather than failing the pack step.
    let report_yaml = work_dir.join("report.yaml");
    if report_yaml.is_file() {
        z.start_file("report.yaml", opts)
            .map_err(|e| CliError::InvalidArg(format!("zip entry: {e}")))?;
        z.write_all(&std::fs::read(&report_yaml)?)?;
    }

    // Mirror Python's `program.pack_zip(..., graph=..., settings=...)`:
    // serialize the persisted contexts as YAML so downstream consumers
    // opening the archive can recover the compile metadata.
    let graph = graph_io::load_graph(work_dir)?;
    let graph_yaml = serde_yaml::to_string(&graph)
        .map_err(|e| CliError::InvalidArg(format!("graph yaml: {e}")))?;
    z.start_file("graph.yaml", opts)
        .map_err(|e| CliError::InvalidArg(format!("zip entry: {e}")))?;
    z.write_all(graph_yaml.as_bytes())?;
    let settings_yaml = serde_yaml::to_string(settings)
        .map_err(|e| CliError::InvalidArg(format!("settings yaml: {e}")))?;
    z.start_file("settings.yaml", opts)
        .map_err(|e| CliError::InvalidArg(format!("zip entry: {e}")))?;
    z.write_all(settings_yaml.as_bytes())?;

    // HLS `_csynth.rpt` files under `report/<rel>`. Mirror Python's
    // `_redact_rpt`: replace the per-run `Date:` line with the fixed
    // 1980-01-01 stamp so re-running HLS produces a byte-identical
    // archive (the same redaction `program.pack_xo` applies to xo).
    let hls_root = work_dir.join("hls");
    if hls_root.is_dir() {
        let mut rpt_files: Vec<std::path::PathBuf> = Vec::new();
        let mut walk = vec![hls_root.clone()];
        while let Some(dir) = walk.pop() {
            for entry in std::fs::read_dir(&dir)? {
                let entry = entry?;
                let path = entry.path();
                if path.is_dir() {
                    walk.push(path);
                } else if path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .is_some_and(|n| n.ends_with("_csynth.rpt"))
                {
                    rpt_files.push(path);
                }
            }
        }
        rpt_files.sort();
        for rpt in &rpt_files {
            let rel = rpt
                .strip_prefix(&hls_root)
                .map_err(|e| CliError::InvalidArg(format!("rpt strip_prefix: {e}")))?;
            let name = format!("report/{}", rel.to_string_lossy());
            z.start_file(name, opts)
                .map_err(|e| CliError::InvalidArg(format!("zip entry: {e}")))?;
            z.write_all(&redact_rpt(&std::fs::read(rpt)?))?;
        }
    }

    z.finish()
        .map_err(|e| CliError::InvalidArg(format!("zip finish: {e}")))?;
    Ok(())
}

/// Port of `tapa.program.pack::_redact_rpt`. Replaces the
/// per-HLS-run `Date:` line with a fixed 1980 stamp so the archive
/// is reproducible. Non-UTF-8 bytes are returned unchanged (Python
/// would have raised — neither path is reachable for valid HLS rpt).
fn redact_rpt(bytes: &[u8]) -> Vec<u8> {
    use std::sync::OnceLock;
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        regex::Regex::new("Date:           ... ... .. ..:..:.. ....")
            .expect("static date regex must compile")
    });
    match std::str::from_utf8(bytes) {
        Ok(text) => re
            .replace_all(text, "Date:           Tue Jan 01 00:00:00 1980")
            .into_owned()
            .into_bytes(),
        Err(_) => bytes.to_vec(),
    }
}

/// `_enforce_path_suffix(suffix=".zip")` for `--output`. Default to
/// `work.zip` in the caller's CWD (matches Python and the Vitis
/// `.xo` default — existing scripts that consume `./work.zip` keep
/// working).
fn enforce_zip_suffix(output: Option<&PathBuf>) -> PathBuf {
    match output {
        None => PathBuf::from("work.zip"),
        Some(p) => {
            if p.extension().and_then(|s| s.to_str()) == Some("zip") {
                p.clone()
            } else {
                let mut s = p.as_os_str().to_owned();
                s.push(".zip");
                PathBuf::from(s)
            }
        }
    }
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
        // `pack_hls_zip` mirrors Python's `pack_zip(..., graph=...)` and
        // requires `graph.json` to be present so it can emit `graph.yaml`.
        graph_io::store_graph(work_dir, &json!({"top": "Top", "tasks": {}}))
            .expect("store graph");
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
        write_state(dir.path(), "cpu-sim");
        let ctx = ctx_with_work_dir(dir.path());
        let err = run_native(&parse_pack(&[]), &ctx).expect_err("unknown target must reject");
        assert!(matches!(err, CliError::InvalidArg(ref m) if m.contains("xilinx-vitis")));
    }

    #[test]
    fn xilinx_hls_target_produces_zip() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_state(dir.path(), "xilinx-hls");

        // Minimal synthesis artifacts: one RTL file + a csynth report
        // whose `Date:` line should be normalized by the redactor.
        let rtl_dir = dir.path().join("rtl");
        std::fs::create_dir_all(&rtl_dir).expect("mkdir rtl");
        std::fs::write(rtl_dir.join("Top.v"), b"module Top; endmodule\n")
            .expect("write rtl stub");
        let report_dir = dir.path().join("hls/Top/syn/report");
        std::fs::create_dir_all(&report_dir).expect("mkdir hls report");
        std::fs::write(
            report_dir.join("Top_csynth.rpt"),
            b"== Header\nDate:           Mon Jan 02 03:04:05 2024\n== End\n",
        )
        .expect("write csynth stub");

        let output_path = dir.path().join("work.zip");
        let output_str = output_path.to_str().expect("utf-8 output");
        let ctx = ctx_with_work_dir(dir.path());
        run_native(&parse_pack(&["--output", output_str]), &ctx)
            .expect("xilinx-hls pack must succeed");
        assert!(output_path.exists(), "expected {output_str} to be written");

        // Inspect the archive: graph/settings yaml metadata are present
        // and the csynth report has the redacted reproducible Date.
        let zip_bytes = std::fs::read(&output_path).expect("read zip");
        let mut zr = zip::ZipArchive::new(std::io::Cursor::new(zip_bytes)).expect("open zip");
        let names: Vec<String> = (0..zr.len())
            .map(|i| zr.by_index(i).unwrap().name().to_string())
            .collect();
        assert!(names.iter().any(|n| n == "graph.yaml"), "graph.yaml missing: {names:?}");
        assert!(names.iter().any(|n| n == "settings.yaml"), "settings.yaml missing: {names:?}");
        assert!(names.iter().any(|n| n == "rtl/Top.v"));
        assert!(names.iter().any(|n| n == "report/Top/syn/report/Top_csynth.rpt"));

        let mut rpt = String::new();
        std::io::Read::read_to_string(
            &mut zr.by_name("report/Top/syn/report/Top_csynth.rpt").unwrap(),
            &mut rpt,
        )
        .expect("read rpt");
        assert!(
            rpt.contains("Date:           Tue Jan 01 00:00:00 1980"),
            "csynth Date not redacted: {rpt}"
        );
    }

    #[test]
    fn enforce_zip_suffix_defaults_to_cwd() {
        // Default mirrors Python's `_enforce_path_suffix(suffix=".zip")`:
        // a bare `work.zip` resolved against the caller's CWD, not
        // <work_dir>/work.zip.
        assert_eq!(enforce_zip_suffix(None), PathBuf::from("work.zip"));
        assert_eq!(
            enforce_zip_suffix(Some(&PathBuf::from("artifact"))),
            PathBuf::from("artifact.zip"),
        );
        assert_eq!(
            enforce_zip_suffix(Some(&PathBuf::from("ok.zip"))),
            PathBuf::from("ok.zip"),
        );
    }

    #[test]
    fn aie_target_is_rejected() {
        // AIE was retired with `program.run_aie`; analyze rejects it
        // up front, but a hand-edited `settings.json` (or a stale work
        // dir from before the change) must still surface a clear error
        // rather than silently no-op'ing.
        let dir = tempfile::tempdir().expect("tempdir");
        write_state(dir.path(), "xilinx-aie");
        let ctx = ctx_with_work_dir(dir.path());
        let err = run_native(&parse_pack(&[]), &ctx).expect_err("AIE must be rejected");
        assert!(
            matches!(err, CliError::InvalidArg(ref m) if m.contains("xilinx-aie")),
            "expected AIE rejection: {err:?}"
        );
    }

    #[test]
    fn missing_rtl_dir_surfaces_invalid_arg() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_state(dir.path(), "xilinx-vitis");
        let ctx = ctx_with_work_dir(dir.path());
        let err = run_native(&parse_pack(&[]), &ctx).expect_err("missing rtl dir must fail");
        assert!(matches!(err, CliError::InvalidArg(ref m) if m.contains("rtl")));
    }
}
