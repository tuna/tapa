//! Conformance and round-trip tests for the `tapa-ir` design model.

use tapa_ir::design::Design;
use tapa_ir::port::ArgCategory;
use tapa_ir::task::TaskLevel;

fn fixture(name: &str) -> String {
    let path = format!("{}/../testdata/topology/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"))
}

fn to_json(design: &Design) -> String {
    serde_json::to_string_pretty(design).expect("serialize design")
}

// ── Positive parse tests ────────────────────────────────────────────

#[test]
fn parse_vadd_design() {
    let d = Design::from_json(&fixture("vadd_design.json")).expect("parse vadd");
    assert_eq!(d.top, "VecAdd", "top task");
    assert_eq!(d.target.as_str(), "xilinx-hls", "target");
    assert_eq!(d.tasks.len(), 4, "task count");
}

#[test]
fn vadd_upper_task_structure() {
    let d = Design::from_json(&fixture("vadd_design.json")).expect("parse");
    let top = &d.tasks["VecAdd"];
    assert_eq!(top.level, TaskLevel::Upper, "VecAdd is upper");
    assert_eq!(top.ports.len(), 4, "VecAdd port count");
    assert_eq!(top.tasks.len(), 3, "VecAdd child task types");
    assert_eq!(top.fifos.len(), 3, "VecAdd FIFO count");
}

#[test]
fn vadd_leaf_task() {
    let d = Design::from_json(&fixture("vadd_design.json")).expect("parse");
    let add = &d.tasks["Add"];
    assert_eq!(add.level, TaskLevel::Lower, "Add is lower");
    assert!(add.tasks.is_empty(), "leaf has no children");
    assert!(add.fifos.is_empty(), "leaf has no FIFOs");
    assert_eq!(add.ports.len(), 4, "Add port count");
}

#[test]
fn vadd_rtl_annotations_preserved() {
    let d = Design::from_json(&fixture("vadd_design.json")).expect("parse");
    let m2s = &d.tasks["Mmap2Stream"];
    let lut = m2s.self_area.get("LUT").expect("has LUT");
    assert_eq!(lut, 414, "LUT value");
    assert_eq!(m2s.clock_period, "2.342", "clock_period");
}

#[test]
fn vadd_fifo_endpoints() {
    let d = Design::from_json(&fixture("vadd_design.json")).expect("parse");
    let top = &d.tasks["VecAdd"];
    let a_q = &top.fifos["a_q"];
    assert_eq!(a_q.depth, Some(2), "a_q depth");
    let consumer = a_q.consumed_by.as_ref().expect("has consumer");
    assert_eq!(consumer.0, "Add", "consumer task");
    assert_eq!(consumer.1, 0, "consumer index");
}

#[test]
fn vadd_instance_args() {
    let d = Design::from_json(&fixture("vadd_design.json")).expect("parse");
    let top = &d.tasks["VecAdd"];
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
    let top = &d.tasks["TopTask"];
    assert!(!top.is_slot, "TopTask is not a slot");
    let slot = &d.tasks["SlotTask"];
    assert!(slot.is_slot, "SlotTask is a slot");
}

#[test]
fn slots_floorplan_region() {
    let d = Design::from_json(&fixture("slots_design.json")).expect("parse");
    let regions = d.slot_task_name_to_fp_region.as_ref().expect("has regions");
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
    let serialized = to_json(&d1);
    let d2 = Design::from_json(&serialized).expect("parse 2");
    assert_eq!(d1.top, d2.top, "top round-trips");
    assert_eq!(d1.target, d2.target, "target round-trips");
    assert_eq!(d1.tasks.len(), d2.tasks.len(), "task count round-trips");
}

#[test]
fn slots_round_trip() {
    let json = fixture("slots_design.json");
    let d1 = Design::from_json(&json).expect("parse 1");
    let serialized = to_json(&d1);
    let d2 = Design::from_json(&serialized).expect("parse 2");
    assert_eq!(d1.tasks["SlotTask"].is_slot, d2.tasks["SlotTask"].is_slot);
    assert_eq!(
        d1.slot_task_name_to_fp_region,
        d2.slot_task_name_to_fp_region
    );
}

/// Replaces the pre-`tapa-ir` `unknown_fields_preserved` test: the
/// design model is now fully typed with `deny_unknown_fields`, so
/// unknown fields are rejected instead of round-tripped.
#[test]
fn unknown_top_level_field_rejected() {
    let json = r#"{
        "top": "T", "target": "xilinx-hls",
        "tasks": {},
        "extra_top_field": "rejected"
    }"#;
    let err = Design::from_json(json).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("extra_top_field") || msg.contains("unknown"),
        "error about unknown top-level field: {msg}"
    );
}

#[test]
fn annotation_round_trip() {
    let json = fixture("vadd_design.json");
    let d1 = Design::from_json(&json).expect("parse");
    let serialized = to_json(&d1);
    let d2 = Design::from_json(&serialized).expect("parse 2");

    let m2s_1 = &d1.tasks["Mmap2Stream"];
    let m2s_2 = &d2.tasks["Mmap2Stream"];
    assert_eq!(m2s_1.self_area, m2s_2.self_area, "self_area round-trips");
    assert_eq!(m2s_1.total_area, m2s_2.total_area, "total_area round-trips");
    assert_eq!(
        m2s_1.clock_period, m2s_2.clock_period,
        "clock_period round-trips"
    );
}

// ── Negative tests ──────────────────────────────────────────────────

#[test]
fn missing_top_field() {
    let json = r#"{"target": "xilinx-hls", "tasks": {}}"#;
    let err = Design::from_json(json).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("top") || msg.contains("missing"),
        "error about top: {msg}"
    );
}

#[test]
fn invalid_level() {
    let json = r#"{
        "top": "T", "target": "xilinx-hls",
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
        "top": "T", "target": "xilinx-hls",
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
        "top": "T", "target": "xilinx-hls",
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
        "top": "T", "target": "xilinx-hls",
        "tasks": {"T": {"name": "T", "level": "lower", "code": "", "target": "hls",
            "is_slot": false, "clock_period": "0",
            "ports": [{"cat": "hmap", "name": "data", "type": "float*", "width": 32}]}}
    }"#;
    let d = Design::from_json(json).expect("parse hmap port");
    assert_eq!(d.tasks["T"].ports[0].cat, ArgCategory::Mmap, "hmap -> Mmap");
    let serialized = to_json(&d);
    assert!(serialized.contains(r#""mmap""#), "round-trips as mmap");
    assert!(!serialized.contains(r#""hmap""#), "no hmap in output");
}
