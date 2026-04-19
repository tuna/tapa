//! Cross-language `design.json` round-trip parity.
//!
//! Direction 1 (Python -> Rust): Python writes `design.json` via
//! `tapa.task.Task.to_topology_dict` + `json.dump`, mirroring
//! `tapa.steps.common.store_design` byte-for-byte. Rust parses with
//! `Design::from_reader`, re-emits with `Design::to_writer`, and asserts
//! the output is byte-equal to the file Python wrote.
//!
//! Direction 2 (Rust -> Python): Rust writes `design.json` via
//! `Design::to_writer`. Python parses it with `json.load` (the load path
//! used by `tapa.steps.common.load_design`) and reconstructs a
//! `tapa.core.Program` with the same top + task name set.
//!
//! Both directions skip cleanly when Python or the `tapa` package is
//! not importable from the test environment, so this suite is safe to
//! run on developer machines without the Python toolchain installed.

use std::process::Command;

use indexmap::IndexMap;
use tapa_task_graph::{Design, TaskTopology};

const REPO_ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn repo_root() -> std::path::PathBuf {
    // CARGO_MANIFEST_DIR is `tapa-core/tapa-task-graph`; walk up two.
    std::path::Path::new(REPO_ROOT)
        .parent()
        .and_then(std::path::Path::parent)
        .map(std::path::Path::to_path_buf)
        .expect("repo root")
}

fn python_can_import_tapa() -> bool {
    let status = Command::new("python3")
        .arg("-c")
        .arg("import sys; sys.path.insert(0, sys.argv[1]); import tapa.task")
        .arg(repo_root())
        .status();
    matches!(status, Ok(s) if s.success())
}

#[test]
fn python_to_topology_dict_round_trips_through_rust() {
    if !python_can_import_tapa() {
        eprintln!("skipping: python3 cannot import `tapa.task` from {REPO_ROOT}");
        return;
    }

    let dir = tempfile::tempdir().expect("tempdir");
    let dest = dir.path().join("design.json");

    let script = r#"
import json, sys, pathlib
sys.path.insert(0, sys.argv[1])
from tapa.task import Task
from tapa.common.target import Target

# Construct two minimal Task instances exercising both task levels and
# the same construction path that `store_design` uses (via
# `to_topology_dict`).
leaf = Task(
    name="Add",
    code="void Add() {}",
    level="lower",
    tasks={},
    fifos={},
    ports=[
        {"cat": "istream", "name": "a", "type": "float", "width": 32},
        {"cat": "ostream", "name": "c", "type": "float", "width": 32},
    ],
    target_type="hls",
    is_slot=False,
)
upper = Task(
    name="VecAdd",
    code="void VecAdd() {}",
    level="upper",
    tasks={
        "Add": [
            {"step": 0, "args": {"a": {"arg": "a_q", "cat": "istream"}}},
        ],
    },
    fifos={
        "a_q": {
            "depth": 2,
            "produced_by": ["Mmap2Stream", 0],
            "consumed_by": ["Add", 0],
        }
    },
    ports=[],
    target_type="hls",
    is_slot=False,
)

design = {
    "top": "VecAdd",
    "target": Target.XILINX_VITIS.value,
    # Insertion order matches Python's topological-sorted `_tasks`.
    "tasks": {"Add": leaf.to_topology_dict(), "VecAdd": upper.to_topology_dict()},
    "slot_task_name_to_fp_region": None,
}

dest = pathlib.Path(sys.argv[2])
dest.parent.mkdir(parents=True, exist_ok=True)
with open(dest, "w", encoding="utf-8") as f:
    json.dump(design, f)
"#;

    let status = Command::new("python3")
        .arg("-c")
        .arg(script)
        .arg(repo_root())
        .arg(&dest)
        .status()
        .expect("python3 available");
    assert!(status.success(), "python writer failed");

    let original = std::fs::read(&dest).expect("read design.json");
    let design = Design::from_reader(&original[..]).expect("rust parses python output");

    assert_eq!(design.top, "VecAdd");
    assert_eq!(design.target, "xilinx-vitis");
    let task_names: Vec<&str> = design.tasks.keys().map(String::as_str).collect();
    assert_eq!(
        task_names,
        vec!["Add", "VecAdd"],
        "Rust must preserve the topological insertion order Python wrote",
    );

    let mut re_emitted = Vec::new();
    design.to_writer(&mut re_emitted).expect("rust re-emits");
    assert_eq!(
        re_emitted, original,
        "Rust → Rust round-trip must be byte-equal to what Python wrote",
    );
}

#[test]
fn rust_design_round_trips_through_python_load_design() {
    if !python_can_import_tapa() {
        eprintln!("skipping: python3 cannot import `tapa.task` from {REPO_ROOT}");
        return;
    }

    let dir = tempfile::tempdir().expect("tempdir");
    let dest = dir.path().join("design.json");

    // Construct a typed Design in Rust and write it to disk.
    let mut tasks: IndexMap<String, TaskTopology> = IndexMap::new();
    tasks.insert(
        "Top".to_string(),
        TaskTopology {
            name: "Top".to_string(),
            level: "lower".to_string(),
            code: "void Top() {}".to_string(),
            ports: Vec::new(),
            tasks: IndexMap::new(),
            fifos: IndexMap::new(),
            target: Some("hls".to_string()),
            is_slot: false,
            self_area: IndexMap::new(),
            total_area: IndexMap::new(),
            clock_period: "0".to_string(),
        },
    );
    let design = Design {
        top: "Top".to_string(),
        target: "xilinx-hls".to_string(),
        tasks,
        slot_task_name_to_fp_region: None,
    };
    std::fs::write(&dest, design.to_json().expect("rust serialize"))
        .expect("write design.json");

    // Have Python's `tapa.steps.common.load_design`-equivalent path
    // (i.e. plain `json.load`) read the file and assert the program
    // schema is intact.
    let script = r#"
import json, sys, pathlib
sys.path.insert(0, sys.argv[1])
# `load_design` is wrapped in a click context dance that we cannot
# reproduce inside `python -c`. The implementation collapses to
# `json.load(open(...))`, so we exercise the exact same call shape
# here. Cross-reference: `tapa/steps/common.py::load_design`.
data = json.load(open(sys.argv[2], encoding="utf-8"))
assert data["top"] == "Top", data
assert data["target"] == "xilinx-hls", data
assert list(data["tasks"]) == ["Top"], data
assert data["tasks"]["Top"]["level"] == "lower"
assert data["tasks"]["Top"]["target"] == "hls"
assert data["slot_task_name_to_fp_region"] is None

# Reconstruct a Program from the loaded data — mirrors the path
# `load_tapa_program` follows when the bridge reads back a Rust-written
# design.json.
from tapa.core import Program
prog = Program(
    {"tasks": data["tasks"], "top": data["top"]},
    target=data["target"],
    work_dir=str(pathlib.Path(sys.argv[2]).parent),
    floorplan_slots=[],
)
assert prog.top == "Top"
assert "Top" in prog._tasks  # noqa: SLF001
print("ok")
"#;

    let output = Command::new("python3")
        .arg("-c")
        .arg(script)
        .arg(repo_root())
        .arg(&dest)
        .output()
        .expect("python3 available");
    assert!(
        output.status.success(),
        "python load_design path failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}
