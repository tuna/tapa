//! Conformance and round-trip tests for the `tapa-ir` task-graph model.
//!
//! `Graph` and `Design` are aliases of the same `TaskGraph`, so one suite
//! covers both of its roles: the `tapacc`-output fixtures under
//! `testdata/task-graph/` and the post-synthesis design fixture under
//! `testdata/topology/`.

use std::io;

use serde::Serialize;
use tapa_ir::port::ArgCategory;
use tapa_ir::task::TaskLevel;
use tapa_ir::TaskGraph;

fn fixture(dir: &str, name: &str) -> String {
    let path = format!("{}/../testdata/{dir}/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"))
}

/// The `VecAdd` design frozen as both of `TaskGraph`'s roles: the
/// `tapacc`-output task graph and the post-synthesis design. Assertions on
/// the shared topology must hold for both spellings.
fn vadd_fixtures() -> [(&'static str, String); 2] {
    [
        ("task-graph/vadd.json", fixture("task-graph", "vadd.json")),
        (
            "topology/vadd_design.json",
            fixture("topology", "vadd_design.json"),
        ),
    ]
}

fn to_json(graph: &TaskGraph) -> String {
    serde_json::to_string_pretty(graph).expect("serialize design")
}

/// Serialize compactly with `, ` / `: ` separators.
///
/// This is a *test device*, not the on-disk format: the CLI pretty-prints
/// the state file it writes. Rendering the model on a single line is simply
/// what lets [`canonical_design_json`] pin field order and
/// `skip_serializing_if` behaviour as one readable literal, which is the
/// invariant these tests exist to catch drift in.
fn to_compact_json(graph: &TaskGraph) -> String {
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
    graph
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
        r#"{"schema_version": 3, "top": "VecAdd", "target": "xilinx-vitis", "cflags": ["-std=c++17"], "tasks": "#,
        r#"{"Add": {"level": "lower", "srcs": ["Add.cpp"], "include_dirs": [], "defines": [], "readable_name": "Add", "#,
        r#""synth": "hls", "ports": [{"cat": "istream", "name": "a", "type": "float", "#,
        r#""width": 32}], "tasks": {}, "fifos": {}, "clock_period": 2342, "#,
        r#""self_area": {"lut": 414, "ff": 0, "bram_18k": 0, "dsp": 0, "uram": 0}, "#,
        r#""total_area": {"lut": 414, "ff": 0, "bram_18k": 0, "dsp": 0, "uram": 0}}, "#,
        r#""VecAdd": {"level": "upper", "srcs": ["VecAdd.cpp"], "include_dirs": [], "defines": [], "#,
        r#""readable_name": "VecAdd", "synth": "hls", "ports": [], "#,
        r#""tasks": {"Add": [{"args": {"a": {"arg": "a_q", "cat": "istream"}}, "step": 0}]}, "#,
        r#""fifos": {"a_q": {"depth": 2, "consumed_by": ["Add", 0], "#,
        r#""produced_by": ["Mmap2Stream", 0]}}}}}"#,
    )
}

#[test]
fn parse_vadd() {
    for (path, json) in vadd_fixtures() {
        let graph = TaskGraph::from_json(&json).unwrap_or_else(|e| panic!("parse {path}: {e}"));
        assert_eq!(graph.top, "VecAdd", "top task name");
        assert_eq!(graph.target.as_str(), "xilinx-hls", "target");
        assert_eq!(graph.tasks.len(), 4, "task count");
        assert!(graph.tasks.contains_key("VecAdd"), "has VecAdd");
        assert!(graph.tasks.contains_key("Add"), "has Add");
    }
}

#[test]
fn vadd_upper_task_structure() {
    for (path, json) in vadd_fixtures() {
        let graph = TaskGraph::from_json(&json).unwrap_or_else(|e| panic!("parse {path}: {e}"));
        let top = &graph.tasks["VecAdd"];
        assert_eq!(top.level, TaskLevel::Upper, "VecAdd is upper");
        assert_eq!(top.ports.len(), 4, "VecAdd port count");
        assert_eq!(top.tasks.len(), 3, "VecAdd child task types");
        assert_eq!(top.fifos.len(), 3, "VecAdd FIFO count");
    }
}

#[test]
fn vadd_leaf_task() {
    for (path, json) in vadd_fixtures() {
        let graph = TaskGraph::from_json(&json).unwrap_or_else(|e| panic!("parse {path}: {e}"));
        let add = &graph.tasks["Add"];
        assert_eq!(add.level, TaskLevel::Lower, "Add is lower");
        assert!(add.tasks.is_empty(), "leaf has no children");
        assert!(add.fifos.is_empty(), "leaf has no FIFOs");
        assert_eq!(add.ports.len(), 4, "Add port count");
    }
}

