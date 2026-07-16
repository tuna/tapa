//! Unit tests for `super::flatten`.

use super::flatten;
use crate::graph::Graph;
use crate::interconnect::EndpointRef;
use crate::synth_target::SynthTarget;
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
    assert!(
        top.code.contains("VecAdd"),
        "top code should still mention VecAdd"
    );
    assert_eq!(top.level, TaskLevel::Upper);
    assert_eq!(top.target, SynthTarget::Hls);
    // `vendor` is not a typed field; it round-trips through `extra`.
    assert_eq!(
        top.extra.get("vendor").and_then(|v| v.as_str()),
        Some("xilinx")
    );
}

/// Even an empty nested upper task must flatten without an error.
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
/// Matches the `recursive_get_interconnect_insts` +
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
    assert_eq!(
        arg.arg, "n",
        "nested scalar arg must promote to Outer's external port"
    );
}

#[test]
fn flatten_uses_explicit_upper_instance_names_in_nested_fifo_paths() {
    let json = r#"{
        "cflags": [],
        "top": "Outer",
        "tasks": {
            "Outer": {
                "code": "", "level": "upper", "target": "hls", "vendor": "xilinx",
                "ports": [],
                "tasks": {"Cluster": [
                    {"name": "west_cluster", "step": 0, "args": {}},
                    {"name": "east_cluster", "step": 0, "args": {}}
                ]},
                "fifos": {}
            },
            "Cluster": {
                "code": "", "level": "upper", "target": "hls", "vendor": "xilinx",
                "ports": [],
                "tasks": {"Stage": [{
                    "name": "compute_stage", "step": 0, "args": {}
                }]},
                "fifos": {}
            },
            "Stage": {
                "code": "", "level": "upper", "target": "hls", "vendor": "xilinx",
                "ports": [],
                "tasks": {
                    "Source": [{"step": 0, "args": {
                        "out": {"arg": "q", "cat": "ostream"}
                    }}],
                    "Sink": [{"step": 0, "args": {
                        "in": {"arg": "q", "cat": "istream"}
                    }}]
                },
                "fifos": {"q": {
                    "depth": 4,
                    "consumed_by": ["Sink", 0],
                    "produced_by": ["Source", 0]
                }}
            },
            "Source": {
                "code": "", "level": "lower", "target": "hls", "vendor": "xilinx",
                "ports": [{"cat": "ostream", "name": "out", "type": "int", "width": 32}]
            },
            "Sink": {
                "code": "", "level": "lower", "target": "hls", "vendor": "xilinx",
                "ports": [{"cat": "istream", "name": "in", "type": "int", "width": 32}]
            }
        }
    }"#;
    let graph = Graph::from_json(json).expect("parse");
    let flattened = flatten(&graph).expect("flatten");
    let top = &flattened.tasks["Outer"];

    let west_fifo = "q_compute_stage_west_cluster_Outer";
    let east_fifo = "q_compute_stage_east_cluster_Outer";
    assert_eq!(top.tasks["Source"][0].args["out"].arg, west_fifo);
    assert_eq!(top.tasks["Sink"][0].args["in"].arg, west_fifo);
    assert_eq!(top.tasks["Source"][1].args["out"].arg, east_fifo);
    assert_eq!(top.tasks["Sink"][1].args["in"].arg, east_fifo);

    assert_eq!(
        top.fifos[west_fifo].produced_by,
        Some(EndpointRef("Source".to_owned(), 0))
    );
    assert_eq!(
        top.fifos[west_fifo].consumed_by,
        Some(EndpointRef("Sink".to_owned(), 0))
    );
    assert_eq!(
        top.fifos[east_fifo].produced_by,
        Some(EndpointRef("Source".to_owned(), 1))
    );
    assert_eq!(
        top.fifos[east_fifo].consumed_by,
        Some(EndpointRef("Sink".to_owned(), 1))
    );
}

#[test]
fn flatten_resolves_indexed_stream_bundle_args_through_parent_binding() {
    let json = r#"{
        "cflags": [],
        "top": "Outer",
        "tasks": {
            "Outer": {
                "code": "", "level": "upper", "target": "hls", "vendor": "xilinx",
                "ports": [],
                "tasks": {"Stage": [{"step": 0, "args": {
                    "in_q[0]": {"arg": "qs[4]", "cat": "istream"},
                    "in_q[1]": {"arg": "qs[5]", "cat": "istream"},
                    "in_q[2]": {"arg": "qs[6]", "cat": "istream"},
                    "in_q[3]": {"arg": "qs[7]", "cat": "istream"}
                }}]},
                "fifos": {
                    "qs[4]": {"depth": 2, "consumed_by": ["Stage", 0]},
                    "qs[5]": {"depth": 2, "consumed_by": ["Stage", 0]},
                    "qs[6]": {"depth": 2, "consumed_by": ["Stage", 0]},
                    "qs[7]": {"depth": 2, "consumed_by": ["Stage", 0]}
                }
            },
            "Stage": {
                "code": "", "level": "upper", "target": "hls", "vendor": "xilinx",
                "ports": [
                    {"cat": "istreams", "name": "in_q", "type": "uint64_t", "width": 64}
                ],
                "tasks": {"Leaf": [{"step": 0, "args": {
                    "pkt_in": {"arg": "in_q[3]", "cat": "istream"}
                }}]},
                "fifos": {
                    "in_q[0]": {"consumed_by": ["Leaf", 0]},
                    "in_q[1]": {"consumed_by": ["Leaf", 0]},
                    "in_q[2]": {"consumed_by": ["Leaf", 0]},
                    "in_q[3]": {"consumed_by": ["Leaf", 0]}
                }
            },
            "Leaf": {
                "code": "", "level": "lower", "target": "hls", "vendor": "xilinx",
                "ports": [
                    {"cat": "istream", "name": "pkt_in", "type": "uint64_t", "width": 64}
                ]
            }
        }
    }"#;
    let g = Graph::from_json(json).expect("parse");
    let out = flatten(&g).expect("flatten");
    let top = out.tasks.get("Outer").expect("top survives");
    let leaf = &top.tasks["Leaf"][0];
    assert_eq!(
        leaf.args["pkt_in"].arg, "qs[7]_Outer",
        "leaf indexed stream arg must resolve through Stage.in_q[3] -> qs[7]"
    );

    let fifo = top.fifos.get("qs[7]_Outer").expect("fifo renamed");
    assert_eq!(fifo.consumed_by, Some(EndpointRef("Leaf".to_string(), 0)));
}
