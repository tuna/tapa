//! `tapa pack` orchestration.
//!
//! Reloads `<work_dir>/tapa.json`, projects the top
//! task's external ports into a [`PackageXoInputs`] block, and drives
//! `tapa_xilinx::pack_xo` against `<work_dir>/rtl` to produce the
//! `.xo`. Three optional overlays are applied around the core pack:
//!
//! * `--custom-rtl <PATH>` (may repeat) — validate user-supplied
//!   Verilog files against `<work_dir>/templates_info.json` and copy
//!   them into `<work_dir>/rtl` before Vivado runs.
//! * `--bitstream-script <FILE>` — after `.xo` emission, render the
//!   `get_vitis_script` helper and drop it at the requested
//!   path (executable on Unix).
//!
//! The Vitis target emits an `.xo`; the HLS target emits a
//! reproducible `.zip` archive.

use std::path::PathBuf;

use clap::Parser;
use fs_err;
use path_slash::PathExt;
use tapa_ir::Target;

use crate::context::CliContext;
use crate::error::{CliError, Result};
use crate::state::work::{self as work_io, WorkState};

mod bitstream_script;
mod custom_rtl;
mod kernel_xml_ports;
mod vitis_packaging;

use custom_rtl::{apply_custom_rtl, load_templates_info};
use vitis_packaging::pack_vitis;

#[derive(Debug, Clone, Parser)]
#[command(
    name = "pack",
    about = "Pack the generated RTL into a Xilinx object file."
)]
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
}

pub fn to_cli_argv(args: &PackArgs) -> Vec<String> {
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
    out
}

/// Dispatch packaging according to the target stored by `analyze`.
pub fn run(args: &PackArgs, ctx: &mut CliContext) -> Result<()> {
    run_native(args, ctx)
}

fn run_native(args: &PackArgs, ctx: &CliContext) -> Result<()> {
    let state = work_io::load(&ctx.work_dir)?;
    // The graph's `target` is the single home of the flow; this exhaustive
    // match is the dispatch site a new `Target` variant would break.
    match state.graph.target {
        Target::XilinxVitis => pack_vitis(args, ctx, &state.graph, &state.flow),
        Target::XilinxHls => pack_hls_zip(args, ctx, &state),
    }
}

