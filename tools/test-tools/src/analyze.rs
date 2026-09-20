use serde_json::Value as JsonValue;
use std::env;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

use crate::common::{read_json, require_dir, require_file, workspace_path, Result};

const VALID_PORT_CATS: &[&str] = &[
    "istream", "istreams", "ostream", "ostreams", "mmap", "scalar",
];
const VALID_LEVELS: &[&str] = &["upper", "lower"];

#[derive(Clone, Copy)]
struct AnalyzeApp {
    name: &'static str,
    /// The kernel translation units, one `analyze --input` each.
    sources: &'static [&'static str],
    top: &'static str,
    expected_tasks: &'static [&'static str],
    requires_vendor: bool,
    /// Extra `--cflags` values appended after the standard include flags.
    /// An `-I` flag carries a workspace-relative dir, resolved at runtime.
    extra_cflags: &'static [&'static str],
}

const ANALYZE_APPS: &[AnalyzeApp] = &[
    AnalyzeApp {
        name: "vadd",
        sources: &["tests/apps/vadd/vadd.cpp"],
        top: "VecAdd",
        expected_tasks: &["VecAdd", "Mmap2Stream", "Add", "Stream2Mmap"],
        requires_vendor: false,
        extra_cflags: &[],
    },
    AnalyzeApp {
        name: "bandwidth",
        sources: &["tests/apps/bandwidth/bandwidth.cpp"],
        top: "Bandwidth",
        expected_tasks: &["Bandwidth"],
        requires_vendor: true,
        extra_cflags: &[],
    },
    AnalyzeApp {
        name: "cannon",
        sources: &["tests/apps/cannon/cannon.cpp"],
        top: "Cannon",
        expected_tasks: &["Cannon", "Gather", "ProcElem", "Scatter"],
        requires_vendor: false,
        extra_cflags: &[],
    },
    AnalyzeApp {
        name: "gemv",
        sources: &["tests/apps/gemv/gemv.cpp"],
        top: "Gemv",
        expected_tasks: &["Gemv"],
        requires_vendor: true,
        extra_cflags: &[],
    },
    AnalyzeApp {
        name: "graph",
        sources: &["tests/apps/graph/graph.cpp"],
        top: "Graph",
        expected_tasks: &["Graph", "Control", "ProcElem", "UpdateHandler"],
        requires_vendor: false,
        extra_cflags: &[],
    },
    AnalyzeApp {
        name: "jacobi",
        sources: &["tests/apps/jacobi/jacobi.cpp"],
        top: "Jacobi",
        expected_tasks: &["Jacobi", "Mmap2Stream", "Stream2Mmap"],
        requires_vendor: false,
        extra_cflags: &[],
    },
    AnalyzeApp {
        name: "network",
        sources: &["tests/apps/network/network.cpp"],
        top: "Network",
        expected_tasks: &["Network", "Consume", "Produce", "Switch2x2"],
        requires_vendor: false,
        extra_cflags: &[],
    },
    AnalyzeApp {
        name: "multi-file",
        sources: &["tests/apps/multi-file/a.cpp", "tests/apps/multi-file/b.cpp"],
        top: "MultiFileTop",
        // The last key is the Combine<float> instantiation; tapacc keys
        // template tasks by their mangled name.
        expected_tasks: &[
            "MultiFileTop",
            "Produce",
            "Consume",
            "tapa_mangled_Z7CombineIfEvRN4tapa7istreamIT_EES4_RNS0_7ostreamIS2_EEm",
        ],
        requires_vendor: false,
        extra_cflags: &["-Itests/apps/multi-file-ext"],
    },
];

pub fn analyze_smoke() -> Result<()> {
    let tapa = workspace_path("tapa-core/tapa");
    let tapa_lib = workspace_path("tapa-lib");
    let has_vendor = env::var_os("XILINX_HLS").is_some() || env::var_os("XILINX_VITIS").is_some();

    for app in ANALYZE_APPS {
        if app.requires_vendor && !has_vendor {
            eprintln!("SKIP {}: requires Vitis HLS vendor headers", app.name);
            continue;
        }
        run_analyze_app(app, &tapa, &tapa_lib)?;
    }
    Ok(())
}

