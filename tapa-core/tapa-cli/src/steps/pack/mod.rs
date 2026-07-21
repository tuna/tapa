//! `tapa pack` orchestration.
//!
//! Reloads `<work_dir>/tapa.json`, projects the top
//! task's external ports into a [`PackageXoInputs`] block, and drives
//! `tapa_xilinx::pack_xo` against `<work_dir>/rtl` to produce the
//! `.xo`. Three optional overlays are applied around the core pack:
//!
//! * `--custom-rtl <PATH>` (may repeat) — validate user-supplied
//!   Verilog files against `<work_dir>/templates_info.json` and copy
//!   them into `<work_dir>/rtl` before Vivado runs. An active floorplan
//!   rejects this late RTL mutation.
//! * `--bitstream-script <FILE>` — after `.xo` emission, render the
//!   `get_vitis_script` helper and drop it at the requested
//!   path (executable on Unix).
//!
//! The Vitis target emits an `.xo`; the HLS target emits a
//! reproducible `.zip` archive.

use std::path::{Path, PathBuf};

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
pub(crate) mod vitis_packaging;

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

    /// Memory connectivity `.ini` (v++ `sp=` bank assignments). When set, the
    /// emitted bitstream script adds it as a `--config`, so `v++ --link` binds
    /// each M-AXI to its bank — required for HBM/DDR designs.
    #[arg(long = "connectivity", value_name = "FILE")]
    pub connectivity: Option<PathBuf>,

    /// Custom RTL files / folders (may repeat; unavailable after floorplan).
    #[arg(long = "custom-rtl", value_name = "PATH")]
    pub custom_rtl: Vec<PathBuf>,
}

pub(super) fn published_floorplan_xdc(
    work_dir: &Path,
    state: &WorkState,
) -> Result<Option<PathBuf>> {
    if state.floorplan.is_none() {
        return Ok(None);
    }
    let path = work_dir.join(crate::steps::floorplan::FLOORPLAN_XDC);
    if !path.is_file() {
        return Err(CliError::MissingState {
            name: "published floorplan constraints (rerun `tapa floorplan`)".to_string(),
            path,
        });
    }
    Ok(Some(path))
}

/// Dispatch packaging according to the target stored by `analyze`.
pub fn run(args: &PackArgs, ctx: &CliContext) -> Result<()> {
    let state = work_io::load(&ctx.work_dir)?;
    if !state.flow.synthed {
        return Err(CliError::MissingState {
            name: "completed synthesis (run `tapa synth` first)".to_string(),
            path: work_io::path_in(&ctx.work_dir),
        });
    }
    published_floorplan_xdc(&ctx.work_dir, &state)?;
    if state.floorplan.is_some() && !args.custom_rtl.is_empty() {
        return Err(CliError::InvalidArg(
            "`--custom-rtl` cannot modify RTL after floorplanning; omit it or rerun synthesis and packaging without the active floorplan"
                .to_string(),
        ));
    }
    // The graph's `target` is the single home of the flow; this exhaustive
    // match is the dispatch site a new `Target` variant would break.
    match state.graph.target {
        Target::XilinxVitis => pack_vitis(args, ctx, &state),
        Target::XilinxHls => pack_hls_zip(args, ctx, &state),
    }
}

/// Package the `xilinx-hls` target. Bundles the synthesized RTL tree
/// under `rtl/`, every HLS
/// `_csynth.rpt` under `report/` (with timestamp redaction so the
/// archive is reproducible), the TAPA report yaml at the archive root
/// when the synth step emitted one, plus a verbatim copy of the
/// `tapa.json` state file carrying the compile context. Output
/// defaults to `work.zip` in the caller's CWD and is always normalized
/// to a `.zip` suffix.
/// Yield `(task_name, report_dir)` for every
/// `<work_dir>/hls/<task>/report/` directory. The zip and xo paths
/// both walk this layout; file selection, staging, and redaction stay
/// per-path.
pub(super) fn hls_task_report_dirs(work_dir: &Path) -> Result<Vec<(String, PathBuf)>> {
    let hls_root = work_dir.join("hls");
    let mut out = Vec::new();
    if !hls_root.is_dir() {
        return Ok(out);
    }
    for task_entry in fs_err::read_dir(&hls_root)? {
        let task_entry = task_entry?;
        if !task_entry.file_type()?.is_dir() {
            continue;
        }
        let report_dir = task_entry.path().join("report");
        if !report_dir.is_dir() {
            continue;
        }
        let task_name = task_entry.file_name().to_string_lossy().into_owned();
        out.push((task_name, report_dir));
    }
    Ok(out)
}

