//! `tapa analyze` — native Rust port of `tapa/steps/analyze.py`.
//!
//! Composes `tapa-cpp` (preprocessor) and `tapacc` (semantic analyzer)
//! invocations, then writes `graph.json`, `design.json`, and
//! `settings.json` directly under `work_dir` using the Python-compatible
//! formatters. The Python CLI was retired in AC-8; this is the only
//! `analyze` path.

use std::fs;
use std::path::PathBuf;

use clap::Parser;
use serde_json::{json, Value};

use crate::context::CliContext;
use crate::error::{CliError, Result};
use crate::state::{design as design_io, graph as graph_io, settings as settings_io};
use crate::tapacc::cflags::{get_system_cflags, get_tapacc_cflags};
use crate::tapacc::discover::find_clang_binary;

mod build_design;
mod run_flatten;
mod run_tapacc;

use build_design::{build_design, flatten_graph_value, is_top_leaf};
use run_flatten::run_flatten;
use run_tapacc::run_tapacc;

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

    /// Target flow.
    #[arg(long = "target", default_value = "xilinx-vitis")]
    pub target: String,

    /// Explicit path to the `tapacc` binary. Overrides the walk-up
    /// `find_resource` search anchored at the `tapa` binary. Used by
    /// Bazel driver rules (`bazel/tapa_rules.bzl::_tapa_xo_impl`)
    /// that locate the toolchain inputs through their own dep graph
    /// and pass them down explicitly.
    #[arg(long = "tapacc", value_name = "FILE")]
    pub tapacc: Option<PathBuf>,

    /// Explicit path to the `tapa-cpp` (clang) binary. Same rationale
    /// as `--tapacc`. Accepts the `--tapa-clang` alias for parity with
    /// the Python-era bazel driver, which used that older spelling.
    #[arg(long = "tapa-cpp", visible_alias = "tapa-clang", value_name = "FILE")]
    pub tapa_cpp: Option<PathBuf>,
}

/// Re-render `args` as the click-flavored argv the Python step expects.
pub fn to_python_argv(args: &AnalyzeArgs) -> Vec<String> {
    let mut out = Vec::<String>::new();
    for f in &args.input_files {
        out.push("--input".to_string());
        out.push(f.display().to_string());
    }
    out.push("--top".to_string());
    out.push(args.top.clone());
    for c in &args.cflags {
        out.push("--cflags".to_string());
        out.push(c.clone());
    }
    if args.flatten_hierarchy {
        out.push("--flatten-hierarchy".to_string());
    } else {
        // Default Python behavior is `--keep-hierarchy`; emit it explicitly
        // so the bridged invocation sees the same boolean shape regardless
        // of whether the user passed `--keep-hierarchy` on the Rust side.
        out.push("--keep-hierarchy".to_string());
    }
    out.push("--target".to_string());
    out.push(args.target.clone());
    out
}

/// Top-level dispatcher for `tapa analyze` (always native; Python CLI
/// was retired in AC-8).
pub fn run(args: &AnalyzeArgs, ctx: &mut CliContext) -> Result<()> {
    run_native(args, ctx)
}