/// Package the `xilinx-hls` target. Bundles the synthesized RTL tree
/// under `rtl/`, every HLS
/// `_csynth.rpt` under `report/` (with timestamp redaction so the
/// archive is reproducible), the TAPA report yaml at the archive root
/// when the synth step emitted one, plus `graph.yaml` and
/// `settings.yaml` snapshots of the persistent compile context. Output
/// defaults to `work.zip` in the caller's CWD and is always normalized
/// to a `.zip` suffix.
fn pack_hls_zip(args: &PackArgs, ctx: &CliContext, state: &WorkState) -> Result<()> {
    use std::io::Write as _;
    let work_dir = ctx.work_dir.as_path();
    let rtl_dir = work_dir.join("rtl");
    if !rtl_dir.is_dir() {
        return Err(CliError::InvalidArg(format!(
            "RTL directory `{}` does not exist; run `tapa synth` first.",
            rtl_dir.display(),
        )));
    }
    if !args.custom_rtl.is_empty() {
        let templates = load_templates_info(&ctx.work_dir)?;
        apply_custom_rtl(&rtl_dir, &args.custom_rtl, &templates)?;
    }
    let output_path = enforce_zip_suffix(args.output.as_ref());
    if let Some(parent) = output_path.parent() {
        if !parent.as_os_str().is_empty() {
            fs_err::create_dir_all(parent)?;
        }
    }
    let file = fs_err::File::create(&output_path)?;
    let mut z = zip::ZipWriter::new(std::io::BufWriter::new(file));
    let opts: zip::write::FileOptions<'_, ()> =
        zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    let mut rtl_files: Vec<std::path::PathBuf> = Vec::new();
    for entry in walkdir::WalkDir::new(&rtl_dir) {
        let entry = entry.map_err(std::io::Error::other)?;
        if entry.file_type().is_file() {
            rtl_files.push(entry.path().to_path_buf());
        }
    }
    rtl_files.sort();
    for rtl_file in &rtl_files {
        let rel = rtl_file
            .strip_prefix(&rtl_dir)
            .map_err(|e| CliError::InvalidArg(format!("rtl strip_prefix: {e}")))?;
        let name = format!("rtl/{}", rel.to_slash_lossy());
        z.start_file(name, opts)
            .map_err(|e| CliError::InvalidArg(format!("zip entry: {e}")))?;
        z.write_all(&fs_err::read(rtl_file)?)?;
    }

    // Include the TAPA report at archive root when synth emitted it.
    // Its absence does not make an otherwise complete RTL archive
    // invalid.
    let report_yaml = work_dir.join("report.yaml");
    if report_yaml.is_file() {
        z.start_file("report.yaml", opts)
            .map_err(|e| CliError::InvalidArg(format!("zip entry: {e}")))?;
        z.write_all(&fs_err::read(&report_yaml)?)?;
    }

    // Serialize the persisted graph and flow settings as YAML so downstream
    // consumers can recover compile metadata from the archive.
    //
    // These two entries stay split, and stay at these names, because they are
    // a cross-workspace contract: `frt-cosim`'s zip reader
    // (`fpga-runtime/frt-cosim/src/metadata`) requires a `graph.yaml` whose
    // root carries `top`/`tasks`, and recovers `part_num` from the root of
    // `settings.yaml`. Merging them into one `tapa.json`-shaped entry would
    // nest the graph a level down and break cosim. The work dir collapsed to
    // one state file; the archive layout is a published surface and did not.
    let graph_yaml = serde_yaml::to_string(&state.graph)
        .map_err(|e| CliError::InvalidArg(format!("graph yaml: {e}")))?;
    z.start_file("graph.yaml", opts)
        .map_err(|e| CliError::InvalidArg(format!("zip entry: {e}")))?;
    z.write_all(graph_yaml.as_bytes())?;
    let settings_yaml = serde_yaml::to_string(&state.flow)
        .map_err(|e| CliError::InvalidArg(format!("settings yaml: {e}")))?;
    z.start_file("settings.yaml", opts)
        .map_err(|e| CliError::InvalidArg(format!("zip entry: {e}")))?;
    z.write_all(settings_yaml.as_bytes())?;

    // Store the curated per-task HLS `_csynth.rpt` files under
    // `report/<task>/<file>` and replace the per-run `Date:` line with the fixed
    // 1980-01-01 stamp so re-running HLS produces a byte-identical
    // archive (the same redaction `program.pack_xo` applies to xo).
    let hls_root = work_dir.join("hls");
    if hls_root.is_dir() {
        let mut rpt_files: Vec<(std::path::PathBuf, String)> = Vec::new();
        for task_entry in fs_err::read_dir(&hls_root)? {
            let task_entry = task_entry?;
            if !task_entry.file_type()?.is_dir() {
                continue;
            }
            let task_name = task_entry.file_name().to_string_lossy().into_owned();
            let report_root = task_entry.path().join("report");
            if !report_root.is_dir() {
                continue;
            }
            for entry in walkdir::WalkDir::new(&report_root) {
                let entry = entry.map_err(std::io::Error::other)?;
                if !entry.file_type().is_file() {
                    continue;
                }
                let path = entry.path();
                if !path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .is_some_and(|n| n.ends_with("_csynth.rpt"))
                {
                    continue;
                }
                let rel = path
                    .strip_prefix(&report_root)
                    .map_err(|e| CliError::InvalidArg(format!("rpt strip_prefix: {e}")))?;
                let name = format!("report/{task_name}/{}", rel.to_slash_lossy());
                rpt_files.push((path.to_path_buf(), name));
            }
        }
        rpt_files.sort();
        for (rpt, name) in &rpt_files {
            z.start_file(name, opts)
                .map_err(|e| CliError::InvalidArg(format!("zip entry: {e}")))?;
            z.write_all(&redact_rpt(&fs_err::read(rpt)?))?;
        }
    }

    z.finish()
        .map_err(|e| CliError::InvalidArg(format!("zip finish: {e}")))?;
    Ok(())
}

/// Replace the per-HLS-run `Date:` line with a fixed 1980 stamp so
/// the archive is reproducible. Non-UTF-8 bytes are returned
/// unchanged; valid HLS reports are UTF-8.
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

