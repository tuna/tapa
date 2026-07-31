//! Conformance and round-trip tests for the `tapa-ir` graph model.

use tapa_ir::port::ArgCategory;
use tapa_ir::task::TaskLevel;
use tapa_ir::Graph;

fn fixture(name: &str) -> String {
    let path = format!(
        "{}/../testdata/task-graph/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"))
}

// ── Positive parse tests ────────────────────────────────────────────

#[test]
fn parse_vadd() {
    let g = Graph::from_json(&fixture("vadd.json")).expect("parse vadd");
    assert_eq!(g.top, "VecAdd", "top task name");
    assert_eq!(g.tasks.len(), 4, "task count");
    assert!(g.tasks.contains_key("VecAdd"), "has VecAdd");
    assert!(g.tasks.contains_key("Add"), "has Add");
}

#[test]
fn vadd_upper_task_structure() {
    let g = Graph::from_json(&fixture("vadd.json")).expect("parse");
    let top = &g.tasks["VecAdd"];
    assert_eq!(top.level, TaskLevel::Upper, "VecAdd is upper");
    assert_eq!(top.ports.len(), 4, "VecAdd port count");
    assert_eq!(top.tasks.len(), 3, "VecAdd child task types");
    assert_eq!(top.fifos.len(), 3, "VecAdd FIFO count");
}

#[test]
fn vadd_fifo_endpoints() {
    let g = Graph::from_json(&fixture("vadd.json")).expect("parse");
    let fifo = &g.tasks["VecAdd"].fifos["a_q"];
    assert_eq!(fifo.depth, Some(2), "a_q depth");
    let consumer = fifo.consumed_by.as_ref().expect("has consumer");
    assert_eq!(consumer.0, "Add", "consumer task");
    assert_eq!(consumer.1, 0, "consumer index");
    let producer = fifo.produced_by.as_ref().expect("has producer");
    assert_eq!(producer.0, "Mmap2Stream", "producer task");
}

#[test]
fn vadd_leaf_task() {
    let g = Graph::from_json(&fixture("vadd.json")).expect("parse");
    let add = &g.tasks["Add"];
    assert_eq!(add.level, TaskLevel::Lower, "Add is lower");
    assert!(add.tasks.is_empty(), "leaf has no children");
    assert!(add.fifos.is_empty(), "leaf has no FIFOs");
    assert_eq!(add.ports.len(), 4, "Add port count");
}

#[test]
fn channelized_mmap_port_deserializes() {
    let g = Graph::from_json(&fixture("mmap_ports.json")).expect("parse");
    let top = &g.tasks["Top"];
    let data_port = top
        .ports
        .iter()
        .find(|p| p.name == "data")
        .expect("data port");
    assert_eq!(data_port.cat, ArgCategory::Mmap, "data is an mmap port");
    assert_eq!(data_port.chan_count, Some(4), "chan_count preserved");
    assert_eq!(data_port.chan_size, Some(1024), "chan_size preserved");
}

#[test]
fn all_category_variants_in_fixture() {
    let g = Graph::from_json(&fixture("mmap_ports.json")).expect("parse");
    let ports = &g.tasks["Top"].ports;
    let cats: Vec<_> = ports.iter().map(|p| p.cat).collect();
    assert!(cats.contains(&ArgCategory::Mmap), "has mmap");
    assert!(cats.contains(&ArgCategory::AsyncMmap), "has async_mmap");
    assert!(cats.contains(&ArgCategory::Istreams), "has istreams");
    assert!(cats.contains(&ArgCategory::Ostreams), "has ostreams");
    assert!(cats.contains(&ArgCategory::Immap), "has immap");
    assert!(cats.contains(&ArgCategory::Ommap), "has ommap");
}

#[test]
fn negative_step_accepted() {
    let json = r#"{
        "cflags": [], "top": "T", "target": "xilinx-hls",
        "tasks": {"T": {"code": "", "level": "upper", "synth": "hls",
            "readable_name": "T",
            "tasks": {"C": [{"args": {}, "step": -1}]}, "fifos": {}, "ports": []}}
    }"#;
    let g = Graph::from_json(json).expect("parse negative step");
    assert_eq!(g.tasks["T"].tasks["C"][0].step, -1, "negative step");
}

