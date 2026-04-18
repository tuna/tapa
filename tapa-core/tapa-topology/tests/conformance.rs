//! Conformance and round-trip tests for `tapa-topology`.

use tapa_task_graph::port::ArgCategory;
use tapa_task_graph::task::TaskLevel;
use tapa_topology::design::Design;

fn fixture(name: &str) -> String {
    let path = format!("{}/../testdata/topology/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"))
}

// ── Positive parse tests ────────────────────────────────────────────

#[test]
fn parse_vadd_design() {
    let d = Design::from_json(&fixture("vadd_design.json")).expect("parse vadd");
    assert_eq!(d.program.top, "VecAdd", "top task");
    assert_eq!(d.program.target, "xilinx-hls", "target");
    assert_eq!(d.program.tasks.len(), 4, "task count");
}

#[test]
fn vadd_upper_task_structure() {
    let d = Design::from_json(&fixture("vadd_design.json")).expect("parse");
    let top = &d.program.tasks["VecAdd"];
    assert_eq!(top.level, TaskLevel::Upper, "VecAdd is upper");
    assert_eq!(top.ports.len(), 4, "VecAdd port count");
    assert_eq!(top.tasks.len(), 3, "VecAdd child task types");
    assert_eq!(top.fifos.len(), 3, "VecAdd FIFO count");
}

#[test]
fn vadd_leaf_task() {
    let d = Design::from_json(&fixture("vadd_design.json")).expect("parse");
    let add = &d.program.tasks["Add"];
    assert_eq!(add.level, TaskLevel::Lower, "Add is lower");
    assert!(add.tasks.is_empty(), "leaf has no children");
    assert!(add.fifos.is_empty(), "leaf has no FIFOs");
    assert_eq!(add.ports.len(), 4, "Add port count");
}

#[test]
fn vadd_rtl_annotations_preserved() {
    let d = Design::from_json(&fixture("vadd_design.json")).expect("parse");
    let m2s = &d.program.tasks["Mmap2Stream"];
    // self_area and total_area should be in annotations
    let self_area = m2s.annotations.get("self_area").expect("has self_area");
    assert!(self_area.is_object(), "self_area is object");
    let lut = self_area.get("LUT").expect("has LUT");
    assert_eq!(lut, 414, "LUT value");

    let clock = m2s.annotations.get("clock_period").expect("has clock_period");
    assert_eq!(clock.as_str().unwrap(), "2.342", "clock_period");
}

#[test]
fn vadd_fifo_endpoints() {
    let d = Design::from_json(&fixture("vadd_design.json")).expect("parse");
    let top = &d.program.tasks["VecAdd"];
    let a_q = &top.fifos["a_q"];
    assert_eq!(a_q.depth, Some(2), "a_q depth");
    let consumer = a_q.consumed_by.as_ref().expect("has consumer");
    assert_eq!(consumer.0, "Add", "consumer task");
    assert_eq!(consumer.1, 0, "consumer index");
}

#[test]
fn vadd_instance_args() {
    let d = Design::from_json(&fixture("vadd_design.json")).expect("parse");
    let top = &d.program.tasks["VecAdd"];
    let add_instances = &top.tasks["Add"];
    assert_eq!(add_instances.len(), 1, "one Add instance");
    let instance = &add_instances[0];
    assert_eq!(instance.args.len(), 4, "4 args");
    let a_arg = &instance.args["a"];
    assert_eq!(a_arg.arg, "a_q", "a connects to a_q FIFO");
    assert_eq!(a_arg.cat, ArgCategory::Istream, "a is istream");
}

// ── Slot tests ──────────────────────────────────────────────────────

#[test]
fn slots_design_is_slot_flag() {
    let d = Design::from_json(&fixture("slots_design.json")).expect("parse slots");
    let top = &d.program.tasks["TopTask"];
    assert!(!top.is_slot, "TopTask is not a slot");
    let slot = &d.program.tasks["SlotTask"];
    assert!(slot.is_slot, "SlotTask is a slot");
}

#[test]
fn slots_floorplan_region() {
    let d = Design::from_json(&fixture("slots_design.json")).expect("parse");
    let regions = d.program.slot_task_name_to_fp_region.as_ref().expect("has regions");
    assert_eq!(regions["SlotTask"], "SLOT_X0Y0:SLOT_X0Y0");
}

#[test]
fn floorplan_slots_derived() {
    let d = Design::from_json(&fixture("slots_design.json")).expect("parse");
    let slots = d.floorplan_slots();
    assert_eq!(slots.len(), 1, "one slot");
    assert!(slots.contains(&"SlotTask".to_owned()));
}

