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
    source: &'static str,
    top: &'static str,
    expected_tasks: &'static [&'static str],
    requires_vendor: bool,
}

const ANALYZE_APPS: &[AnalyzeApp] = &[
    AnalyzeApp {
        name: "vadd",
        source: "tests/apps/vadd/vadd.cpp",
        top: "VecAdd",
        expected_tasks: &["VecAdd", "Mmap2Stream", "Add", "Stream2Mmap"],
        requires_vendor: false,
    },
    AnalyzeApp {
        name: "bandwidth",
        source: "tests/apps/bandwidth/bandwidth.cpp",
        top: "Bandwidth",
        expected_tasks: &["Bandwidth"],
        requires_vendor: true,
    },
    AnalyzeApp {
        name: "cannon",
        source: "tests/apps/cannon/cannon.cpp",
        top: "Cannon",
        expected_tasks: &["Cannon", "Gather", "ProcElem", "Scatter"],
        requires_vendor: false,
    },
    AnalyzeApp {
        name: "gemv",
        source: "tests/apps/gemv/gemv.cpp",
        top: "Gemv",
        expected_tasks: &["Gemv"],
        requires_vendor: true,
    },
    AnalyzeApp {
        name: "graph",
        source: "tests/apps/graph/graph.cpp",
        top: "Graph",
        expected_tasks: &["Graph", "Control", "ProcElem", "UpdateHandler"],
        requires_vendor: false,
    },
    AnalyzeApp {
        name: "jacobi",
        source: "tests/apps/jacobi/jacobi.cpp",
        top: "Jacobi",
        expected_tasks: &["Jacobi", "Mmap2Stream", "Stream2Mmap"],
        requires_vendor: false,
    },
    AnalyzeApp {
        name: "network",
        source: "tests/apps/network/network.cpp",
        top: "Network",
        expected_tasks: &["Network", "Consume", "Produce", "Switch2x2"],
        requires_vendor: false,
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
    let source = workspace_path(app.source);
    let work_dir = TempDir::with_prefix(&format!("tapa-analyze-{}-", app.name))
        .map_err(|error| format!("failed to create temp dir: {error}"))?;
    let source_dir = source
        .parent()
        .ok_or_else(|| format!("source has no parent: {}", source.display()))?;

    let output = Command::new(tapa)
        .arg("--work-dir")
        .arg(work_dir.path())
        .arg("analyze")
        .arg("--input")
        .arg(&source)
        .arg("--top")
        .arg(app.top)
        .arg("--cflags")
        .arg(format!("-I{}", source_dir.display()))
        .arg("--cflags")
        .arg(format!("-I{}", tapa_lib.display()))
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

    let graph_path = work_dir.path().join("graph.json");
    require_file(&graph_path)?;
    require_file(&work_dir.path().join("settings.json"))?;
    require_dir(&work_dir.path().join("flatten"))?;

    let graph = read_json(&graph_path)?;
    validate_graph(&graph, app)
}

fn validate_graph(graph: &JsonValue, app: &AnalyzeApp) -> Result<()> {
    let tasks = graph
        .get("tasks")
        .and_then(JsonValue::as_object)
        .ok_or_else(|| format!("{}: graph.json missing object 'tasks'", app.name))?;
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
        validate_task(task, task_name, app.name)?;
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

fn validate_task(task: &JsonValue, task_name: &str, app_name: &str) -> Result<()> {
    let ctx = format!("{app_name}/{task_name}");
    let level = task
        .get("level")
        .and_then(JsonValue::as_str)
        .ok_or_else(|| format!("{ctx}: missing string 'level'"))?;
    if !VALID_LEVELS.contains(&level) {
        return Err(format!("{ctx}: invalid level '{level}'"));
    }
    require_key(task, "target", &ctx)?;
    let code = task
        .get("code")
        .and_then(JsonValue::as_str)
        .ok_or_else(|| format!("{ctx}: missing string 'code'"))?;
    if code.is_empty() {
        return Err(format!("{ctx}: empty code"));
    }
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
