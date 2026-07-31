//! Conformance and round-trip tests for the `tapa-ir` design model.

use std::io;

use serde::Serialize;
use tapa_ir::port::ArgCategory;
use tapa_ir::task::TaskLevel;
use tapa_ir::Design;

fn fixture(name: &str) -> String {
    let path = format!("{}/../testdata/topology/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"))
}

fn to_json(design: &Design) -> String {
    serde_json::to_string_pretty(design).expect("serialize design")
}

/// Serialize compactly with `, ` / `: ` separators.
///
/// This is a *test device*, not the on-disk format: the CLI pretty-prints
/// the state file it writes. Rendering the model on a single line is simply
/// what lets [`canonical_design_json`] pin field order and
/// `skip_serializing_if` behaviour as one readable literal, which is the
/// invariant these tests exist to catch drift in.
fn to_compact_json(design: &Design) -> String {
    struct SpacedFormatter;
    impl serde_json::ser::Formatter for SpacedFormatter {
        fn begin_array_value<W: io::Write + ?Sized>(
            &mut self,
            writer: &mut W,
            first: bool,
        ) -> io::Result<()> {
            if first {
                Ok(())
            } else {
                writer.write_all(b", ")
            }
        }

        fn begin_object_key<W: io::Write + ?Sized>(
            &mut self,
            writer: &mut W,
            first: bool,
        ) -> io::Result<()> {
            if first {
                Ok(())
            } else {
                writer.write_all(b", ")
            }
        }

        fn begin_object_value<W: io::Write + ?Sized>(&mut self, writer: &mut W) -> io::Result<()> {
            writer.write_all(b": ")
        }
    }

    let mut buf = Vec::new();
    let mut serializer = serde_json::Serializer::with_formatter(&mut buf, SpacedFormatter);
    design
        .serialize(&mut serializer)
        .expect("serialize design with the state formatter");
    String::from_utf8(buf).expect("utf-8 JSON")
}

/// A canonical task-graph payload, written in **field declaration order**
/// and in the compact rendering [`to_compact_json`] emits.
///
/// This is the literal that pins the wire shape documented on
/// `tapa_ir::graph`: struct field order is stable, `tasks` / `fifos` are
/// `BTreeMap`s so keys emit alphabetically, and the post-synthesis
/// annotations (`clock_period`, `self_area`, `total_area`) are omitted
/// until populated. It deliberately exercises both a lower task (with
/// annotations present) and an upper task (with them absent), ports,
/// child instances with args, and FIFO endpoints — so reordering,
/// renaming, or dropping any field breaks the byte comparison.
fn canonical_design_json() -> &'static str {
    concat!(
        r#"{"top": "VecAdd", "target": "xilinx-vitis", "cflags": ["-std=c++17"], "tasks": "#,
        r#"{"Add": {"level": "lower", "code": "void Add() {}", "readable_name": "Add", "#,
        r#""synth": "hls", "ports": [{"cat": "istream", "name": "a", "type": "float", "#,
        r#""width": 32}], "tasks": {}, "fifos": {}, "clock_period": "2.342", "#,
        r#""self_area": {"LUT": 414}, "total_area": {"LUT": 414}}, "#,
        r#""VecAdd": {"level": "upper", "code": "void VecAdd() {}", "#,
        r#""readable_name": "VecAdd", "synth": "hls", "ports": [], "#,
        r#""tasks": {"Add": [{"args": {"a": {"arg": "a_q", "cat": "istream"}}, "step": 0}]}, "#,
        r#""fifos": {"a_q": {"depth": 2, "consumed_by": ["Add", 0], "#,
        r#""produced_by": ["Mmap2Stream", 0]}}}}}"#,
    )
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

/// Byte-exact re-emission: parsing the canonical payload and serializing
/// it back must reproduce the input byte-for-byte.
///
/// This pins the wire-shape invariant documented on `tapa_ir::graph`.
/// A field reordered, renamed, or dropped on `TaskGraph` / `Task` /
/// `Port` / `TaskInstance` / `InterconnectDefinition` — or a
/// `skip_serializing_if` gained or lost — changes the emitted bytes and
/// fails here, which a field-count or top-level-only assertion cannot
/// catch.
#[test]
fn round_trip_byte_equal() {
    let json = canonical_design_json();
    let design = Design::from_json(json).expect("parse canonical task graph");
    assert_eq!(
        to_compact_json(&design),
        json,
        "round-trip must preserve the byte sequence",
    );
}

