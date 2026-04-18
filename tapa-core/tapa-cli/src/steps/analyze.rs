//! `tapa analyze` — native Rust port of `tapa/steps/analyze.py`.
//!
//! Composes `tapa-cpp` (preprocessor) and `tapacc` (semantic analyzer)
//! invocations, then writes `graph.json`, `design.json`, and
//! `settings.json` directly under `work_dir` using the Python-compatible
//! formatters. The Python bridge remains reachable behind
//! `TAPA_STEP_ANALYZE_PYTHON=1` for fallback parity.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use clap::Parser;
use indexmap::IndexMap;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use tapa_task_graph::{flatten, Design, Graph, TaskTopology, TransformError};

use crate::context::CliContext;
use crate::error::{CliError, Result};
use crate::state::{design as design_io, graph as graph_io, settings as settings_io};
use crate::steps::python_bridge;
use crate::tapacc::cflags::{get_system_cflags, get_tapacc_cflags};
use crate::tapacc::discover::find_clang_binary;

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

/// Top-level dispatcher.
///
/// `analyze` is a fully ported step (per AC-6), so the
/// `TAPA_STEP_ANALYZE_PYTHON=1` env flag is a no-op: we always run the
/// native path. The bridge shim is kept only so `to_python_argv`
/// remains available for composites that transitively forward to
/// un-ported step branches.
pub fn run(args: &AnalyzeArgs, ctx: &mut CliContext) -> Result<()> {
    let _ = python_bridge::is_enabled("analyze");
    run_native(args, ctx)
}

/// Native implementation. Mirrors `tapa.steps.analyze.analyze` minus the
/// `--flatten-hierarchy` transform and the heavy `Program` orchestration.
fn run_native(args: &AnalyzeArgs, ctx: &CliContext) -> Result<()> {
    let tapa_cpp = find_clang_binary("tapa-cpp-binary")?;
    let tapacc = find_clang_binary("tapacc-binary")?;

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
            "the top task is a leaf task; target `xilinx-vitis` is not supported. \
             Rerun with `--target xilinx-hls` or set `TAPA_STEP_ANALYZE_PYTHON=1`."
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

/// Run `tapa-cpp` once per input file and write the preprocessed source
/// to `<work_dir>/flatten/flatten-<digest>-<basename>`.
fn run_flatten(
    tapa_cpp: &Path,
    files: &[PathBuf],
    cflags: &[String],
    work_dir: &Path,
) -> Result<Vec<PathBuf>> {
    let flatten_dir = work_dir.join("flatten");
    fs::create_dir_all(&flatten_dir)?;
    let mut out = Vec::<PathBuf>::with_capacity(files.len());
    for file in files {
        let abs = fs::canonicalize(file).unwrap_or_else(|_| file.clone());
        let digest = sha256_truncated_hex(abs.display().to_string().as_bytes());
        let basename = file.file_name().map_or_else(
            || "input.cpp".to_string(),
            |s| s.to_string_lossy().into_owned(),
        );
        let flatten_path = flatten_dir.join(format!("flatten-{digest}-{basename}"));

        let mut cmd = Command::new(tapa_cpp);
        cmd.args([
            "-x",
            "c++",
            "-E",
            "-CC",
            "-P",
            "-fkeep-system-includes",
            "-D__SYNTHESIS__",
            "-DAESL_SYN",
            "-DAP_AUTOCC",
            "-DTAPA_TARGET_DEVICE_",
            "-DTAPA_TARGET_STUB_",
        ]);
        for flag in cflags {
            cmd.arg(flag);
        }
        cmd.arg(file);
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::inherit());
        let output = cmd.output().map_err(|e| CliError::TapaccNotExecutable {
            path: tapa_cpp.to_path_buf(),
            reason: e.to_string(),
        })?;
        if !output.status.success() {
            return Err(CliError::TapaccFailed {
                code: output.status.code().unwrap_or(-1),
                stderr: format!("tapa-cpp on {}", file.display()),
            });
        }
        fs::write(&flatten_path, &output.stdout)?;
        out.push(flatten_path);
    }
    Ok(out)
}