fn run_analyze_app(app: &AnalyzeApp, tapa: &Path, tapa_lib: &Path) -> Result<()> {
    let sources: Vec<_> = app
        .sources
        .iter()
        .map(|source| workspace_path(source))
        .collect();
    for source in &sources {
        require_file(source)?;
    }
    let work_dir = TempDir::with_prefix(format!("tapa-analyze-{}-", app.name))
        .map_err(|error| format!("failed to create temp dir: {error}"))?;
    let source_dir = sources
        .first()
        .and_then(|source| source.parent())
        .ok_or_else(|| format!("{}: source has no parent", app.name))?;

    let mut command = Command::new(tapa);
    command
        .arg("--work-dir")
        .arg(work_dir.path())
        .arg("analyze");
    for source in &sources {
        command.arg("--input").arg(source);
    }
    command
        .arg("--top")
        .arg(app.top)
        .arg("--cflags")
        .arg(format!("-I{}", source_dir.display()))
        .arg("--cflags")
        .arg(format!("-I{}", tapa_lib.display()));
    for flag in app.extra_cflags {
        let value = match flag.strip_prefix("-I") {
            Some(dir) => format!("-I{}", workspace_path(dir).display()),
            None => (*flag).to_string(),
        };
        command.arg("--cflags").arg(value);
    }
    let output = command
        .output()
        .map_err(|error| format!("failed to run {}: {error}", tapa.display()))?;

    if !output.status.success() {
        return Err(format!(
            "tapa analyze failed for {}\nstdout:\n{}\nstderr:\n{}",
            app.name,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    // `tapa analyze` persists exactly one state file plus the verbatim
    // `tapacc` output kept as a debug artifact, and the mirrored source
    // tree the task manifests point into.
    let state_path = work_dir.path().join("tapa.json");
    require_file(&state_path)?;
    require_file(&work_dir.path().join("tapacc.json"))?;
    require_dir(&work_dir.path().join("rewritten"))?;

    let state = read_json(&state_path)?;
    let graph = state
        .get("graph")
        .ok_or_else(|| format!("{}: tapa.json missing 'graph'", app.name))?;
    validate_graph(graph, work_dir.path(), app)
}

fn validate_graph(graph: &JsonValue, work_dir: &Path, app: &AnalyzeApp) -> Result<()> {
    let tasks = graph
        .get("tasks")
        .and_then(JsonValue::as_object)
        .ok_or_else(|| format!("{}: tapa.json graph missing object 'tasks'", app.name))?;
    if tasks.is_empty() {
        return Err(format!("{}: graph contains no tasks", app.name));
    }
    for expected in app.expected_tasks {
        if !tasks.contains_key(*expected) {
            let found: Vec<&String> = tasks.keys().collect();
            return Err(format!(
                "{}: expected task '{expected}' not found; found {found:?}",
                app.name
            ));
        }
    }
    for (task_name, task) in tasks {
        validate_task(task, task_name, work_dir, app.name)?;
    }
    let top_level = tasks
        .get(app.top)
        .and_then(|task| task.get("level"))
        .and_then(JsonValue::as_str);
    if top_level != Some("upper") {
        return Err(format!(
            "{}: top task '{}' should be upper-level",
            app.name, app.top
        ));
    }
    Ok(())
}

fn validate_task(task: &JsonValue, task_name: &str, work_dir: &Path, app_name: &str) -> Result<()> {
    let ctx = format!("{app_name}/{task_name}");
    let level = task
        .get("level")
        .and_then(JsonValue::as_str)
        .ok_or_else(|| format!("{ctx}: missing string 'level'"))?;
    if !VALID_LEVELS.contains(&level) {
        return Err(format!("{ctx}: invalid level '{level}'"));
    }
    // Per-task synthesis policy. Named `synth` since the flow target moved
    // to the graph root: one fact, one field.
    require_key(task, "synth", &ctx)?;
    // Per-task source manifest: every src names a file the
    // analyze step actually wrote under the rewritten tree.
    let srcs = task
        .get("srcs")
        .and_then(JsonValue::as_array)
        .ok_or_else(|| format!("{ctx}: missing array 'srcs'"))?;
    if srcs.is_empty() {
        return Err(format!("{ctx}: empty srcs"));
    }
    for src in srcs {
        let src = src
            .as_str()
            .ok_or_else(|| format!("{ctx}: srcs entries must be strings, got {src}"))?;
        require_file(&work_dir.join("rewritten").join(src))
            .map_err(|e| format!("{ctx}: manifest src does not exist: {e}"))?;
    }
    require_key(task, "include_dirs", &ctx)?;
    require_key(task, "defines", &ctx)?;
    let ports = task
        .get("ports")
        .and_then(JsonValue::as_array)
        .ok_or_else(|| format!("{ctx}: missing array 'ports'"))?;
    if ports.is_empty() {
        return Err(format!("{ctx}: no ports"));
    }
    for port in ports {
        let port_name = port
            .get("name")
            .and_then(JsonValue::as_str)
            .ok_or_else(|| format!("{ctx}: port missing string 'name'"))?;
        let cat = port
            .get("cat")
            .and_then(JsonValue::as_str)
            .ok_or_else(|| format!("{ctx}/{port_name}: missing string 'cat'"))?;
        if !VALID_PORT_CATS.contains(&cat) {
            return Err(format!("{ctx}/{port_name}: invalid cat '{cat}'"));
        }
    }
    if level == "upper" {
        let subtasks = task
            .get("tasks")
            .and_then(JsonValue::as_object)
            .ok_or_else(|| format!("{ctx}: upper-level task missing object 'tasks'"))?;
        if subtasks.is_empty() {
            return Err(format!("{ctx}: upper-level task has no subtasks"));
        }
        require_key(task, "fifos", &ctx)?;
    }
    Ok(())
}

fn require_key(value: &JsonValue, key: &str, ctx: &str) -> Result<()> {
    if value.get(key).is_none() {
        return Err(format!("{ctx}: missing '{key}'"));
    }
    Ok(())
}