// ── Round-trip tests ────────────────────────────────────────────────

#[test]
fn vadd_round_trip() {
    let json = fixture("vadd_design.json");
    let d1 = Design::from_json(&json).expect("parse 1");
    let serialized = d1.to_json().expect("serialize");
    let d2 = Design::from_json(&serialized).expect("parse 2");
    assert_eq!(d1.program.top, d2.program.top, "top round-trips");
    assert_eq!(d1.program.target, d2.program.target, "target round-trips");
    assert_eq!(d1.program.tasks.len(), d2.program.tasks.len(), "task count round-trips");
}

#[test]
fn slots_round_trip() {
    let json = fixture("slots_design.json");
    let d1 = Design::from_json(&json).expect("parse 1");
    let serialized = d1.to_json().expect("serialize");
    let d2 = Design::from_json(&serialized).expect("parse 2");
    assert_eq!(d1.program.tasks["SlotTask"].is_slot, d2.program.tasks["SlotTask"].is_slot);
    assert_eq!(d1.program.slot_task_name_to_fp_region, d2.program.slot_task_name_to_fp_region);
}

#[test]
fn unknown_fields_preserved() {
    let json = r#"{
        "top": "T", "target": "hls",
        "tasks": {
            "T": {
                "level": "lower", "code": "", "target": "hls",
                "custom_field": 42
            }
        },
        "extra_top_field": "preserved"
    }"#;
    let d = Design::from_json(json).expect("parse with extras");
    let serialized = d.to_json().expect("serialize");
    assert!(serialized.contains("extra_top_field"), "top-level extra preserved");
    assert!(serialized.contains("custom_field"), "task-level extra preserved");
}

#[test]
fn annotation_round_trip() {
    let json = fixture("vadd_design.json");
    let d1 = Design::from_json(&json).expect("parse");
    let serialized = d1.to_json().expect("serialize");
    let d2 = Design::from_json(&serialized).expect("parse 2");

    let m2s_1 = &d1.program.tasks["Mmap2Stream"];
    let m2s_2 = &d2.program.tasks["Mmap2Stream"];
    assert_eq!(m2s_1.annotations, m2s_2.annotations, "annotations round-trip");
}

// ── Negative tests ──────────────────────────────────────────────────

#[test]
fn missing_top_field() {
    let json = r#"{"target": "hls", "tasks": {}}"#;
    let err = Design::from_json(json).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("top") || msg.contains("missing"), "error about top: {msg}");
}

#[test]
fn invalid_level() {
    let json = r#"{
        "top": "T", "target": "hls",
        "tasks": {"T": {"level": "invalid", "code": "", "target": "hls"}}
    }"#;
    let err = Design::from_json(json).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("level") || msg.contains("invalid"),
        "error about level: {msg}"
    );
}

#[test]
fn empty_input() {
    let err = Design::from_json("").unwrap_err();
    assert!(!err.to_string().is_empty(), "error is not empty");
}

#[test]
fn invalid_port_category_rejected() {
    let json = r#"{
        "top": "T", "target": "hls",
        "tasks": {"T": {"level": "lower", "code": "", "target": "hls",
            "ports": [{"cat": "not_a_real_cat", "name": "x", "type": "int", "width": 32}]}}
    }"#;
    let err = Design::from_json(json).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("not_a_real_cat") || msg.contains("cat") || msg.contains("unknown"),
        "error about invalid category: {msg}"
    );
}

#[test]
fn invalid_instance_arg_category_rejected() {
    let json = r#"{
        "top": "T", "target": "hls",
        "tasks": {"T": {"level": "upper", "code": "", "target": "hls",
            "tasks": {"C": [{"args": {"p": {"arg": "x", "cat": "bogus"}}, "step": 0}]},
            "fifos": {}}}
    }"#;
    let err = Design::from_json(json).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("bogus") || msg.contains("cat") || msg.contains("unknown"),
        "error about invalid arg category: {msg}"
    );
}

#[test]
fn hmap_port_category_round_trips_as_mmap() {
    let json = r#"{
        "top": "T", "target": "hls",
        "tasks": {"T": {"level": "lower", "code": "", "target": "hls",
            "ports": [{"cat": "hmap", "name": "data", "type": "float*", "width": 32}]}}
    }"#;
    let d = Design::from_json(json).expect("parse hmap port");
    assert_eq!(d.program.tasks["T"].ports[0].cat, ArgCategory::Mmap, "hmap -> Mmap");
    let serialized = d.to_json().expect("serialize");
    assert!(serialized.contains(r#""mmap""#), "round-trips as mmap");
    assert!(!serialized.contains(r#""hmap""#), "no hmap in output");
}
