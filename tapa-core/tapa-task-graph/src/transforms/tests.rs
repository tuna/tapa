//! Unit tests for `super::flatten` and `super::apply_floorplan`.

use std::collections::BTreeMap;

use super::{
    apply_floorplan, convert_region_format, flatten, region_to_slot_name, TransformError,
};
use crate::graph::Graph;
use crate::interconnect::EndpointRef;
use crate::task::TaskLevel;

fn vadd_two_level_graph_json() -> &'static str {
    r#"{
        "cflags": ["-std=c++14"],
        "top": "VecAdd",
        "tasks": {
            "VecAdd": {
                "code": "extern \"C\" {\nvoid VecAdd(uint64_t n);\n}  // extern \"C\"\n\nextern \"C\" {\nvoid VecAdd(uint64_t n) { /* top body */ }\n}  // extern \"C\"\n",
                "level": "upper",
                "target": "hls",
                "vendor": "xilinx",
                "ports": [
                    {"cat": "scalar", "name": "n", "type": "uint64_t", "width": 64}
                ],
                "tasks": {
                    "A": [{"args": {
                        "n": {"arg": "n", "cat": "scalar"},
                        "out": {"arg": "fifo", "cat": "ostream"}
                    }, "step": 0}],
                    "B": [{"args": {
                        "n": {"arg": "n", "cat": "scalar"},
                        "in": {"arg": "fifo", "cat": "istream"}
                    }, "step": 0}]
                },
                "fifos": {
                    "fifo": {
                        "depth": 2,
                        "consumed_by": ["B", 0],
                        "produced_by": ["A", 0]
                    }
                }
            },
            "A": {
                "code": "void A() {}",
                "level": "lower",
                "target": "hls",
                "vendor": "xilinx",
                "ports": [
                    {"cat": "scalar", "name": "n", "type": "uint64_t", "width": 64},
                    {"cat": "ostream", "name": "out", "type": "float", "width": 32}
                ]
            },
            "B": {
                "code": "void B() {}",
                "level": "lower",
                "target": "hls",
                "vendor": "xilinx",
                "ports": [
                    {"cat": "scalar", "name": "n", "type": "uint64_t", "width": 64},
                    {"cat": "istream", "name": "in", "type": "float", "width": 32}
                ]
            }
        }
    }"#
}

#[test]
fn flatten_collapses_two_level_hierarchy() {
    let g = Graph::from_json(vadd_two_level_graph_json()).expect("parse");
    let out = flatten(&g).expect("flatten ok");
    assert_eq!(out.top, "VecAdd");
    let new_top = out.tasks.get("VecAdd").expect("top survives");
    let a_inst = &new_top.tasks["A"][0];
    assert_eq!(a_inst.args["out"].arg, "fifo_VecAdd");
    assert_eq!(a_inst.args["n"].arg, "n");
    let b_inst = &new_top.tasks["B"][0];
    assert_eq!(b_inst.args["in"].arg, "fifo_VecAdd");
    let fifo = new_top.fifos.get("fifo_VecAdd").expect("fifo renamed");
    assert_eq!(fifo.consumed_by, Some(EndpointRef("B".to_string(), 0)));
    assert_eq!(fifo.produced_by, Some(EndpointRef("A".to_string(), 0)));
    assert_eq!(fifo.depth, Some(2));
}

#[test]
fn flatten_preserves_top_metadata() {
    let g = Graph::from_json(vadd_two_level_graph_json()).expect("parse");
    let out = flatten(&g).expect("flatten ok");
    let top = out.tasks.get("VecAdd").expect("top survives");
    assert_eq!(top.ports.len(), 1);
    assert_eq!(top.ports[0].name, "n");
    assert_eq!(out.cflags, vec!["-std=c++14".to_string()]);
    assert!(top.code.contains("VecAdd"), "top code should still mention VecAdd");
    assert_eq!(top.level, TaskLevel::Upper);
    assert_eq!(top.target, "hls");
    assert_eq!(top.vendor, "xilinx");
}

/// Round-16 regression: the previous port rejected any design where
/// the top had upper-level children with `DeepHierarchyNotSupported`.
/// Python's `Graph.get_flatten_graph` recursively collects leaves, so
/// the Rust port now matches — even an "empty" nested upper must
/// round-trip cleanly without an error.
#[test]
fn flatten_accepts_nested_upper_without_error() {
    let json = r#"{
        "cflags": [],
        "top": "Outer",
        "tasks": {
            "Outer": {
                "code": "", "level": "upper", "target": "hls", "vendor": "xilinx",
                "ports": [],
                "tasks": {"Inner": [{"args": {}, "step": 0}]},
                "fifos": {}
            },
            "Inner": {
                "code": "", "level": "upper", "target": "hls", "vendor": "xilinx",
                "ports": [],
                "tasks": {}, "fifos": {}
            }
        }
    }"#;
    let g = Graph::from_json(json).expect("parse");
    let out = flatten(&g).expect("recursive flatten ok");
    assert_eq!(out.top, "Outer");
    // Inner has no tasks → no leaves, top's `tasks` map is empty.
    assert!(out.tasks["Outer"].tasks.is_empty());
}