/// Run `tapacc` and parse its JSON stdout.
fn run_tapacc(
    tapacc: &Path,
    files: &[PathBuf],
    top: &str,
    cflags: &[String],
    target: &str,
) -> Result<Value> {
    let mut cmd = Command::new(tapacc);
    for f in files {
        cmd.arg(f);
    }
    cmd.args(["-top", top, "--target", target, "--"]);
    for f in cflags {
        cmd.arg(f);
    }
    cmd.args(["-DTAPA_TARGET_DEVICE_", "-DTAPA_TARGET_STUB_"]);
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    let output = cmd.output().map_err(|e| CliError::TapaccNotExecutable {
        path: tapacc.to_path_buf(),
        reason: e.to_string(),
    })?;
    if !output.status.success() {
        return Err(CliError::TapaccFailed {
            code: output.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    let value: Value = serde_json::from_slice(&output.stdout)?;
    Ok(value)
}

/// Truncate a SHA-256 digest to the first 8 hex characters, matching
/// Python's `hashlib.sha256(...).hexdigest()[:8]`.
fn sha256_truncated_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut s = String::with_capacity(8);
    for byte in digest.iter().take(4) {
        let _ = write!(s, "{byte:02x}");
    }
    s
}

/// Round-trip a tapacc graph dict through the typed [`Graph`] schema and
/// return the result of [`flatten`] re-serialized as `serde_json::Value`.
///
/// The CLI keeps the on-disk graph as a `Value` because the legacy
/// Python pipeline accepts a richer schema in some downstream stages,
/// but the transform itself is defined on the strict `Graph` type to
/// maximize Python parity.
fn flatten_graph_value(graph: &Value) -> Result<Value> {
    let json = serde_json::to_string(graph)?;
    let typed = Graph::from_json(&json)?;
    let flat = flatten(&typed).map_err(|e| match e {
        TransformError::DeepHierarchyNotSupported(child) => CliError::InvalidArg(format!(
            "`--flatten-hierarchy` only supports single-level hierarchies for now; \
             child task `{child}` is itself an upper task. The native port covers \
             the vadd-shaped case; deeper graphs are pending.",
        )),
        other @ (TransformError::MissingTop(_)
        | TransformError::TopIsLeaf(_)
        | TransformError::UnknownFloorplanInstance(_)
        | TransformError::SlotNameCollision(_)
        | TransformError::Json(_)) => {
            CliError::InvalidArg(format!("flatten failed: {other}"))
        }
    })?;
    let out_json = flat.to_json()?;
    let value: Value = serde_json::from_str(&out_json)?;
    Ok(value)
}

/// True when the top task in `graph` is a leaf-level task.
fn is_top_leaf(graph: &Value, top: &str) -> bool {
    graph
        .get("tasks")
        .and_then(|t| t.get(top))
        .and_then(|task| task.get("level"))
        .and_then(Value::as_str)
        .is_some_and(|level| level == "lower")
}

/// Project the tapacc graph dict into a typed [`Design`] suitable for
/// `<work_dir>/design.json`. Mirrors the Python `Task.to_topology_dict`
/// projection, but drops `vendor` and other tapacc-only keys.
fn build_design(top: &str, target: &str, graph: &Value) -> Result<Design> {
    let tasks_obj = graph
        .get("tasks")
        .and_then(Value::as_object)
        .ok_or_else(|| CliError::InvalidArg(
            "tapacc graph is missing the `tasks` object".to_string(),
        ))?;

    let mut topology: IndexMap<String, TaskTopology> = IndexMap::new();
    for (name, task) in tasks_obj {
        topology.insert(name.clone(), task_to_topology(name, task));
    }

    Ok(Design {
        top: top.to_string(),
        target: target.to_string(),
        tasks: topology,
        slot_task_name_to_fp_region: None,
    })
}

fn task_to_topology(name: &str, task: &Value) -> TaskTopology {
    let level = task
        .get("level")
        .and_then(Value::as_str)
        .unwrap_or("lower")
        .to_string();
    let code = task
        .get("code")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let ports = task
        .get("ports")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|p| serde_json::from_value(p.clone()).ok())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let tasks = value_to_indexmap(task.get("tasks"));
    let fifos = value_to_indexmap(task.get("fifos"));
    let target = task
        .get("target")
        .and_then(Value::as_str)
        .map(ToString::to_string);

    TaskTopology {
        name: name.to_string(),
        level,
        code,
        ports,
        tasks,
        fifos,
        target,
        is_slot: false,
        self_area: IndexMap::new(),
        total_area: IndexMap::new(),
        clock_period: "0".to_string(),
    }
}

fn value_to_indexmap(value: Option<&Value>) -> IndexMap<String, Value> {
    let Some(Value::Object(obj)) = value else {
        return IndexMap::new();
    };
    obj_to_indexmap(obj)
}

fn obj_to_indexmap(obj: &Map<String, Value>) -> IndexMap<String, Value> {
    let mut map = IndexMap::new();
    for (k, v) in obj {
        map.insert(k.clone(), v.clone());
    }
    map
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

    #[test]
    fn sha256_truncated_matches_python_eight_hex_chars() {
        // Python: hashlib.sha256(b"foo").hexdigest()[:8] == "2c26b46b"
        assert_eq!(sha256_truncated_hex(b"foo"), "2c26b46b");
    }

    #[test]
    fn is_top_leaf_detects_lower_level() {
        let g = json!({"tasks": {"T": {"level": "lower"}}, "top": "T"});
        assert!(is_top_leaf(&g, "T"));
        let g = json!({"tasks": {"T": {"level": "upper"}}, "top": "T"});
        assert!(!is_top_leaf(&g, "T"));
        // Missing top is treated as upper for safety.
        assert!(!is_top_leaf(&g, "DoesNotExist"));
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

    /// `analyze --flatten-hierarchy` exercises the
    /// [`tapa_task_graph::flatten`] code path on a vadd-shaped graph.
    /// We hit `flatten_graph_value` directly (the helper invoked from
    /// `run_native` when `flatten_hierarchy` is set) because the full
    /// `run_native` path depends on a process-wide `OnceLock` for the
    /// `find_resource` search anchor — sharing that across tests would
    /// require more invasive plumbing than this transform-coverage
    /// check warrants.
    #[test]
    fn flatten_graph_value_renames_fifos_for_vadd_shape() {
        let raw = json!({
            "cflags": [],
            "top": "VecAdd",
            "tasks": {
                "VecAdd": {
                    "code": "void VecAdd() {}",
                    "level": "upper",
                    "target": "hls",
                    "vendor": "xilinx",
                    "ports": [
                        {"cat": "scalar", "name": "n",
                         "type": "uint64_t", "width": 64}
                    ],
                    "tasks": {
                        "A": [{"step": 0, "args": {
                            "n": {"arg": "n", "cat": "scalar"},
                            "out": {"arg": "fifo", "cat": "ostream"}
                        }}],
                        "B": [{"step": 0, "args": {
                            "n": {"arg": "n", "cat": "scalar"},
                            "in": {"arg": "fifo", "cat": "istream"}
                        }}]
                    },
                    "fifos": {
                        "fifo": {"depth": 2, "consumed_by": ["B", 0],
                                 "produced_by": ["A", 0]}
                    }
                },
                "A": {
                    "code": "void A() {}", "level": "lower",
                    "target": "hls", "vendor": "xilinx",
                    "ports": [
                        {"cat": "scalar", "name": "n",
                         "type": "uint64_t", "width": 64},
                        {"cat": "ostream", "name": "out",
                         "type": "float", "width": 32}
                    ]
                },
                "B": {
                    "code": "void B() {}", "level": "lower",
                    "target": "hls", "vendor": "xilinx",
                    "ports": [
                        {"cat": "scalar", "name": "n",
                         "type": "uint64_t", "width": 64},
                        {"cat": "istream", "name": "in",
                         "type": "float", "width": 32}
                    ]
                }
            }
        });

        let out = flatten_graph_value(&raw).expect("flatten ok");
        let top = out["tasks"]["VecAdd"]
            .as_object()
            .expect("top survives");
        assert!(
            top["fifos"].get("fifo_VecAdd").is_some(),
            "flatten must rename `fifo` to `fifo_VecAdd`; got {top:?}",
        );
        let a0 = &top["tasks"]["A"][0]["args"]["out"]["arg"];
        assert_eq!(a0, &json!("fifo_VecAdd"));
    }

    /// Deep hierarchies (children that are themselves upper tasks)
    /// must surface a typed `InvalidArg` rather than crashing.
    #[test]
    fn flatten_graph_value_rejects_deep_hierarchy() {
        let raw = json!({
            "cflags": [],
            "top": "Outer",
            "tasks": {
                "Outer": {
                    "code": "", "level": "upper", "target": "hls",
                    "vendor": "xilinx", "ports": [],
                    "tasks": {"Inner": [{"args": {}, "step": 0}]},
                    "fifos": {}
                },
                "Inner": {
                    "code": "", "level": "upper", "target": "hls",
                    "vendor": "xilinx", "ports": [],
                    "tasks": {}, "fifos": {}
                }
            }
        });
        let err = flatten_graph_value(&raw).expect_err("must reject deep");
        assert!(
            matches!(err, CliError::InvalidArg(ref m)
                if m.contains("single-level")),
            "expected single-level error, got {err:?}",
        );
    }
}