/// Return `<default>` in the caller's CWD when `output` is absent;
/// otherwise append `.{ext}`
/// unless the path already carries it. Used for both `--output` shapes
/// (`.zip` for HLS pack, `.xo` for Vitis pack).
fn enforce_path_suffix(output: Option<&PathBuf>, ext: &str, default: &str) -> PathBuf {
    match output {
        None => PathBuf::from(default),
        Some(p) if p.extension().and_then(|s| s.to_str()) == Some(ext) => p.clone(),
        Some(p) => {
            let mut s = p.as_os_str().to_owned();
            s.push(".");
            s.push(ext);
            PathBuf::from(s)
        }
    }
}

fn enforce_zip_suffix(output: Option<&PathBuf>) -> PathBuf {
    enforce_path_suffix(output, "zip", "work.zip")
}

fn enforce_xo_suffix(output: Option<&PathBuf>) -> PathBuf {
    enforce_path_suffix(output, "xo", "work.xo")
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

    use std::collections::BTreeMap;

    use indexmap::IndexMap;
    use tapa_ir::{
        port::{ArgCategory, Port},
        SynthTarget, Task, TaskGraph, TaskLevel,
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

    fn write_state(work_dir: &Path, target: Target) {
        fs_err::create_dir_all(work_dir).expect("mkdir work");
        let mut tasks = BTreeMap::new();
        tasks.insert(
            "Top".to_string(),
            Task {
                level: TaskLevel::Upper,
                code: "void Top() {}".to_string(),
                ports: vec![Port {
                    cat: ArgCategory::Mmap,
                    name: "gmem0".to_string(),
                    ctype: "int*".to_string(),
                    width: 512,
                    chan_count: None,
                    chan_size: None,
                }],
                tasks: BTreeMap::new(),
                fifos: BTreeMap::new(),
                readable_name: "Top".to_string(),
                synth: SynthTarget::Hls,
                self_area: IndexMap::new(),
                total_area: IndexMap::new(),
                clock_period: "3.33".to_string(),
            },
        );
        let mut state = WorkState::new(TaskGraph {
            top: "Top".to_string(),
            target,
            tasks,
            cflags: Vec::new(),
        });
        state.flow.part_num = Some("xcu250-figd2104-2L-e".to_string());
        state.flow.clock_period = Some("3.33".to_string());
        state.flow.synthed = true;
        work_io::store(work_dir, &state).expect("store state");
    }

    #[test]
    fn argv_round_trips_current_shape() {
        let args = parse_pack(&["--output", "vadd.xo"]);
        let argv = to_cli_argv(&args);
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
    fn unsupported_target_is_rejected_with_the_supported_flows() {
        // The flow target now has exactly one home, so an unsupported value
        // is caught when the state parses rather than by a settings-vs-design
        // reconciliation. The user must still be told which flows do work.
        let dir = tempfile::tempdir().expect("tempdir");
        write_state(dir.path(), Target::XilinxHls);
        let path = crate::state::work::path_in(dir.path());
        let text = fs_err::read_to_string(&path).expect("read state");
        fs_err::write(&path, text.replace("xilinx-hls", "cpu-sim")).expect("write state");

        let ctx = ctx_with_work_dir(dir.path());
        let err = run_native(&parse_pack(&[]), &ctx).expect_err("unknown target must reject");
        let text = err.to_string();
        assert!(
            text.contains("cpu-sim") && text.contains("xilinx-vitis"),
            "error must name the bad target and the supported flows; got {text}",
        );
    }

    #[test]
    fn xilinx_hls_target_produces_zip() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_state(dir.path(), Target::XilinxHls);

        // Minimal synthesis artifacts: one RTL file + a csynth report
        // whose `Date:` line should be normalized by the redactor.
        let rtl_dir = dir.path().join("rtl");
        fs_err::create_dir_all(&rtl_dir).expect("mkdir rtl");
        fs_err::write(rtl_dir.join("Top.v"), b"module Top; endmodule\n").expect("write rtl stub");
        let report_dir = dir.path().join("hls/Top/report");
        fs_err::create_dir_all(&report_dir).expect("mkdir hls report");
        fs_err::write(
            report_dir.join("Top_csynth.rpt"),
            b"== Header\nDate:           Mon Jan 02 03:04:05 2024\n== End\n",
        )
        .expect("write csynth stub");
        // Kept HLS projects may contain another copy of the report. It must
        // not leak into the public archive alongside the curated report.
        let project_report_dir = dir.path().join("hls/Top/project/Top/syn/report");
        fs_err::create_dir_all(&project_report_dir).expect("mkdir project report");
        fs_err::write(
            project_report_dir.join("Top_csynth.rpt"),
            b"project-internal duplicate\n",
        )
        .expect("write project report stub");

        let output_path = dir.path().join("work.zip");
        let output_str = output_path.to_str().expect("utf-8 output");
        let ctx = ctx_with_work_dir(dir.path());
        run_native(&parse_pack(&["--output", output_str]), &ctx)
            .expect("xilinx-hls pack must succeed");
        assert!(output_path.exists(), "expected {output_str} to be written");

        // Inspect the archive: the state snapshot is present and the
        // csynth report has the redacted reproducible Date.
        let zip_bytes = fs_err::read(&output_path).expect("read zip");
        let mut zr = zip::ZipArchive::new(std::io::Cursor::new(zip_bytes)).expect("open zip");
        let names: Vec<String> = (0..zr.len())
            .map(|i| zr.by_index(i).unwrap().name().to_string())
            .collect();
        assert!(
            names.iter().any(|n| n == "graph.yaml"),
            "graph.yaml missing: {names:?}"
        );
        assert!(
            names.iter().any(|n| n == "settings.yaml"),
            "settings.yaml missing: {names:?}"
        );

        // These two entries are `frt-cosim`'s contract, so pin the *shape* it
        // reads, not just the names: `graph.yaml` must carry `top`/`tasks` at
        // the root (`frt-cosim::metadata::zip_pkg::parse_graph_yaml`) and
        // `settings.yaml` must carry `part_num` at the root
        // (`parse_part_from_settings_yaml`). Nesting either under a `graph:` /
        // `flow:` key would break cosim at runtime, not at compile time.
        let read_entry = |zr: &mut zip::ZipArchive<std::io::Cursor<Vec<u8>>>, name: &str| {
            let mut s = String::new();
            std::io::Read::read_to_string(&mut zr.by_name(name).unwrap(), &mut s)
                .unwrap_or_else(|e| panic!("read {name}: {e}"));
            serde_yaml::from_str::<serde_yaml::Value>(&s)
                .unwrap_or_else(|e| panic!("parse {name}: {e}"))
        };
        let graph_v = read_entry(&mut zr, "graph.yaml");
        assert_eq!(
            graph_v.get("top").and_then(|v| v.as_str()),
            Some("Top"),
            "graph.yaml must keep `top` at the root for frt-cosim",
        );
        assert!(
            graph_v
                .get("tasks")
                .and_then(|v| v.as_mapping())
                .is_some_and(|m| m.contains_key(serde_yaml::Value::String("Top".to_string()))),
            "graph.yaml must keep the `tasks` mapping at the root for frt-cosim",
        );
        let settings_v = read_entry(&mut zr, "settings.yaml");
        assert_eq!(
            settings_v.get("part_num").and_then(|v| v.as_str()),
            Some("xcu250-figd2104-2L-e"),
            "settings.yaml must keep `part_num` at the root for frt-cosim",
        );
        assert!(names.iter().any(|n| n == "rtl/Top.v"));
        assert!(names.iter().any(|n| n == "report/Top/Top_csynth.rpt"));
        assert_eq!(
            names
                .iter()
                .filter(|n| n.ends_with("/Top_csynth.rpt"))
                .count(),
            1,
            "kept HLS project reports must not be duplicated: {names:?}"
        );
        // AC-2: ZIP entry names must never contain platform separators.
        for name in &names {
            assert!(
                !name.contains('\\'),
                "ZIP entry name must use forward slashes: {name}"
            );
        }

        let mut rpt = String::new();
        std::io::Read::read_to_string(
            &mut zr.by_name("report/Top/Top_csynth.rpt").unwrap(),
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
        // A bare `work.zip` resolves against the caller's CWD, not
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
    fn missing_rtl_dir_surfaces_invalid_arg() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_state(dir.path(), Target::XilinxVitis);
        let ctx = ctx_with_work_dir(dir.path());
        let err = run_native(&parse_pack(&[]), &ctx).expect_err("missing rtl dir must fail");
        assert!(matches!(err, CliError::InvalidArg(ref m) if m.contains("rtl")));
    }
}