/// End-to-end nested flatten: a top that indirects through an upper
/// child holding a leaf must produce the leaf at the flattened top.
/// Matches the Python `recursive_get_interconnect_insts` +
/// `get_leaf_tasks_insts` shape.
#[test]
fn flatten_hoists_leaf_under_nested_upper() {
    let json = r#"{
        "cflags": [],
        "top": "Outer",
        "tasks": {
            "Outer": {
                "code": "", "level": "upper", "target": "hls", "vendor": "xilinx",
                "ports": [
                    {"cat": "scalar", "name": "n", "type": "uint64_t", "width": 64}
                ],
                "tasks": {"Inner": [{"step": 0, "args": {
                    "p": {"arg": "n", "cat": "scalar"}
                }}]},
                "fifos": {}
            },
            "Inner": {
                "code": "", "level": "upper", "target": "hls", "vendor": "xilinx",
                "ports": [
                    {"cat": "scalar", "name": "p", "type": "uint64_t", "width": 64}
                ],
                "tasks": {"Leaf": [{"step": 0, "args": {
                    "q": {"arg": "p", "cat": "scalar"}
                }}]},
                "fifos": {}
            },
            "Leaf": {
                "code": "void Leaf() {}", "level": "lower", "target": "hls",
                "vendor": "xilinx",
                "ports": [
                    {"cat": "scalar", "name": "q", "type": "uint64_t", "width": 64}
                ]
            }
        }
    }"#;
    let g = Graph::from_json(json).expect("parse");
    let out = flatten(&g).expect("recursive flatten");
    let top = out.tasks.get("Outer").expect("top");
    let leaf_insts = top.tasks.get("Leaf").expect("leaf hoisted under top");
    assert_eq!(leaf_insts.len(), 1);
    // The leaf's `q` arg must resolve to the outermost `n` binding
    // (promoted through `p` in Inner → `n` in Outer).
    let arg = leaf_insts[0].args.get("q").expect("q arg present");
    assert_eq!(arg.arg, "n", "nested scalar arg must promote to Outer's external port");
}

#[test]
fn apply_floorplan_creates_slot_tasks() {
    let g = Graph::from_json(vadd_two_level_graph_json()).expect("parse");
    let flat = flatten(&g).expect("flatten");
    let mut slot_to_insts: BTreeMap<String, Vec<String>> = BTreeMap::new();
    slot_to_insts.insert("SLOT_X0Y0".to_string(), vec!["A_0".to_string()]);
    slot_to_insts.insert("SLOT_X0Y1".to_string(), vec!["B_0".to_string()]);
    let (out, regions) = apply_floorplan(&flat, &slot_to_insts).expect("apply");

    assert!(out.tasks.contains_key("SLOT_X0Y0"));
    assert!(out.tasks.contains_key("SLOT_X0Y1"));
    assert_eq!(out.tasks["SLOT_X0Y0"].level, TaskLevel::Upper);

    let s0 = &out.tasks["SLOT_X0Y0"];
    assert!(s0.tasks.contains_key("A"), "slot SLOT_X0Y0 must wrap A_0");
    assert_eq!(s0.tasks["A"].len(), 1);

    let top = &out.tasks["VecAdd"];
    assert!(top.tasks.contains_key("SLOT_X0Y0"));
    assert!(top.tasks.contains_key("SLOT_X0Y1"));
    assert_eq!(top.tasks["SLOT_X0Y0"].len(), 1);

    let cross = top
        .fifos
        .get("fifo_VecAdd")
        .expect("cross-slot fifo retained");
    assert_eq!(
        cross.produced_by,
        Some(EndpointRef("SLOT_X0Y0".to_string(), 0)),
    );
    assert_eq!(
        cross.consumed_by,
        Some(EndpointRef("SLOT_X0Y1".to_string(), 0)),
    );

    assert!(regions.contains_key("SLOT_X0Y0"));
}

#[test]
fn apply_floorplan_normalizes_region_names() {
    assert_eq!(
        region_to_slot_name("SLOT_X0Y0:SLOT_X0Y1"),
        "SLOT_X0Y0_SLOT_X0Y1",
    );
    assert_eq!(
        convert_region_format("SLOT_X0Y0:SLOT_X0Y1"),
        "SLOT_X0Y0_TO_SLOT_X0Y1",
    );
    assert_eq!(convert_region_format("solo"), "solo");
}

