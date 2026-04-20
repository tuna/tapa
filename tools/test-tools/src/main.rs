use serde_json::Value as JsonValue;
use std::collections::BTreeMap;
use std::env;
use std::ffi::OsString;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{self, Command};
use tempfile::TempDir;
use zip::ZipArchive;

type Result<T> = std::result::Result<T, String>;

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

const REPORT_FILES: &[&str] = &[
    "report.json",
    "report.yaml",
    "report/Add/Add_csynth.xml",
    "report/Mmap2Stream/Impl_csynth.xml",
    "report/Mmap2Stream/Mmap2Stream_csynth.xml",
    "report/Stream2Mmap/Impl_csynth.xml",
    "report/Stream2Mmap/Stream2Mmap_csynth.xml",
    "report/VecAdd/VecAdd_csynth.xml",
];

const REPORT_MODULES: &[&str] = &["Add", "Mmap2Stream", "Stream2Mmap"];

const SLOT_FORMAT_PREFIX: &str = "SLOT_X";
const FLOORPLAN_SEED0_SLOTS: &[(u64, u64)] = &[
    (3, 3),
    (0, 2),
    (3, 3),
    (2, 3),
    (2, 1),
    (1, 2),
    (1, 0),
    (2, 1),
    (2, 0),
    (0, 2),
    (3, 0),
    (2, 3),
    (2, 1),
    (3, 3),
    (2, 0),
    (0, 0),
    (3, 0),
];

const FLOORPLAN_LEAVES: &[(&str, &[&str])] = &[
    (
        "bandwidth",
        &[
            "Bandwidth_fsm",
            "Copy_0",
            "Copy_1",
            "Copy_2",
            "Copy_3",
            "chan_0",
            "chan_1",
            "chan_2",
            "chan_3",
        ],
    ),
    (
        "cannon",
        &[
            "Gather_0",
            "ProcElem_0",
            "ProcElem_1",
            "ProcElem_2",
            "ProcElem_3",
            "Scatter_0",
            "Scatter_1",
            "a_vec",
            "b_vec",
            "b_vec",
        ],
    ),
    (
        "gemv",
        &["GemvCore_0", "GemvCore_1", "mat_a", "vec_x", "vec_y"],
    ),
    (
        "graph",
        &[
            "Control_0",
            "Graph_fsm",
            "ProcElem_0",
            "UpdateHandler_0",
            "edges",
            "num_edges",
            "num_vertices",
            "updates",
            "vertices",
        ],
    ),
    (
        "jacobi",
        &[
            "Jacobi_fsm",
            "Mmap2Stream_0",
            "Module0Func_0",
            "Module1Func_0",
            "Module1Func_1",
            "Module1Func_2",
            "Module1Func_3",
            "Module1Func_4",
            "Module1Func_5",
            "Module3Func1_0",
            "Module3Func2_0",
            "Module6Func1_0",
            "Module6Func2_0",
            "Module8Func_0",
            "Stream2Mmap_0",
            "bank_0_t0",
            "bank_0_t1",
        ],
    ),
    (
        "network",
        &[
            "Consume_0",
            "Network_fsm",
            "Produce_0",
            "Switch2x2_0",
            "Switch2x2_1",
            "Switch2x2_10",
            "Switch2x2_11",
            "Switch2x2_2",
            "Switch2x2_3",
            "Switch2x2_4",
            "Switch2x2_5",
            "Switch2x2_6",
            "Switch2x2_7",
            "Switch2x2_8",
            "Switch2x2_9",
            "mmap_in",
            "mmap_out",
        ],
    ),
    (
        "shared_mmap",
        &[
            "Add_0",
            "Mmap2Stream_0",
            "Mmap2Stream_1",
            "Stream2Mmap_0",
            "VecAddShared_fsm",
            "elems",
        ],
    ),
    (
        "vadd",
        &[
            "Add_0",
            "Mmap2Stream_0",
            "Mmap2Stream_1",
            "Stream2Mmap_0",
            "VecAdd_fsm",
            "a",
            "b",
            "c",
        ],
    ),
];

fn main() {
    if let Err(error) = run(env::args_os().skip(1).collect()) {
        eprintln!("{error}");
        process::exit(1);
    }
}

fn run(args: Vec<OsString>) -> Result<()> {
    let Some(command) = args.first().and_then(|arg| arg.to_str()) else {
        return Err(usage());
    };
    match command {
        "analyze-smoke" => analyze_smoke(),
        "compare-abgraph" => {
            let name = arg_str(&args, 1, "compare-abgraph <name>")?;
            compare_abgraph(name)
        }
        "gen-floorplan" => {
            let index = arg_str(&args, 1, "gen-floorplan <index> <app> <output>")?
                .parse::<u64>()
                .map_err(|error| format!("invalid floorplan index: {error}"))?;
            let app = arg_str(&args, 2, "gen-floorplan <index> <app> <output>")?;
            let output = arg_path(&args, 3, "gen-floorplan <index> <app> <output>")?;
            gen_floorplan(index, app, &output)
        }
        "check-xo-reports" => {
            let path = arg_str(&args, 1, "check-xo-reports <workspace-path>")?;
            check_xo_reports(&workspace_path(path))
        }
        _ => Err(usage()),
    }
}