#[test]
fn vadd_fifo_endpoints() {
    for (path, json) in vadd_fixtures() {
        let graph = TaskGraph::from_json(&json).unwrap_or_else(|e| panic!("parse {path}: {e}"));
        let fifo = &graph.tasks["VecAdd"].fifos["a_q"];
        assert_eq!(fifo.depth, Some(2), "a_q depth");
        let consumer = fifo.consumed_by.as_ref().expect("has consumer");
        assert_eq!(consumer.0, "Add", "consumer task");
        assert_eq!(consumer.1, 0, "consumer index");
        let producer = fifo.produced_by.as_ref().expect("has producer");
        assert_eq!(producer.0, "Mmap2Stream", "producer task");
    }
}

#[test]
fn vadd_rtl_annotations_preserved() {
    let d = TaskGraph::from_json(&fixture("topology", "vadd_design.json")).expect("parse");
    let m2s = &d.tasks["Mmap2Stream"];
    let area = m2s.self_area.expect("has self area");
    assert_eq!(area.lut, 414, "LUT value");
    assert_eq!(
        m2s.clock_period,
        Some(tapa_ir::ClockPeriod::from_picoseconds(2342)),
        "clock_period"
    );
}

/// An all-default area object (`"self_area": {}`) is a stale spelling of
/// "unannotated", not a measured zero: it must parse as `None` so reports
/// and floorplanning derive from the children instead of trusting a zero.
#[test]
fn empty_area_annotation_reads_as_unannotated() {
    let d = TaskGraph::from_json(&fixture("topology", "vadd_design.json")).expect("parse");
    let stream2mmap = &d.tasks["Stream2Mmap"];
    assert_eq!(stream2mmap.self_area, None, "empty self_area is no data");
    let top = &d.tasks["VecAdd"];
    assert_eq!(top.total_area, None, "empty total_area is no data");
}

#[test]
fn vadd_instance_args() {
    let d = TaskGraph::from_json(&fixture("topology", "vadd_design.json")).expect("parse");
    let top = &d.tasks["VecAdd"];
    let add_instances = &top.tasks["Add"];
    assert_eq!(add_instances.len(), 1, "one Add instance");
    let instance = &add_instances[0];
    assert_eq!(instance.args.len(), 4, "4 args");
    let a_arg = &instance.args["a"];
    assert_eq!(a_arg.name(), Some("a_q"), "a connects to a_q FIFO");
    assert_eq!(a_arg.cat, ArgCategory::Istream, "a is istream");
}

#[test]
fn channelized_mmap_port_deserializes() {
    let g = TaskGraph::from_json(&fixture("task-graph", "mmap_ports.json")).expect("parse");
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
    let g = TaskGraph::from_json(&fixture("task-graph", "mmap_ports.json")).expect("parse");
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
        "schema_version": 3,
        "cflags": [], "top": "T", "target": "xilinx-hls",
        "tasks": {"T": {"srcs": ["T.cpp"], "include_dirs": [], "defines": [], "level": "upper", "synth": "hls",
            "readable_name": "T",
            "tasks": {"C": [{"args": {}, "step": -1}]}, "fifos": {}, "ports": []}}
    }"#;
    let g = TaskGraph::from_json(json).expect("parse negative step");
    assert_eq!(g.tasks["T"].tasks["C"][0].step, -1, "negative step");
}

/// `readable_name` is the one `tapacc`-emitted field that used to ride in
/// the untyped `extra` bag. It is a typed field now: it must parse and
/// survive a round-trip. `tapacc` emits the demangled template
/// specialization for template tasks and the plain task name otherwise.
#[test]
fn readable_name_is_typed_and_round_trips() {
    let json = r#"{
        "schema_version": 3,
        "cflags": [], "top": "T", "target": "xilinx-hls",
        "tasks": {
            "T": {"srcs": ["T.cpp"], "include_dirs": [], "defines": [], "level": "upper", "synth": "hls",
                  "readable_name": "Compute<float, 4>",
                  "tasks": {}, "fifos": {}, "ports": []},
            "U": {"srcs": ["U.cpp"], "include_dirs": [], "defines": [], "level": "lower", "synth": "hls",
                  "readable_name": "U", "ports": []}
        }
    }"#;
    let g = TaskGraph::from_json(json).expect("parse readable_name");
    assert_eq!(g.tasks["T"].readable_name, "Compute<float, 4>");
    assert_eq!(g.tasks["U"].readable_name, "U", "non-template name");

    let g2 = TaskGraph::from_json(&serde_json::to_string_pretty(&g).expect("serialize"))
        .expect("reparse");
    assert_eq!(g, g2, "readable_name round-trips");
}