#[test]
fn apply_floorplan_rejects_unknown_instance() {
    let g = Graph::from_json(vadd_two_level_graph_json()).expect("parse");
    let flat = flatten(&g).expect("flatten");
    let mut slot_to_insts: BTreeMap<String, Vec<String>> = BTreeMap::new();
    slot_to_insts.insert("SLOT".to_string(), vec!["NoSuch_0".to_string()]);
    let err = apply_floorplan(&flat, &slot_to_insts).expect_err("must reject");
    assert!(matches!(err, TransformError::UnknownFloorplanInstance(_)));
}

/// Snapshot the slot wrapper C++ and a compact form of the rewritten
/// graph so regressions in `gen_slot_cpp` wiring or port plumbing show
/// up as diff noise on this test.
#[test]
fn apply_floorplan_emits_slot_cpp_wrapper() {
    let g = Graph::from_json(vadd_two_level_graph_json()).expect("parse");
    let flat = flatten(&g).expect("flatten");
    let mut slot_to_insts: BTreeMap<String, Vec<String>> = BTreeMap::new();
    slot_to_insts.insert("SLOT_X0Y0".to_string(), vec!["A_0".to_string()]);
    slot_to_insts.insert("SLOT_X0Y1".to_string(), vec!["B_0".to_string()]);
    let (out, _regions) = apply_floorplan(&flat, &slot_to_insts).expect("apply");

    // --- Slot wrapper code snapshot ---------------------------------
    let slot_a = &out.tasks["SLOT_X0Y0"];
    let code_a = &slot_a.code;
    assert!(
        code_a.contains("void SLOT_X0Y0("),
        "slot wrapper must declare SLOT_X0Y0 fn; got:\n{code_a}",
    );
    assert!(
        code_a.contains("uint64_t n"),
        "slot wrapper must forward the scalar `n`; got:\n{code_a}",
    );
    assert!(
        code_a.contains("tapa::ostream<float>&"),
        "slot A must expose an ostream port (FIFO into SLOT_X0Y1); got:\n{code_a}",
    );
    assert!(
        code_a.contains("#pragma HLS interface ap_fifo"),
        "slot wrapper must stamp HLS fifo pragma; got:\n{code_a}",
    );
    assert!(
        !code_a.contains("TODO(rust-port)"),
        "slot code must no longer carry the TODO placeholder; got:\n{code_a}",
    );

    let slot_b = &out.tasks["SLOT_X0Y1"];
    let code_b = &slot_b.code;
    assert!(
        code_b.contains("void SLOT_X0Y1("),
        "slot wrapper must declare SLOT_X0Y1 fn; got:\n{code_b}",
    );
    assert!(
        code_b.contains("tapa::istream<float>&"),
        "slot B must expose an istream port (FIFO out of SLOT_X0Y0); got:\n{code_b}",
    );

    // --- Port-type plumbing snapshot --------------------------------
    // Regression: pre-fix, FIFO/mmap ports were emitted with empty
    // `ctype` + `width=0`, which makes HLS wrappers uncompilable.
    let fifo_port = slot_a
        .ports
        .iter()
        .find(|p| p.name == "fifo_VecAdd")
        .expect("slot A must carry the bridged FIFO port");
    assert_eq!(fifo_port.ctype, "float", "FIFO port type must come from A.out");
    assert_eq!(fifo_port.width, 32, "FIFO port width must come from A.out");

    // --- Design.json snapshot via serde round-trip ------------------
    // The rewritten graph is what feeds `design.json`, so capturing its
    // shape here catches regressions in slot instantiation / fifo /
    // port emission without needing the CLI to be in-process.
    let graph_json = out.to_json().expect("re-serialize floorplanned graph");
    let parsed: serde_json::Value = serde_json::from_str(&graph_json).expect("parse");
    let tasks = parsed["tasks"].as_object().expect("tasks object");
    let slot_names: Vec<&String> = tasks.keys().collect();
    assert!(
        slot_names.iter().any(|k| k.as_str() == "SLOT_X0Y0"),
        "rewritten graph must carry SLOT_X0Y0",
    );
    assert!(
        slot_names.iter().any(|k| k.as_str() == "SLOT_X0Y1"),
        "rewritten graph must carry SLOT_X0Y1",
    );
    let top_tasks = parsed["tasks"]["VecAdd"]["tasks"]
        .as_object()
        .expect("top tasks object");
    assert!(
        top_tasks.contains_key("SLOT_X0Y0") && top_tasks.contains_key("SLOT_X0Y1"),
        "top must instantiate both slots; got keys: {:?}",
        top_tasks.keys().collect::<Vec<_>>(),
    );
}