/// `tasks` is a `BTreeMap`, so keys come out alphabetically — the sorted
/// order `tapa analyze` writes — regardless of the order they arrived in.
/// The payload here lists `VecAdd` before `Add` on purpose: an insertion-
/// ordered map (`IndexMap`) would echo the input order and fail.
#[test]
fn task_order_preserved() {
    let json = concat!(
        r#"{"top": "VecAdd", "target": "xilinx-hls", "cflags": [], "tasks": {"#,
        r#""VecAdd": {"level": "upper", "code": "", "readable_name": "VecAdd", "#,
        r#""synth": "hls", "ports": [], "tasks": {}, "fifos": {}}, "#,
        r#""Add": {"level": "lower", "code": "", "readable_name": "Add", "#,
        r#""synth": "hls", "ports": [], "tasks": {}, "fifos": {}}}}"#,
    );
    let design = Design::from_json(json).expect("parse");
    let names: Vec<&str> = design.tasks.keys().map(String::as_str).collect();
    assert_eq!(
        names,
        vec!["Add", "VecAdd"],
        "task keys must sort alphabetically, not follow input order",
    );

    // The emitted form is sorted too, not just the in-memory map.
    let reparsed = Design::from_json(&to_compact_json(&design)).expect("reparse");
    let emitted: Vec<&str> = reparsed.tasks.keys().map(String::as_str).collect();
    assert_eq!(emitted, vec!["Add", "VecAdd"], "sorted order survives emit");
}

/// `from_reader` is a public parse entry point, so it gets direct coverage.
#[test]
fn from_reader_works() {
    let json = canonical_design_json();
    let design = Design::from_reader(json.as_bytes()).expect("from_reader");
    assert_eq!(design.top, "VecAdd", "top");
    assert_eq!(design.target.as_str(), "xilinx-vitis", "target");
    assert_eq!(design.tasks.len(), 2, "task count");
}

/// `Task` carries `deny_unknown_fields`: an unknown *per-task* field is a
/// malformed graph, not something to round-trip through an `extra` bag.
/// The error must also carry the `tasks.<name>` path pointer that makes a
/// deep schema error actionable.
#[test]
fn unknown_task_field_rejected() {
    let json = concat!(
        r#"{"top": "T", "target": "xilinx-hls", "cflags": [], "tasks": {"#,
        r#""T": {"level": "lower", "code": "", "readable_name": "T", "synth": "hls", "#,
        r#""ports": [], "tasks": {}, "fifos": {}, "bogus_field": 1}}}"#,
    );
    let err = Design::from_json(json).expect_err("unknown task field must fail");
    let msg = err.to_string();
    assert!(
        msg.contains("bogus_field") || msg.contains("unknown field"),
        "error must mention the offending field; got {msg}",
    );
    assert!(
        msg.contains("tasks.T"),
        "error must include a path pointer; got {msg}",
    );
}

/// The design model is fully typed with `deny_unknown_fields`, so unknown
/// fields are rejected instead of round-tripped.
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
fn missing_root_target_field() {
    let json = r#"{"top": "T", "tasks": {}}"#;
    let err = Design::from_json(json).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("target") || msg.contains("missing"),
        "error about target: {msg}"
    );
}

#[test]
fn invalid_level() {
    let json = r#"{
        "top": "T", "target": "xilinx-hls",
        "tasks": {"T": {"level": "invalid", "code": "", "synth": "hls"}}
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
        "tasks": {"T": {"level": "lower", "code": "", "synth": "hls",
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
        "tasks": {"T": {"level": "upper", "code": "", "synth": "hls",
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
fn unknown_task_synth_policy_rejected() {
    // The closed `SynthTarget` enum rejects the old flow-derived task
    // targets: only `"hls"` / `"ignore"` are valid per-task values now.
    let json = r#"{
        "top": "T", "target": "xilinx-hls",
        "tasks": {"T": {"level": "lower", "code": "", "synth": "xilinx_vitis"}}
    }"#;
    let err = Design::from_json(json).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("xilinx_vitis") || msg.contains("synth") || msg.contains("unknown"),
        "error about invalid synth policy: {msg}"
    );
}

#[test]
fn mmap_port_category_round_trips() {
    let json = r#"{
        "top": "T", "target": "xilinx-hls",
        "tasks": {"T": {"level": "lower", "code": "", "synth": "hls",
            "readable_name": "T",
            "clock_period": "0",
            "ports": [{"cat": "mmap", "name": "data", "type": "float*", "width": 32}]}}
    }"#;
    let d = Design::from_json(json).expect("parse mmap port");
    assert_eq!(d.tasks["T"].ports[0].cat, ArgCategory::Mmap);
    let serialized = to_json(&d);
    assert!(serialized.contains(r#""mmap""#), "round-trips as mmap");
}