fn pack_hls_zip(args: &PackArgs, ctx: &CliContext, state: &WorkState) -> Result<()> {
    use std::io::Write as _;
    let work_dir = ctx.work_dir.as_path();
    let rtl_dir = work_dir.join("rtl");
    if !rtl_dir.is_dir() {
        return Err(CliError::MissingState {
            name: "RTL directory (run `tapa synth` first)".to_string(),
            path: rtl_dir,
        });
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
            .map_err(|e| CliError::Archive(format!("rtl strip_prefix: {e}")))?;
        let name = format!("rtl/{}", rel.to_slash_lossy());
        z.start_file(name, opts)
            .map_err(|e| CliError::Archive(format!("zip entry: {e}")))?;
        z.write_all(&fs_err::read(rtl_file)?)?;
    }

    // Include the TAPA report at archive root when synth emitted it.
    // Its absence does not make an otherwise complete RTL archive
    // invalid.
    let report_yaml = work_dir.join("report.yaml");
    if report_yaml.is_file() {
        z.start_file("report.yaml", opts)
            .map_err(|e| CliError::Archive(format!("zip entry: {e}")))?;
        z.write_all(&fs_err::read(&report_yaml)?)?;
    }

    // The state file itself, byte-for-byte: one schema, one format, one
    // definition. `frt-cosim` parses this entry back with the very
    // `tapa_ir::WorkState` types written here, so the archive's compile
    // metadata cannot drift from the work dir's.
    z.start_file(work_io::FILE_NAME, opts)
        .map_err(|e| CliError::Archive(format!("zip entry: {e}")))?;
    z.write_all(&work_io::to_bytes(state)?)?;

    // Store the curated per-task HLS `_csynth.rpt` files under
    // `report/<task>/<file>` and replace the per-run `Date:` line with the fixed
    // 1980-01-01 stamp so re-running HLS produces a byte-identical
    // archive (the same redaction `program.pack_xo` applies to xo).
    let task_report_dirs = hls_task_report_dirs(work_dir)?;
    if !task_report_dirs.is_empty() {
        let mut rpt_files: Vec<(std::path::PathBuf, String)> = Vec::new();
        for (task_name, report_root) in &task_report_dirs {
            for entry in walkdir::WalkDir::new(report_root) {
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
                    .strip_prefix(report_root)
                    .map_err(|e| CliError::Archive(format!("rpt strip_prefix: {e}")))?;
                let name = format!("report/{task_name}/{}", rel.to_slash_lossy());
                rpt_files.push((path.to_path_buf(), name));
            }
        }
        rpt_files.sort();
        for (rpt, name) in &rpt_files {
            z.start_file(name, opts)
                .map_err(|e| CliError::Archive(format!("zip entry: {e}")))?;
            z.write_all(&redact_rpt(&fs_err::read(rpt)?))?;
        }
    }

    z.finish()
        .map_err(|e| CliError::Archive(format!("zip finish: {e}")))?;
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
        Area, FloorplanResult, SynthTarget, Task, TaskGraph, TaskLevel,
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

    fn activate_floorplan(work_dir: &Path) {
        let mut state = work_io::load(work_dir).expect("load state");
        state.floorplan = Some(FloorplanResult {
            device: "u280".to_string(),
            grid: (2, 3),
            regions: BTreeMap::new(),
            routes: Vec::new(),
            slot_usage: BTreeMap::<String, Area>::new(),
        });
        work_io::store(work_dir, &state).expect("store floorplan");
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
        let err = run(&parse_pack(&[]), &ctx).expect_err("unknown target must reject");
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
        run(&parse_pack(&["--output", output_str]), &ctx).expect("xilinx-hls pack must succeed");
        assert!(output_path.exists(), "expected {output_str} to be written");

        // Inspect the archive: the state snapshot is present and the
        // csynth report has the redacted reproducible Date.
        let zip_bytes = fs_err::read(&output_path).expect("read zip");
        let mut zr = zip::ZipArchive::new(std::io::Cursor::new(zip_bytes)).expect("open zip");
        let names: Vec<String> = (0..zr.len())
            .map(|i| zr.by_index(i).unwrap().name().to_string())
            .collect();
        assert!(
            names.iter().any(|n| n == "tapa.json"),
            "tapa.json missing: {names:?}"
        );
        assert!(
            !names
                .iter()
                .any(|n| n == "graph.yaml" || n == "settings.yaml"),
            "the split YAML snapshots must be gone, not shipped alongside \
             tapa.json: {names:?}",
        );

        // The `tapa.json` entry is `frt-cosim`'s contract. Pin that it is the
        // work-dir state file verbatim — same bytes, same schema — because
        // cosim parses it with `tapa_ir::WorkState` and would fail at
        // runtime, not compile time, on a reshaped archive.
        let mut packed = Vec::new();
        std::io::Read::read_to_end(&mut zr.by_name("tapa.json").unwrap(), &mut packed)
            .expect("read tapa.json");
        let on_disk = fs_err::read(crate::state::work::path_in(dir.path())).expect("read state");
        assert_eq!(
            packed, on_disk,
            "the archive's tapa.json must be the work dir's, byte for byte",
        );
        let state =
            tapa_ir::WorkState::from_json(std::str::from_utf8(&packed).expect("utf-8 tapa.json"))
                .expect(
                    "packed tapa.json must strict-parse as WorkState (frt-cosim does exactly this)",
                );
        assert_eq!(state.graph.top, "Top", "cosim recovers the top task name");
        assert!(
            state.graph.tasks.contains_key("Top"),
            "cosim recovers the top task's ports from the tasks map",
        );
        assert_eq!(
            state.flow.part_num.as_deref(),
            Some("xcu250-figd2104-2L-e"),
            "cosim recovers the part number from the flow block",
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
    fn missing_rtl_dir_surfaces_missing_state() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_state(dir.path(), Target::XilinxVitis);
        let ctx = ctx_with_work_dir(dir.path());
        let err = run(&parse_pack(&[]), &ctx).expect_err("missing rtl dir must fail");
        assert!(matches!(err, CliError::MissingState { ref name, .. } if name.contains("RTL")));
    }

    #[test]
    fn incomplete_synthesis_is_rejected_before_packaging() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_state(dir.path(), Target::XilinxVitis);
        let mut state = work_io::load(dir.path()).expect("load");
        state.flow.synthed = false;
        work_io::store(dir.path(), &state).expect("store");

        let error = run(&parse_pack(&[]), &ctx_with_work_dir(dir.path()))
            .expect_err("pack must not consume partial synthesis outputs");
        assert!(
            matches!(error, CliError::MissingState { ref name, .. } if name.contains("synthesis")),
            "got {error}",
        );
    }

    #[test]
    fn every_target_requires_the_active_floorplan_publication_marker() {
        for target in [Target::XilinxVitis, Target::XilinxHls] {
            let dir = tempfile::tempdir().expect("tempdir");
            write_state(dir.path(), target);
            activate_floorplan(dir.path());

            let error = run(&parse_pack(&[]), &ctx_with_work_dir(dir.path()))
                .expect_err("unpublished floorplan must not be packaged");

            assert!(
                matches!(error, CliError::MissingState { ref name, .. }
                    if name.contains("floorplan constraints")),
                "got {error}",
            );
        }
    }

    #[test]
    fn every_target_rejects_custom_rtl_after_floorplanning() {
        for target in [Target::XilinxVitis, Target::XilinxHls] {
            let dir = tempfile::tempdir().expect("tempdir");
            write_state(dir.path(), target);
            activate_floorplan(dir.path());
            fs_err::write(
                dir.path().join(crate::steps::floorplan::FLOORPLAN_XDC),
                "constraints",
            )
            .expect("write marker");

            let error = run(
                &parse_pack(&["--custom-rtl", "replacement.v"]),
                &ctx_with_work_dir(dir.path()),
            )
            .expect_err("post-floorplan RTL mutation must be rejected");

            assert!(matches!(error, CliError::InvalidArg(ref message)
                if message.contains("--custom-rtl") && message.contains("after floorplanning")));
        }
    }
}