/// Native implementation. Mirrors `tapa.steps.analyze.analyze` minus the
/// `--flatten-hierarchy` transform and the heavy `Program` orchestration.
fn run_native(args: &AnalyzeArgs, ctx: &CliContext) -> Result<()> {
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

    // Vitis HLS only supports up to C++14; this matches the Python order.
    let mut user_cflags = args.cflags.clone();
    user_cflags.push("-std=c++14".to_string());

    let mut all_cflags = user_cflags.clone();
    all_cflags.extend(get_tapacc_cflags(false));
    all_cflags.extend(get_system_cflags());

    let work_dir = ctx.work_dir.as_path();
    fs::create_dir_all(work_dir)?;
    let flatten_files = run_flatten(&tapa_cpp, &args.input_files, &all_cflags, work_dir)?;
    let mut graph_dict = run_tapacc(
        &tapacc,
        &flatten_files,
        &args.top,
        &all_cflags,
        &args.target,
    )?;

    // Mirror Python: overwrite cflags with the user's tuple (with c++14).
    if let Some(obj) = graph_dict.as_object_mut() {
        obj.insert(
            "cflags".to_string(),
            Value::Array(user_cflags.iter().cloned().map(Value::String).collect()),
        );
    }

    if args.flatten_hierarchy {
        graph_dict = flatten_graph_value(&graph_dict)?;
    }

    if is_top_leaf(&graph_dict, &args.top) && args.target == "xilinx-vitis" {
        return Err(CliError::InvalidArg(
            "the top task is a leaf task; target `xilinx-vitis` is not supported \
             (Vitis requires an upper top for kernel.xml generation). \
             Rerun with `--target xilinx-hls`."
                .to_string(),
        ));
    }

    // Persist all three on-disk artifacts.
    graph_io::store_graph(work_dir, &graph_dict)?;
    let mut settings = settings_io::Settings::new();
    settings.insert("target".to_string(), json!(args.target.clone()));
    settings_io::store_settings(work_dir, &settings)?;
    let design = build_design(&args.top, &args.target, &graph_dict)?;
    design_io::store_design(work_dir, &design)?;

    // Cache state for downstream chained steps in this process.
    let mut flow = ctx.flow.borrow_mut();
    flow.graph = Some(graph_dict);
    flow.settings = Some(settings);
    flow.design = Some(design);
    flow.pipelined.insert("analyze".to_string(), true);
    drop(flow);

    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::similar_names,
        reason = "the `args`/`argv` pair appears throughout the dispatcher; \
                  matching the production names keeps tests legible"
    )]

    use super::*;

    use std::fs;

    use crate::context::CliContext;
    use crate::globals::GlobalArgs;

    #[test]
    fn argv_round_trips_python_shape() {
        let args = AnalyzeArgs::try_parse_from([
            "analyze",
            "--input",
            "vadd.cpp",
            "--top",
            "VecAdd",
            "--target",
            "xilinx-hls",
        ])
        .unwrap();
        let argv = to_python_argv(&args);
        assert!(argv.contains(&"--input".to_string()));
        assert!(argv.contains(&"vadd.cpp".to_string()));
        assert!(argv.contains(&"--top".to_string()));
        assert!(argv.contains(&"VecAdd".to_string()));
        assert!(argv.contains(&"--target".to_string()));
        assert!(argv.contains(&"xilinx-hls".to_string()));
    }

    #[test]
    fn to_python_argv_includes_keep_hierarchy_when_default() {
        let args = AnalyzeArgs::try_parse_from([
            "analyze",
            "--input",
            "vadd.cpp",
            "--top",
            "VecAdd",
        ])
        .unwrap();
        let argv = to_python_argv(&args);
        assert!(
            argv.contains(&"--keep-hierarchy".to_string()),
            "default analyze must propagate `--keep-hierarchy` to the bridge",
        );
        assert!(
            !argv.contains(&"--flatten-hierarchy".to_string()),
            "default analyze must not propagate `--flatten-hierarchy`",
        );
    }

    #[test]
    fn to_python_argv_includes_flatten_hierarchy_when_set() {
        let args = AnalyzeArgs::try_parse_from([
            "analyze",
            "--input",
            "vadd.cpp",
            "--top",
            "VecAdd",
            "--flatten-hierarchy",
        ])
        .unwrap();
        let argv = to_python_argv(&args);
        assert!(
            argv.contains(&"--flatten-hierarchy".to_string()),
            "`--flatten-hierarchy` must be forwarded to the bridge",
        );
        assert!(
            !argv.contains(&"--keep-hierarchy".to_string()),
            "the two boolean siblings must not both appear",
        );
    }

    #[cfg(unix)]
    #[test]
    fn native_run_writes_graph_design_settings() {
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
        fs::set_permissions(&tapa_cpp, fs::Permissions::from_mode(0o755))
            .expect("chmod tapa-cpp");

        // tapacc: `--version` is parseable; otherwise it emits a fixed
        // tapacc-shaped graph.json on stdout.
        let fixed_graph = r#"{"cflags": [], "tasks": {"VecAdd": {"code": "void VecAdd() {}", "level": "upper", "target": "hls", "ports": [], "tasks": {"Add": [{"step": 0, "args": {}}]}, "fifos": {}}, "Add": {"code": "void Add() {}", "level": "lower", "target": "hls", "ports": []}}, "top": "VecAdd"}"#;
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
        fs::set_permissions(&tapacc, fs::Permissions::from_mode(0o755))
            .expect("chmod tapacc");

        // Plant a trivial input file under the same root.
        let input_file = root.join("vadd.cpp");
        fs::write(&input_file, b"void VecAdd() {}\n").expect("write vadd.cpp");

        // Steer `find_resource` at `root` so the planted binaries win.
        std::env::set_var("TAPA_CLI_SEARCH_ANCHOR", root);
        // Make sure no parent invocation accidentally enabled the bridge.
        std::env::remove_var("TAPA_STEP_ANALYZE_PYTHON");

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

        run_native(&args, &ctx).expect("native analyze should succeed");

        // graph.json must contain the patched cflags and the tapacc tasks.
        let graph_path = ctx.work_dir.join("graph.json");
        assert!(graph_path.exists(), "graph.json must be written");
        let graph_v: Value =
            serde_json::from_str(&fs::read_to_string(&graph_path).expect("read graph"))
                .expect("parse graph.json");
        assert_eq!(graph_v["top"], json!("VecAdd"));
        // Native analyze overwrites cflags with the user tuple +
        // `-std=c++14`. The user passed no `-c`, so we expect just that.
        assert_eq!(graph_v["cflags"], json!(["-std=c++14"]));

        // settings.json must record the target.
        let settings = settings_io::load_settings(&ctx.work_dir).expect("load settings");
        assert_eq!(settings.get("target"), Some(&json!("xilinx-hls")));

        // design.json must round-trip with the projected topology.
        let design = design_io::load_design(&ctx.work_dir).expect("load design");
        assert_eq!(design.top, "VecAdd");
        assert_eq!(design.target, "xilinx-hls");
        assert!(design.tasks.contains_key("VecAdd"));
        assert!(design.tasks.contains_key("Add"));
        assert_eq!(design.tasks["VecAdd"].level, "upper");
        assert_eq!(design.tasks["Add"].level, "lower");
        assert!(design.slot_task_name_to_fp_region.is_none());

        // FlowState must cache all three artifacts.
        let flow = ctx.flow.borrow();
        assert!(flow.design.is_some(), "design cached for chained steps");
        assert!(flow.graph.is_some(), "graph cached for chained steps");
        assert!(flow.settings.is_some(), "settings cached for chained steps");
        assert_eq!(flow.pipelined.get("analyze"), Some(&true));
    }
}
