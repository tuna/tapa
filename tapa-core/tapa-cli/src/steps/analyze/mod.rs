//! `tapa analyze` orchestration.
//!
//! Composes `tapa-cpp` (preprocessor) and `tapacc` (semantic analyzer)
//! invocations, then writes the work dir's one state file,
//! `<work_dir>/tapa.json`, plus the verbatim `tapacc` output as a debug
//! artifact.

use std::fs;
use std::path::PathBuf;

use clap::{Parser, ValueEnum};
use serde_json::{json, Value};
use tapa_ir::Graph;

use crate::context::CliContext;
use crate::error::{CliError, Result};
use crate::state::json::write_bytes_atomic;
use crate::state::work::{self as work_io, WorkState};
use crate::tapacc::cflags::{get_system_cflags, get_tapacc_cflags};
use crate::tapacc::discover::find_clang_binary;

/// Verbatim `tapacc` stdout, kept under the work dir for provenance and
/// debugging. **Nothing reads this back** — `analyze` parses `tapacc`'s
/// output in-process into the typed model that `tapa.json` carries. Keeping
/// it write-only is what stops the work dir growing a second schema-bearing
/// file that can drift from the first.
const TAPACC_ARTIFACT: &str = "tapacc.json";

mod build_design;
mod run_flatten;
mod run_tapacc;

use build_design::{flatten_graph_value, is_top_leaf};
use run_flatten::run_flatten;
use run_tapacc::run_tapacc;

/// Target flows accepted by `tapa analyze`. Kebab-case spellings match
/// the wire strings persisted into `tapa.json` and read by `synth` /
/// `pack` (`xilinx-vitis`, `xilinx-hls`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum AnalyzeTarget {
    #[value(name = "xilinx-vitis")]
    XilinxVitis,
    #[value(name = "xilinx-hls")]
    XilinxHls,
}

impl AnalyzeTarget {
    /// Canonical wire string persisted into `tapa.json`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::XilinxVitis => "xilinx-vitis",
            Self::XilinxHls => "xilinx-hls",
        }
    }
}

/// Bridge the clap arg enum to the schema enum stored in `tapa.json`.
/// Keeps `tapa-ir` free of a clap dependency.
impl From<AnalyzeTarget> for tapa_ir::Target {
    fn from(target: AnalyzeTarget) -> Self {
        match target {
            AnalyzeTarget::XilinxVitis => Self::XilinxVitis,
            AnalyzeTarget::XilinxHls => Self::XilinxHls,
        }
    }
}

#[derive(Debug, Clone, Parser)]
#[command(
    name = "analyze",
    about = "Analyze TAPA program and store the program description."
)]
pub struct AnalyzeArgs {
    /// Input file, usually TAPA C++ source code (may repeat).
    #[arg(short = 'f', long = "input", value_name = "FILE", required = true)]
    pub input_files: Vec<PathBuf>,

    /// Name of the top-level task.
    #[arg(short = 't', long = "top", value_name = "TASK", required = true)]
    pub top: String,

    /// Compiler flags for the kernel; may appear many times.
    #[arg(short = 'c', long = "cflags", value_name = "FLAG")]
    pub cflags: Vec<String>,

    /// Flatten the hierarchy with all leaf-level tasks at top.
    #[arg(long = "flatten-hierarchy", default_value_t = false)]
    pub flatten_hierarchy: bool,

    /// Counterpart to `--flatten-hierarchy`; default behavior.
    #[arg(long = "keep-hierarchy", conflicts_with = "flatten_hierarchy")]
    pub keep_hierarchy: bool,

    /// Target flow. Restricted to targets the pipeline can drive
    /// end-to-end so typos and unsupported targets fail
    /// at parse time instead of producing an unusable `tapa.json` that
    /// only blows up later in `synth` or `pack`.
    #[arg(long = "target", value_enum, default_value_t = AnalyzeTarget::XilinxVitis)]
    pub target: AnalyzeTarget,

    /// Explicit path to the `tapacc` binary. Overrides the walk-up
    /// `find_resource` search anchored at the `tapa` binary. Used by
    /// Bazel driver rules (`bazel/tapa_rules.bzl::_tapa_xo_impl`)
    /// that locate the toolchain inputs through their own dep graph
    /// and pass them down explicitly.
    #[arg(long = "tapacc", value_name = "FILE")]
    pub tapacc: Option<PathBuf>,

    /// Explicit path to the `tapa-cpp` (clang) binary. Same rationale
    /// as `--tapacc`. Accepts the `--tapa-clang` alias used by older
    /// Bazel driver rules.
    #[arg(long = "tapa-cpp", visible_alias = "tapa-clang", value_name = "FILE")]
    pub tapa_cpp: Option<PathBuf>,
}