/// `tapacc` emits `readable_name` unconditionally for every task, so the
/// field is required rather than defaulted: a payload that omits it does
/// not describe real `tapacc` output and must be rejected outright. A
/// `#[serde(default)]` here would silently paper over a malformed graph.
#[test]
fn readable_name_is_required() {
    let json = r#"{
        "schema_version": 3,
        "cflags": [], "top": "T", "target": "xilinx-hls",
        "tasks": {
            "T": {"srcs": ["T.cpp"], "include_dirs": [], "defines": [], "level": "lower", "synth": "hls", "ports": []}
        }
    }"#;
    let err = TaskGraph::from_json(json).expect_err("omitted readable_name must fail");
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

/// Schema v3's per-task file manifest (`srcs`, `include_dirs`, `defines`)
/// is required on the wire: producers must emit it. A `#[serde(default)]`
/// would let a producer drop the fields and have every task read as
/// source-less, surfacing far away at synth time.
#[test]
fn source_manifest_fields_are_required() {
    for missing in ["srcs", "include_dirs", "defines"] {
        let json = r#"{
            "schema_version": 3,
            "cflags": [], "top": "T", "target": "xilinx-hls",
            "tasks": {
                "T": {"srcs": ["T.cpp"], "include_dirs": [], "defines": [],
                    "level": "lower", "synth": "hls", "readable_name": "T",
                    "ports": []}
            }
        }"#
        .replace(missing_member(missing), "");
        let err =
            TaskGraph::from_json(&json).expect_err(&format!("removing `{missing}` must fail"));
        assert!(
            err.to_string().contains(missing),
            "error must name the missing field `{missing}`; got {err}",
        );
    }
}

/// The full wire member one iteration of the test above removes.
fn missing_member(field: &str) -> &'static str {
    match field {
        "srcs" => r#""srcs": ["T.cpp"], "#,
        "include_dirs" => r#""include_dirs": [], "#,
        _ => r#""defines": [],"#,
    }
}

#[test]
fn consumer_only_fifo() {
    let json = r#"{
        "schema_version": 3,
        "cflags": [], "top": "T", "target": "xilinx-hls",
        "tasks": {"T": {"srcs": ["T.cpp"], "include_dirs": [], "defines": [], "level": "upper", "synth": "hls",
            "readable_name": "T",
            "tasks": {}, "fifos": {"ext": {"consumed_by": ["X", 0]}}, "ports": []}}
    }"#;
    let g = TaskGraph::from_json(json).expect("parse consumer-only FIFO");
    let f = &g.tasks["T"].fifos["ext"];
    assert!(f.consumed_by.is_some(), "has consumer");
    assert!(f.produced_by.is_none(), "no producer");
    assert!(f.depth.is_none(), "no depth");
}

