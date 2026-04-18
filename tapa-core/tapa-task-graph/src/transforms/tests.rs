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
                "code": "void VecAdd() {}",
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
    assert_eq!(top.code, "void VecAdd() {}");
    assert_eq!(top.level, TaskLevel::Upper);
    assert_eq!(top.target, "hls");
    assert_eq!(top.vendor, "xilinx");
}

#[test]
fn flatten_rejects_deep_hierarchy() {
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
    let err = flatten(&g).expect_err("must reject deep");
    assert!(matches!(err, TransformError::DeepHierarchyNotSupported(_)));
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