fn usage() -> String {
    "usage: tapa-test-tools <analyze-smoke|compare-abgraph|gen-floorplan|check-xo-reports> ..."
        .to_string()
}

fn arg_str<'a>(args: &'a [OsString], index: usize, usage: &str) -> Result<&'a str> {
    args.get(index)
        .and_then(|arg| arg.to_str())
        .ok_or_else(|| format!("usage: tapa-test-tools {usage}"))
}

fn arg_path(args: &[OsString], index: usize, usage: &str) -> Result<PathBuf> {
    args.get(index)
        .map(PathBuf::from)
        .ok_or_else(|| format!("usage: tapa-test-tools {usage}"))
}

fn workspace_path(rel: &str) -> PathBuf {
    let rel = rel.trim_start_matches("_main/");
    let path = Path::new(rel);
    if path.exists() {
        return path.to_path_buf();
    }
    let workspace = env::var("TEST_WORKSPACE").unwrap_or_else(|_| "_main".to_string());
    for env_var in ["TEST_SRCDIR", "RUNFILES_DIR"] {
        if let Ok(base) = env::var(env_var) {
            for candidate in [
                Path::new(&base).join(&workspace).join(rel),
                Path::new(&base).join("_main").join(rel),
                Path::new(&base).join(rel),
            ] {
                if candidate.exists() {
                    return candidate;
                }
            }
        }
    }
    path.to_path_buf()
}