#[test]
fn mmap_port_category_round_trips() {
    let json = r#"{
        "schema_version": 3,
        "top": "T", "target": "xilinx-hls",
        "tasks": {"T": {"level": "lower", "srcs": ["T.cpp"], "include_dirs": [], "defines": [], "synth": "hls",
            "readable_name": "T",
            "ports": [{"cat": "mmap", "name": "data", "type": "float*", "width": 32}]}}
    }"#;
    let d = TaskGraph::from_json(json).expect("parse mmap port");
    assert_eq!(d.tasks["T"].ports[0].cat, ArgCategory::Mmap);
    let serialized = to_json(&d);
    assert!(serialized.contains(r#""mmap""#), "round-trips as mmap");
}

#[test]
fn vadd_round_trip() {
    for (path, json) in vadd_fixtures() {
        let g1 = TaskGraph::from_json(&json).unwrap_or_else(|e| panic!("parse {path}: {e}"));
        let serialized = serde_json::to_string_pretty(&g1).expect("serialize");
        let g2 = TaskGraph::from_json(&serialized).expect("parse 2");
        assert_eq!(g1, g2, "round-trip equality");
    }
}

#[test]
fn mmap_ports_round_trip() {
    let json = fixture("task-graph", "mmap_ports.json");
    let g1 = TaskGraph::from_json(&json).expect("parse 1");
    let serialized = serde_json::to_string_pretty(&g1).expect("serialize");
    let g2 = TaskGraph::from_json(&serialized).expect("parse 2");
    assert_eq!(g1, g2, "round-trip equality");
}

#[test]
fn annotation_round_trip() {
    let json = fixture("topology", "vadd_design.json");
    let d1 = TaskGraph::from_json(&json).expect("parse");
    let serialized = to_json(&d1);
    let d2 = TaskGraph::from_json(&serialized).expect("parse 2");

    let m2s_1 = &d1.tasks["Mmap2Stream"];
    let m2s_2 = &d2.tasks["Mmap2Stream"];
    assert_eq!(m2s_1.self_area, m2s_2.self_area, "self_area round-trips");
    assert_eq!(m2s_1.total_area, m2s_2.total_area, "total_area round-trips");
    assert_eq!(
        m2s_1.clock_period, m2s_2.clock_period,
        "clock_period round-trips"
    );
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
    let design = TaskGraph::from_json(json).expect("parse canonical task graph");
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
fn task_order_is_sorted_not_as_written() {
    let json = concat!(
        r#"{"schema_version": 3, "top": "VecAdd", "target": "xilinx-hls", "cflags": [], "tasks": {"#,
        r#""VecAdd": {"level": "upper", "srcs": ["VecAdd.cpp"], "include_dirs": [], "defines": [], "readable_name": "VecAdd", "#,
        r#""synth": "hls", "ports": [], "tasks": {}, "fifos": {}}, "#,
        r#""Add": {"level": "lower", "srcs": ["Add.cpp"], "include_dirs": [], "defines": [], "readable_name": "Add", "#,
        r#""synth": "hls", "ports": [], "tasks": {}, "fifos": {}}}}"#,
    );
    let design = TaskGraph::from_json(json).expect("parse");
    let names: Vec<&str> = design.tasks.keys().map(String::as_str).collect();
    assert_eq!(
        names,
        vec!["Add", "VecAdd"],
        "task keys must sort alphabetically, not follow input order",
    );

    // The emitted form is sorted too, not just the in-memory map.
    let reparsed = TaskGraph::from_json(&to_compact_json(&design)).expect("reparse");
    let emitted: Vec<&str> = reparsed.tasks.keys().map(String::as_str).collect();
    assert_eq!(emitted, vec!["Add", "VecAdd"], "sorted order survives emit");
}

/// A frontend payload is one JSON document. Trailing output must not be
/// discarded after a valid graph prefix.
#[test]
fn graph_parser_rejects_trailing_data() {
    for (path, json) in vadd_fixtures() {
        let graph = TaskGraph::from_json(&json).unwrap_or_else(|e| panic!("parse {path}: {e}"));
        let padded = format!("{json}\n\t\r ");
        assert_eq!(
            TaskGraph::from_json(&padded).expect("trailing whitespace is valid"),
            graph,
            "JSON whitespace must not change the graph",
        );

        for suffix in [json.as_str(), "\nnot-json", " []"] {
            let malformed = format!("{json}{suffix}");
            TaskGraph::from_json(&malformed)
                .expect_err("a graph must consume the entire JSON payload");
        }
    }
}

#[test]
fn unknown_top_level_field_rejected() {
    let json = r#"{"schema_version": 3, "cflags": [], "top": "T", "target": "xilinx-hls", "tasks": {}, "bogus": true}"#;
    let err = TaskGraph::from_json(json).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("bogus") || msg.contains("unknown"),
        "error mentions field: {msg}"
    );
}

/// `Task` carries `deny_unknown_fields`: an unknown *per-task* field is a
/// malformed graph, not something to round-trip through an `extra` bag.
/// The error must also carry the `tasks.<name>` path pointer that makes a
/// deep schema error actionable.
#[test]
fn unknown_task_field_rejected() {
    let json = concat!(
        r#"{"schema_version": 3, "top": "T", "target": "xilinx-hls", "cflags": [], "tasks": {"#,
        r#""T": {"level": "lower", "srcs": ["T.cpp"], "include_dirs": [], "defines": [], "readable_name": "T", "synth": "hls", "#,
        r#""ports": [], "tasks": {}, "fifos": {}, "bogus_field": 1}}}"#,
    );
    let err = TaskGraph::from_json(json).expect_err("unknown task field must fail");
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

#[test]
fn missing_top_field() {
    let json = r#"{"schema_version": 3, "target": "xilinx-hls", "tasks": {}}"#;
    let err = TaskGraph::from_json(json).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("top") || msg.contains("missing"),
        "error about top: {msg}"
    );
}

#[test]
fn invalid_level_rejected() {
    let json = r#"{
        "schema_version": 3,
        "cflags": [], "top": "T", "target": "xilinx-hls",
        "tasks": {"T": {"srcs": ["T.cpp"], "include_dirs": [], "defines": [], "level": "invalid", "synth": "hls"}}
    }"#;
    let err = TaskGraph::from_json(json).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("level") || msg.contains("invalid"),
        "error mentions level: {msg}"
    );
}
