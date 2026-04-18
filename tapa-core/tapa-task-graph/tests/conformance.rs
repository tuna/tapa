//! Conformance and round-trip tests for `tapa-task-graph`.

use tapa_task_graph::graph::Graph;
use tapa_task_graph::port::ArgCategory;
use tapa_task_graph::task::TaskLevel;

fn fixture(name: &str) -> String {
    let path = format!("{}/../testdata/task-graph/{name}", env!("CARGO_MANIFEST_DIR"));
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
fn hmap_port_deserializes_to_mmap() {
    let g = Graph::from_json(&fixture("hmap_ports.json")).expect("parse hmap");
    let top = &g.tasks["Top"];
    let data_port = top.ports.iter().find(|p| p.name == "data").expect("data port");
    assert_eq!(data_port.cat, ArgCategory::Mmap, "hmap -> Mmap");
    assert_eq!(data_port.chan_count, Some(4), "chan_count preserved");
    assert_eq!(data_port.chan_size, Some(1024), "chan_size preserved");
}

#[test]
fn all_category_variants_in_fixture() {
    let g = Graph::from_json(&fixture("hmap_ports.json")).expect("parse");
    let ports = &g.tasks["Top"].ports;
    let cats: Vec<_> = ports.iter().map(|p| p.cat).collect();
    assert!(cats.contains(&ArgCategory::Mmap), "has mmap (from hmap)");
    assert!(cats.contains(&ArgCategory::AsyncMmap), "has async_mmap");
    assert!(cats.contains(&ArgCategory::Istreams), "has istreams");
    assert!(cats.contains(&ArgCategory::Ostreams), "has ostreams");
    assert!(cats.contains(&ArgCategory::Immap), "has immap");
    assert!(cats.contains(&ArgCategory::Ommap), "has ommap");
}

#[test]
fn negative_step_accepted() {
    let json = r#"{
        "cflags": [], "top": "T",
        "tasks": {"T": {"code": "", "level": "upper", "target": "hls", "vendor": "",
            "tasks": {"C": [{"args": {}, "step": -1}]}, "fifos": {}, "ports": []}}
    }"#;
    let g = Graph::from_json(json).expect("parse negative step");
    assert_eq!(g.tasks["T"].tasks["C"][0].step, -1, "negative step");
}

#[test]
fn consumer_only_fifo() {
    let json = r#"{
        "cflags": [], "top": "T",
        "tasks": {"T": {"code": "", "level": "upper", "target": "hls", "vendor": "",
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
    let serialized = g1.to_json().expect("serialize");
    let g2 = Graph::from_json(&serialized).expect("parse 2");
    assert_eq!(g1, g2, "round-trip equality");
}

#[test]
fn hmap_round_trip_canonical() {
    let json = fixture("hmap_ports.json");
    let g = Graph::from_json(&json).expect("parse");
    let serialized = g.to_json().expect("serialize");
    // After round-trip, "hmap" should appear as "mmap"
    assert!(!serialized.contains(r#""hmap""#), "hmap should not appear in output");
    assert!(serialized.contains(r#""mmap""#), "mmap should appear");
}

// ── Negative tests ──────────────────────────────────────────────────

#[test]
fn unknown_top_level_field_rejected() {
    let json = r#"{"cflags": [], "top": "T", "tasks": {}, "bogus": true}"#;
    let err = Graph::from_json(json).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("bogus") || msg.contains("unknown"), "error mentions field: {msg}");
}

#[test]
fn invalid_level_rejected() {
    let json = r#"{
        "cflags": [], "top": "T",
        "tasks": {"T": {"code": "", "level": "invalid", "target": "hls", "vendor": ""}}
    }"#;
    let err = Graph::from_json(json).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("level") || msg.contains("invalid"), "error mentions level: {msg}");
}

#[test]
fn invalid_category_rejected_with_path() {
    let json = r#"{
        "cflags": [], "top": "T",
        "tasks": {"T": {"code": "", "level": "lower", "target": "hls", "vendor": "",
            "ports": [{"cat": "nonexistent", "name": "x", "type": "int", "width": 32}]}}
    }"#;
    let err = Graph::from_json(json).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("nonexistent") || msg.contains("cat"), "error about cat: {msg}");
}

#[test]
fn empty_input_rejected() {
    let err = Graph::from_json("").unwrap_err();
    assert!(!err.to_string().is_empty(), "error message is not empty");
}