/// `readable_name` is the one `tapacc`-emitted field that used to ride in
/// the untyped `extra` bag. It is a typed field now: it must parse and
/// survive a round-trip. `tapacc` emits the demangled template
/// specialization for template tasks and the plain task name otherwise.
#[test]
fn readable_name_is_typed_and_round_trips() {
    let json = r#"{
        "cflags": [], "top": "T", "target": "xilinx-hls",
        "tasks": {
            "T": {"code": "", "level": "upper", "synth": "hls",
                  "readable_name": "Compute<float, 4>",
                  "tasks": {}, "fifos": {}, "ports": []},
            "U": {"code": "", "level": "lower", "synth": "hls",
                  "readable_name": "U", "ports": []}
        }
    }"#;
    let g = Graph::from_json(json).expect("parse readable_name");
    assert_eq!(g.tasks["T"].readable_name, "Compute<float, 4>");
    assert_eq!(g.tasks["U"].readable_name, "U", "non-template name");

    let g2 =
        Graph::from_json(&serde_json::to_string_pretty(&g).expect("serialize")).expect("reparse");
    assert_eq!(g, g2, "readable_name round-trips");
}

/// `tapacc` emits `readable_name` unconditionally for every task, so the
/// field is required rather than defaulted: a payload that omits it does
/// not describe real `tapacc` output and must be rejected outright. A
/// `#[serde(default)]` here would silently paper over a malformed graph.
#[test]
fn readable_name_is_required() {
    let json = r#"{
        "cflags": [], "top": "T", "target": "xilinx-hls",
        "tasks": {
            "T": {"code": "", "level": "lower", "synth": "hls", "ports": []}
        }
    }"#;
    let err = Graph::from_json(json).expect_err("omitted readable_name must fail");
    let msg = err.to_string();
    assert!(
        msg.contains("readable_name"),
        "error must name the missing field; got {msg}",
    );
    assert!(
        msg.contains("tasks.T"),
        "error must carry a path pointer; got {msg}",
    );
}

#[test]
fn consumer_only_fifo() {
    let json = r#"{
        "cflags": [], "top": "T", "target": "xilinx-hls",
        "tasks": {"T": {"code": "", "level": "upper", "synth": "hls",
            "readable_name": "T",
            "tasks": {}, "fifos": {"ext": {"consumed_by": ["X", 0]}}, "ports": []}}
    }"#;
    let g = Graph::from_json(json).expect("parse consumer-only FIFO");
    let f = &g.tasks["T"].fifos["ext"];
    assert!(f.consumed_by.is_some(), "has consumer");
    assert!(f.produced_by.is_none(), "no producer");
    assert!(f.depth.is_none(), "no depth");
}

// ── Round-trip tests ────────────────────────────────────────────────

#[test]
fn vadd_round_trip() {
    let json = fixture("vadd.json");
    let g1 = Graph::from_json(&json).expect("parse 1");
    let serialized = serde_json::to_string_pretty(&g1).expect("serialize");
    let g2 = Graph::from_json(&serialized).expect("parse 2");
    assert_eq!(g1, g2, "round-trip equality");
}

#[test]
fn mmap_ports_round_trip() {
    let json = fixture("mmap_ports.json");
    let g1 = Graph::from_json(&json).expect("parse 1");
    let serialized = serde_json::to_string_pretty(&g1).expect("serialize");
    let g2 = Graph::from_json(&serialized).expect("parse 2");
    assert_eq!(g1, g2, "round-trip equality");
}

// ── Negative tests ──────────────────────────────────────────────────

#[test]
fn unknown_top_level_field_rejected() {
    let json = r#"{"cflags": [], "top": "T", "target": "xilinx-hls", "tasks": {}, "bogus": true}"#;
    let err = Graph::from_json(json).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("bogus") || msg.contains("unknown"),
        "error mentions field: {msg}"
    );
}

#[test]
fn invalid_level_rejected() {
    let json = r#"{
        "cflags": [], "top": "T", "target": "xilinx-hls",
        "tasks": {"T": {"code": "", "level": "invalid", "synth": "hls"}}
    }"#;
    let err = Graph::from_json(json).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("level") || msg.contains("invalid"),
        "error mentions level: {msg}"
    );
}

#[test]
fn invalid_category_rejected_with_path() {
    let json = r#"{
        "cflags": [], "top": "T", "target": "xilinx-hls",
        "tasks": {"T": {"code": "", "level": "lower", "synth": "hls",
            "ports": [{"cat": "nonexistent", "name": "x", "type": "int", "width": 32}]}}
    }"#;
    let err = Graph::from_json(json).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("nonexistent") || msg.contains("cat"),
        "error about cat: {msg}"
    );
}

#[test]
fn empty_input_rejected() {
    let err = Graph::from_json("").unwrap_err();
    assert!(!err.to_string().is_empty(), "error message is not empty");
}