/// Run tapacc on each input, merge the task graphs, and write
/// `<work_dir>/tapa.json` (plus the flattened sources when
/// `--flatten-hierarchy` is set).
pub fn run(args: &AnalyzeArgs, ctx: &CliContext) -> Result<()> {
    // `--tapacc`/`--tapa-cpp` override the walk-up `find_resource`
    // search. Used by the Bazel driver to inject the exact sandbox
    // paths; direct `tapa analyze` runs on a developer machine still
    // fall through to the default discovery path.
    let tapa_cpp = if let Some(p) = args.tapa_cpp.as_ref() {
        p.clone()
    } else {
        find_clang_binary("tapa-cpp-binary")?
    };
    let tapacc = if let Some(p) = args.tapacc.as_ref() {
        p.clone()
    } else {
        find_clang_binary("tapacc-binary")?
    };

    // Vitis HLS only supports up to C++14; keep it after user flags.
    let mut user_cflags = args.cflags.clone();
    user_cflags.push("-std=c++14".to_string());

    let mut all_cflags = user_cflags.clone();
    all_cflags.extend(get_tapacc_cflags(false));
    all_cflags.extend(get_system_cflags());

    let work_dir = ctx.work_dir.as_path();
    fs::create_dir_all(work_dir)?;
    let flatten_files = run_flatten(
        &tapa_cpp,
        &args.input_files,
        &all_cflags,
        work_dir,
        ctx.clang_format_quota_in_bytes,
    )?;
    let target_str = args.target.as_str();
    let stdout = run_tapacc(
        &tapacc,
        &flatten_files,
        &args.top,
        &all_cflags,
        target_str,
        work_dir,
    )?;

    // Persist the raw bytes first, so a `tapacc` output that fails the parse
    // below is still on disk for inspection.
    write_bytes_atomic(work_dir, TAPACC_ARTIFACT, stdout.as_bytes())?;

    let mut graph_dict: Value = serde_json::from_str(&stdout)?;

    // Analyze owns the two root facts `tapacc` does not emit: the user's
    // cflags tuple (plus the required C++ standard flag) and the flow
    // target. Both are injected before the strict parse below, so the graph
    // conforms to the schema `tapa.json` carries.
    if let Some(obj) = graph_dict.as_object_mut() {
        obj.insert(
            "cflags".to_string(),
            Value::Array(user_cflags.iter().cloned().map(Value::String).collect()),
        );
        obj.insert(
            "target".to_string(),
            json!(tapa_ir::Target::from(args.target).as_str()),
        );
    }

    let mut graph: Graph = serde_json::from_value(graph_dict)
        .map_err(|e| CliError::Codegen(format!("graph schema error: {e}")))?;

    if args.flatten_hierarchy {
        graph = flatten_graph_value(&graph)?;
    }

    if is_top_leaf(&graph, &args.top) && args.target == AnalyzeTarget::XilinxVitis {
        return Err(CliError::InvalidArg(
            "the top task is a leaf task; target `xilinx-vitis` is not supported \
             (Vitis requires an upper top for kernel.xml generation). \
             Rerun with `--target xilinx-hls`."
                .to_string(),
        ));
    }

    // Seed the work dir's one state file. `synth` annotates the graph in
    // place with post-synthesis results and fills in `flow`.
    let state = WorkState::new(graph);
    work_io::store(work_dir, &state)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;

    use crate::context::CliContext;
    use crate::globals::GlobalArgs;

    #[cfg(unix)]
    #[test]
    fn analyze_writes_state_and_tapacc_artifact() {
        use std::os::unix::fs::PermissionsExt;

        // Build an isolated tempdir that doubles as both:
        //   - the search anchor for `find_resource` (POTENTIAL_PATHS roots)
        //   - the work_dir for the analyze step.
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();

        // Plant fake `tapa-cpp` and `tapacc` binaries that satisfy
        // `find_clang_binary`'s `--version` probe and emit a fixed graph.
        let tapa_cpp_dir = root.join("tapa-cpp");
        let tapacc_dir = root.join("tapacc");
        fs::create_dir_all(&tapa_cpp_dir).expect("mkdir tapa-cpp");
        fs::create_dir_all(&tapacc_dir).expect("mkdir tapacc");
        let tapa_cpp = tapa_cpp_dir.join("tapa-cpp");
        let tapacc = tapacc_dir.join("tapacc");

        // tapa-cpp: `--version` prints a parseable line; otherwise it
        // writes its trailing positional input file's bytes to stdout.
        fs::write(
            &tapa_cpp,
            "#!/bin/sh\n\
             if [ \"$1\" = \"--version\" ]; then\n\
               echo 'fake tapa-cpp version 18.0.0'\n\
               exit 0\n\
             fi\n\
             # Last argument is the input file.\n\
             eval last=\\${$#}\n\
             cat \"$last\"\n",
        )
        .expect("write tapa-cpp");
        fs::set_permissions(&tapa_cpp, fs::Permissions::from_mode(0o755)).expect("chmod tapa-cpp");

        // tapacc: `--version` is parseable; otherwise it emits a fixed
        // tapacc-shaped task graph on stdout.
        // Mirrors real `tapacc` output: `readable_name` is emitted for every
        // task (equal to the task name for these non-template tasks).
        let fixed_graph = r#"{"cflags": [], "tasks": {"VecAdd": {"code": "void VecAdd() {}", "level": "upper", "synth": "hls", "readable_name": "VecAdd", "ports": [], "tasks": {"Add": [{"step": 0, "args": {}}]}, "fifos": {}}, "Add": {"code": "void Add() {}", "level": "lower", "synth": "hls", "readable_name": "Add", "ports": []}}, "top": "VecAdd"}"#;
        fs::write(
            &tapacc,
            format!(
                "#!/bin/sh\n\
                 if [ \"$1\" = \"--version\" ]; then\n\
                   echo 'fake tapacc version 18.0.0'\n\
                   exit 0\n\
                 fi\n\
                 cat <<'__JSON__'\n{fixed_graph}\n__JSON__\n",
            ),
        )
        .expect("write tapacc");
        fs::set_permissions(&tapacc, fs::Permissions::from_mode(0o755)).expect("chmod tapacc");

        // Plant a trivial input file under the same root.
        let input_file = root.join("vadd.cpp");
        fs::write(&input_file, b"void VecAdd() {}\n").expect("write vadd.cpp");

        // Steer `find_resource` at `root` so the planted binaries win.
        std::env::set_var("TAPA_CLI_SEARCH_ANCHOR", root);
        let work_dir = root.join("work");
        let globals = GlobalArgs::try_parse_from([
            "tapa",
            "--work-dir",
            work_dir.to_str().expect("utf-8 work dir"),
        ])
        .expect("parse globals");
        let ctx = CliContext::from_globals(&globals);

        let args = AnalyzeArgs::try_parse_from([
            "analyze",
            "--input",
            input_file.to_str().expect("utf-8 path"),
            "--top",
            "VecAdd",
            "--target",
            "xilinx-hls",
        ])
        .expect("parse analyze args");

        run(&args, &ctx).expect("native analyze should succeed");

        // The single state file must round-trip with the projected topology.
        let state = work_io::load(&ctx.work_dir).expect("load state");
        assert_eq!(state.version, work_io::VERSION, "state is version-stamped");
        assert_eq!(state.graph.top, "VecAdd");
        // Analyze injects the root flow target `tapacc` does not emit.
        assert_eq!(state.graph.target.as_str(), "xilinx-hls");
        // Analyze stores the user cflags plus `-std=c++14`. The user passed
        // no `-c`, so we expect just that.
        assert_eq!(state.graph.cflags, vec!["-std=c++14".to_string()]);
        assert!(state.graph.tasks.contains_key("VecAdd"));
        assert!(state.graph.tasks.contains_key("Add"));
        assert_eq!(state.graph.tasks["VecAdd"].level, tapa_ir::TaskLevel::Upper);
        assert_eq!(state.graph.tasks["Add"].level, tapa_ir::TaskLevel::Lower);
        assert_eq!(state.graph.tasks["Add"].synth, tapa_ir::SynthTarget::Hls);
        // Nothing has synthesized yet, so the flow block is still pristine.
        assert_eq!(state.flow, crate::state::work::FlowSettings::default());

        // The superseded state files must not reappear: one schema-bearing
        // file in the work dir is the whole point.
        for stale in ["graph.json", "design.json", "settings.json"] {
            assert!(
                !ctx.work_dir.join(stale).exists(),
                "`{stale}` must no longer be written",
            );
        }

        // The verbatim tapacc output is kept for debugging, byte-for-byte,
        // *without* analyze's injected cflags/target.
        let raw = fs::read_to_string(ctx.work_dir.join("tapacc.json")).expect("read tapacc.json");
        assert_eq!(
            raw.trim_end(),
            fixed_graph,
            "tapacc.json must hold tapacc's stdout verbatim",
        );
    }
}