fn analyze_smoke() -> Result<()> {
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

#[derive(Debug, PartialEq, Eq)]
struct NormalizedAbgraph {
    vertices: Vec<String>,
    edges: Vec<(i64, i64, String, String)>,
}

fn compare_abgraph(name: &str) -> Result<()> {
    let actual = workspace_path(&format!(
        "tests/functional/abgraph/{name}-abgraph-json.json"
    ));
    let golden = workspace_path(&format!("tests/functional/abgraph/golden/{name}.json"));
    let actual = normalize_abgraph(&read_json(&actual)?)?;
    let golden = normalize_abgraph(&read_json(&golden)?)?;
    if actual != golden {
        return Err(format!("{name}: generated ABGraph does not match golden"));
    }
    Ok(())
}

fn normalize_abgraph(graph: &JsonValue) -> Result<NormalizedAbgraph> {
    let vertices = graph
        .get("vs")
        .and_then(JsonValue::as_array)
        .ok_or_else(|| "ABGraph missing array 'vs'".to_string())?
        .iter()
        .map(|vertex| string_field(vertex, "name"))
        .collect::<Result<Vec<_>>>()?;
    let mut vertices = vertices;
    vertices.sort();

    let edges = graph
        .get("es")
        .and_then(JsonValue::as_array)
        .ok_or_else(|| "ABGraph missing array 'es'".to_string())?
        .iter()
        .map(|edge| {
            Ok((
                int_field(edge, "index")?,
                int_field(edge, "width")?,
                string_field(
                    edge.get("source_vertex")
                        .ok_or_else(|| "edge missing source_vertex".to_string())?,
                    "name",
                )?,
                string_field(
                    edge.get("target_vertex")
                        .ok_or_else(|| "edge missing target_vertex".to_string())?,
                    "name",
                )?,
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    let mut edges = edges;
    edges.sort();

    Ok(NormalizedAbgraph { vertices, edges })
}

fn string_field(value: &JsonValue, key: &str) -> Result<String> {
    value
        .get(key)
        .and_then(JsonValue::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| format!("missing string field '{key}'"))
}

fn int_field(value: &JsonValue, key: &str) -> Result<i64> {
    value
        .get(key)
        .and_then(JsonValue::as_i64)
        .ok_or_else(|| format!("missing integer field '{key}'"))
}

fn gen_floorplan(index: u64, app: &str, output: &Path) -> Result<()> {
    let leaves = FLOORPLAN_LEAVES
        .iter()
        .find_map(|(name, leaves)| (*name == app).then_some(*leaves))
        .ok_or_else(|| format!("unknown floorplan app '{app}'"))?;
    let floorplan = leaves
        .iter()
        .enumerate()
        .map(|(i, leaf)| {
            let (x, y) = floorplan_slot(index, i);
            (
                (*leaf).to_string(),
                format!("{SLOT_FORMAT_PREFIX}{x}Y{y}:{SLOT_FORMAT_PREFIX}{x}Y{y}"),
            )
        })
        .collect::<BTreeMap<_, _>>();
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
    }
    let data = serde_json::to_string_pretty(&floorplan)
        .map_err(|error| format!("failed to encode floorplan: {error}"))?;
    fs::write(output, format!("{data}\n"))
        .map_err(|error| format!("failed to write {}: {error}", output.display()))
}

fn floorplan_slot(index: u64, position: usize) -> (u64, u64) {
    if index == 0 && position < FLOORPLAN_SEED0_SLOTS.len() {
        return FLOORPLAN_SEED0_SLOTS[position];
    }
    let mut rng = SplitMix64::new(index ^ (position as u64).wrapping_mul(0x517c_c1b7_2722_0a95));
    (rng.next_range(4), rng.next_range(4))
}

struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut value = self.state;
        value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        value ^ (value >> 31)
    }

    fn next_range(&mut self, limit: u64) -> u64 {
        self.next() % limit
    }
}

fn check_xo_reports(path: &Path) -> Result<()> {
    let file =
        File::open(path).map_err(|error| format!("failed to open {}: {error}", path.display()))?;
    let mut archive = ZipArchive::new(file)
        .map_err(|error| format!("failed to read {} as zip: {error}", path.display()))?;

    for report_file in REPORT_FILES {
        archive
            .by_name(report_file)
            .map_err(|_| format!("{} missing {report_file}", path.display()))?;
    }

    let report_json = archive_text(&mut archive, "report.json")?;
    let report_json: JsonValue = serde_json::from_str(&report_json)
        .map_err(|error| format!("invalid report.json: {error}"))?;
    for module in REPORT_MODULES {
        require_synth_source_json(&report_json, module, "report.json")?;
    }

    let report_yaml = archive_text(&mut archive, "report.yaml")?;
    let report_yaml: serde_yaml::Value = serde_yaml::from_str(&report_yaml)
        .map_err(|error| format!("invalid report.yaml: {error}"))?;
    for module in REPORT_MODULES {
        require_synth_source_yaml(&report_yaml, module, "report.yaml")?;
    }

    Ok(())
}

fn archive_text(archive: &mut ZipArchive<File>, name: &str) -> Result<String> {
    let mut file = archive
        .by_name(name)
        .map_err(|_| format!("zip missing {name}"))?;
    let mut contents = String::new();
    std::io::Read::read_to_string(&mut file, &mut contents)
        .map_err(|error| format!("failed to read {name}: {error}"))?;
    Ok(contents)
}

fn require_synth_source_json(report: &JsonValue, module: &str, name: &str) -> Result<()> {
    let source = report
        .get("area")
        .and_then(|value| value.get("breakdown"))
        .and_then(|value| value.get(module))
        .and_then(|value| value.get("area"))
        .and_then(|value| value.get("source"))
        .and_then(JsonValue::as_str);
    if source != Some("synth") {
        return Err(format!("{name}: {module} area source is not synth"));
    }
    Ok(())
}

fn require_synth_source_yaml(report: &serde_yaml::Value, module: &str, name: &str) -> Result<()> {
    let source = report
        .get("area")
        .and_then(|value| value.get("breakdown"))
        .and_then(|value| value.get(module))
        .and_then(|value| value.get("area"))
        .and_then(|value| value.get("source"))
        .and_then(serde_yaml::Value::as_str);
    if source != Some("synth") {
        return Err(format!("{name}: {module} area source is not synth"));
    }
    Ok(())
}

fn read_json(path: &Path) -> Result<JsonValue> {
    let data = fs::read_to_string(path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    serde_json::from_str(&data).map_err(|error| format!("invalid JSON {}: {error}", path.display()))
}

fn require_file(path: &Path) -> Result<()> {
    if !path.is_file() {
        return Err(format!("missing file {}", path.display()));
    }
    Ok(())
}

fn require_dir(path: &Path) -> Result<()> {
    if !path.is_dir() {
        return Err(format!("missing directory {}", path.display()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_abgraph_independent_of_order() {
        let first = serde_json::json!({
            "vs": [{"name": "b"}, {"name": "a"}],
            "es": [
                {
                    "index": 2,
                    "width": 32,
                    "source_vertex": {"name": "b"},
                    "target_vertex": {"name": "a"}
                },
                {
                    "index": 1,
                    "width": 64,
                    "source_vertex": {"name": "a"},
                    "target_vertex": {"name": "b"}
                }
            ]
        });
        let second = serde_json::json!({
            "vs": [{"name": "a"}, {"name": "b"}],
            "es": [
                {
                    "index": 1,
                    "width": 64,
                    "source_vertex": {"name": "a"},
                    "target_vertex": {"name": "b"}
                },
                {
                    "index": 2,
                    "width": 32,
                    "source_vertex": {"name": "b"},
                    "target_vertex": {"name": "a"}
                }
            ]
        });
        assert_eq!(
            normalize_abgraph(&first).unwrap(),
            normalize_abgraph(&second).unwrap()
        );
    }

    #[test]
    fn floorplan_generation_is_deterministic() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("first.json");
        let second = dir.path().join("second.json");
        gen_floorplan(7, "vadd", &first).unwrap();
        gen_floorplan(7, "vadd", &second).unwrap();
        assert_eq!(
            fs::read_to_string(first).unwrap(),
            fs::read_to_string(second).unwrap()
        );
    }
}
