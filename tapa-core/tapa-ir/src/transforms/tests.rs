//! Unit tests for the flattening transforms.

use super::{flatten, flatten_with_origins, OccurrenceId};
use crate::graph::TaskGraph;
use crate::interconnect::EndpointRef;
use crate::synth_target::SynthTarget;
use crate::task::TaskLevel;

fn vadd_two_level_graph_json() -> &'static str {
    r#"{
        "schema_version": 4,
        "cflags": ["-std=c++14"],
        "top": "VecAdd",
        "target": "xilinx-hls",
        "tasks": {
            "VecAdd": {
                "readable_name": "VecAdd",
                "srcs": ["VecAdd.cpp"], "include_dirs": [], "defines": [],
                "level": "upper",
                "synth": "hls",
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
                "readable_name": "A",
                "srcs": ["A.cpp"], "include_dirs": [], "defines": [],
                "level": "lower",
                "synth": "hls",
                "ports": [
                    {"cat": "scalar", "name": "n", "type": "uint64_t", "width": 64},
                    {"cat": "ostream", "name": "out", "type": "float", "width": 32}
                ]
            },
            "B": {
                "readable_name": "B",
                "srcs": ["B.cpp"], "include_dirs": [], "defines": [],
                "level": "lower",
                "synth": "hls",
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
    let g = TaskGraph::from_json(vadd_two_level_graph_json()).expect("parse");
    let out = flatten(&g).expect("flatten ok");
    assert_eq!(out.top, "VecAdd");
    let new_top = out.tasks.get("VecAdd").expect("top survives");
    let a_inst = &new_top.tasks["A"][0];
    assert_eq!(a_inst.args["out"].name(), Some("fifo_VecAdd"));
    assert_eq!(a_inst.args["n"].name(), Some("n"));
    let b_inst = &new_top.tasks["B"][0];
    assert_eq!(b_inst.args["in"].name(), Some("fifo_VecAdd"));
    let fifo = new_top.fifos.get("fifo_VecAdd").expect("fifo renamed");
    assert_eq!(fifo.consumed_by, Some(EndpointRef("B".to_string(), 0)));
    assert_eq!(fifo.produced_by, Some(EndpointRef("A".to_string(), 0)));
    assert_eq!(fifo.depth, Some(2));
}

/// Kernel stream ports retain their boundary names when hierarchy flattening
/// promotes their child bindings; only internal FIFOs need a scope suffix.
#[test]
fn flatten_preserves_external_stream_boundary_names() {
    for is_array in [false, true] {
        let mut graph = TaskGraph::from_json(vadd_two_level_graph_json()).expect("parse");
        let input_name = if is_array { "input[0]" } else { "input" };
        let output_name = if is_array { "output[0]" } else { "output" };
        let top = graph.tasks.get_mut("VecAdd").expect("top task");
        top.fifos.clear();
        for (name, consumed_by, produced_by) in [
            (input_name, Some(EndpointRef("B".into(), 0)), None),
            (output_name, None, Some(EndpointRef("A".into(), 0))),
        ] {
            top.fifos.insert(
                name.into(),
                crate::InterconnectDefinition {
                    depth: None,
                    consumed_by,
                    produced_by,
                },
            );
        }
        top.tasks.get_mut("A").expect("producer")[0]
            .args
            .get_mut("out")
            .expect("output binding")
            .arg = crate::ArgSource::Name(output_name.into());
        top.tasks.get_mut("B").expect("consumer")[0]
            .args
            .get_mut("in")
            .expect("input binding")
            .arg = crate::ArgSource::Name(input_name.into());
        for (name, category) in [
            ("input", if is_array { "istreams" } else { "istream" }),
            ("output", if is_array { "ostreams" } else { "ostream" }),
        ] {
            top.ports.push(
                serde_json::from_str(&format!(
                    r#"{{"name":"{name}","cat":"{category}","type":"float","width":32}}"#
                ))
                .expect("stream port"),
            );
        }

        let out = flatten(&graph).expect("flatten");
        let top = &out.tasks["VecAdd"];
        assert!(
            top.fifos.contains_key(input_name),
            "external input {input_name}"
        );
        assert!(
            top.fifos.contains_key(output_name),
            "external output {output_name}"
        );
        assert_eq!(top.tasks["B"][0].args["in"].name(), Some(input_name));
        assert_eq!(top.tasks["A"][0].args["out"].name(), Some(output_name));
        assert_eq!(
            top.fifos[input_name].consumed_by,
            Some(EndpointRef("B".into(), 0))
        );
        assert_eq!(
            top.fifos[output_name].produced_by,
            Some(EndpointRef("A".into(), 0))
        );
    }
}

/// A boundary port can have the same spelling as another FIFO's global name;
/// flattening must report the collision before it overwrites either connection.
#[test]
fn flatten_rejects_external_internal_fifo_name_collision() {
    let mut graph = TaskGraph::from_json(vadd_two_level_graph_json()).expect("parse");
    let boundary_name = "fifo_VecAdd";
    let mut external_port = graph.tasks["B"].port("in").expect("consumer port").clone();
    external_port.name = boundary_name.into();
    let top = graph.tasks.get_mut("VecAdd").expect("top task");
    top.ports.push(external_port);
    let consumers = top.tasks.get_mut("B").expect("consumer task");
    let mut external_consumer = consumers[0].clone();
    external_consumer
        .args
        .get_mut("in")
        .expect("input binding")
        .arg = crate::ArgSource::Name(boundary_name.into());
    consumers.push(external_consumer);
    top.fifos.insert(
        boundary_name.into(),
        crate::InterconnectDefinition {
            depth: None,
            consumed_by: Some(EndpointRef("B".into(), 1)),
            produced_by: None,
        },
    );

    let error = flatten(&graph).expect_err("distinct FIFOs must not merge");
    assert!(error.to_string().contains(boundary_name), "{error}");
}

#[test]
fn flatten_preserves_top_metadata() {
    let g = TaskGraph::from_json(vadd_two_level_graph_json()).expect("parse");
    let out = flatten(&g).expect("flatten ok");
    let top = out.tasks.get("VecAdd").expect("top survives");
    assert_eq!(top.ports.len(), 1);
    assert_eq!(top.ports[0].name, "n");
    assert_eq!(out.cflags, vec!["-std=c++14".to_string()]);
    assert!(
        top.srcs.iter().any(|src| src.contains("VecAdd")),
        "top srcs should still mention VecAdd"
    );
    assert_eq!(top.level, TaskLevel::Upper);
    assert_eq!(top.synth, SynthTarget::Hls);
}

/// A constant bound at an upper-task invoke site must reach the leaves
/// below it: with name-only bindings the leaf's arg stayed a dangling
/// `Name` of the middle task's port — a wire that exists nowhere.
#[test]
fn flatten_propagates_constants_through_upper_tasks() {
    let g = TaskGraph::from_json(
        r#"{
        "schema_version": 4,
        "cflags": [],
        "top": "Top",
        "target": "xilinx-hls",
        "tasks": {
            "Top": {
                "readable_name": "Top",
                "srcs": ["Top.cpp"], "include_dirs": [], "defines": [],
                "level": "upper",
                "synth": "hls",
                "ports": [],
                "tasks": {
                    "Mid": [{"args": {
                        "n": {"arg": {"width": 64, "value": 16}, "cat": "scalar"}
                    }, "step": 0}]
                },
                "fifos": {}
            },
            "Mid": {
                "readable_name": "Mid",
                "srcs": ["Mid.cpp"], "include_dirs": [], "defines": [],
                "level": "upper",
                "synth": "hls",
                "ports": [
                    {"cat": "scalar", "name": "n", "type": "uint64_t", "width": 64}
                ],
                "tasks": {
                    "Leaf": [{"args": {
                        "n": {"arg": "n", "cat": "scalar"}
                    }, "step": 0}]
                },
                "fifos": {}
            },
            "Leaf": {
                "readable_name": "Leaf",
                "srcs": ["Leaf.cpp"], "include_dirs": [], "defines": [],
                "level": "lower",
                "synth": "hls",
                "ports": [
                    {"cat": "scalar", "name": "n", "type": "uint64_t", "width": 64}
                ]
            }
        }
    }"#,
    )
    .expect("parse");
    let out = flatten(&g).expect("flatten ok");
    let top = out.tasks.get("Top").expect("top survives");
    let leaf = &top.tasks["Leaf"][0];
    assert_eq!(
        leaf.args["n"].arg,
        crate::instance::ArgSource::Literal(crate::instance::WireValue {
            width: 64,
            value: 16
        }),
        "the invoke-site constant must reach the leaf, not dangle as a port name"
    );
}

/// Even an empty nested upper task must flatten without an error.
#[test]
fn flatten_accepts_nested_upper_without_error() {
    let json = r#"{
        "schema_version": 4,
        "cflags": [],
        "top": "Outer",
        "target": "xilinx-hls",
        "tasks": {
            "Outer": {
                "readable_name": "Outer",
                "srcs": ["Outer.cpp"], "include_dirs": [], "defines": [], "level": "upper", "synth": "hls",
                "ports": [],
                "tasks": {"Inner": [{"args": {}, "step": 0}]},
                "fifos": {}
            },
            "Inner": {
                "readable_name": "Inner",
                "srcs": ["Inner.cpp"], "include_dirs": [], "defines": [], "level": "upper", "synth": "hls",
                "ports": [],
                "tasks": {}, "fifos": {}
            }
        }
    }"#;
    let g = TaskGraph::from_json(json).expect("parse");
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
        "schema_version": 4,
        "cflags": [],
        "top": "Outer",
        "target": "xilinx-hls",
        "tasks": {
            "Outer": {
                "readable_name": "Outer",
                "srcs": ["Outer.cpp"], "include_dirs": [], "defines": [], "level": "upper", "synth": "hls",
                "ports": [
                    {"cat": "scalar", "name": "n", "type": "uint64_t", "width": 64}
                ],
                "tasks": {"Inner": [{"step": 0, "args": {
                    "p": {"arg": "n", "cat": "scalar"}
                }}]},
                "fifos": {}
            },
            "Inner": {
                "readable_name": "Inner",
                "srcs": ["Inner.cpp"], "include_dirs": [], "defines": [], "level": "upper", "synth": "hls",
                "ports": [
                    {"cat": "scalar", "name": "p", "type": "uint64_t", "width": 64}
                ],
                "tasks": {"Leaf": [{"step": 0, "args": {
                    "q": {"arg": "p", "cat": "scalar"}
                }}]},
                "fifos": {}
            },
            "Leaf": {
                "readable_name": "Leaf",
                "srcs": ["Leaf.cpp"], "include_dirs": [], "defines": [], "level": "lower", "synth": "hls",
                "ports": [
                    {"cat": "scalar", "name": "q", "type": "uint64_t", "width": 64}
                ]
            }
        }
    }"#;
    let g = TaskGraph::from_json(json).expect("parse");
    let projection = flatten_with_origins(&g).expect("recursive flatten");
    let top = projection.flat.tasks.get("Outer").expect("top");
    let leaf_insts = top.tasks.get("Leaf").expect("leaf hoisted under top");
    assert_eq!(leaf_insts.len(), 1);
    // The leaf's `q` arg must resolve to the outermost `n` binding
    // (promoted through `p` in Inner → `n` in Outer).
    let arg = leaf_insts[0].args.get("q").expect("q arg present");
    assert_eq!(
        arg.name(),
        Some("n"),
        "nested scalar arg must promote to Outer's external port"
    );
    // Control lowering must still see both original forwarding boundaries:
    // Outer.n -> Inner.p -> Leaf.q, rather than the substituted leaf binding.
    assert_eq!(
        projection
            .source_invocation(&g, OccurrenceId(1))
            .expect("original Inner invocation")
            .args["p"]
            .name(),
        Some("n")
    );
    assert_eq!(
        projection
            .source_invocation(&g, OccurrenceId(2))
            .expect("original Leaf invocation")
            .args["q"]
            .name(),
        Some("p")
    );
    assert_eq!(projection.occurrences[2].parent, Some(OccurrenceId(1)));
}

const NESTED_CLUSTERS_GRAPH_JSON: &str = r#"{
        "schema_version": 4,
        "cflags": [],
        "top": "Outer",
        "target": "xilinx-hls",
        "tasks": {
            "Outer": {
                "readable_name": "Outer",
                "srcs": ["Outer.cpp"], "include_dirs": [], "defines": [], "level": "upper", "synth": "hls",
                "ports": [],
                "tasks": {"Cluster": [
                    {"name": "west_cluster", "step": 0, "args": {}},
                    {"name": "east_cluster", "step": 0, "args": {}}
                ]},
                "fifos": {}
            },
            "Cluster": {
                "readable_name": "Cluster",
                "srcs": ["Cluster.cpp"], "include_dirs": [], "defines": [], "level": "upper", "synth": "hls",
                "ports": [],
                "tasks": {"compute_stage_west_cluster_Outer": [{
                    "name": "compute_stage", "step": 0, "args": {}
                }]},
                "fifos": {}
            },
            "compute_stage_west_cluster_Outer": {
                "readable_name": "compute_stage_west_cluster_Outer",
                "srcs": ["Stage.cpp"], "include_dirs": [], "defines": [], "level": "upper", "synth": "hls",
                "ports": [],
                "tasks": {
                    "Source": [{"name": "worker", "step": 0, "args": {
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
                "readable_name": "Source",
                "srcs": ["Source.cpp"], "include_dirs": [], "defines": [], "level": "lower", "synth": "hls",
                "ports": [{"cat": "ostream", "name": "out", "type": "int", "width": 32}]
            },
            "Sink": {
                "readable_name": "Sink",
                "srcs": ["Sink.cpp"], "include_dirs": [], "defines": [], "level": "lower", "synth": "hls",
                "ports": [{"cat": "istream", "name": "in", "type": "int", "width": 32}]
            }
        }
    }"#;

/// Reused upper definitions have separate original owners even though their
/// local child references are identical. Detach belongs to the immediate
/// invocation; it must not replace the joined children inside that scope.
#[test]
fn flat_projection_preserves_repeated_upper_control_ownership() {
    let mut graph = TaskGraph::from_json(NESTED_CLUSTERS_GRAPH_JSON).expect("parse");
    graph
        .tasks
        .get_mut("Outer")
        .expect("top")
        .tasks
        .get_mut("Cluster")
        .expect("clusters")[0]
        .step = -1;
    let projection = flatten_with_origins(&graph).expect("projection");
    assert_eq!(projection.occurrences[0].parent, None);
    assert_eq!(projection.occurrences[0].source, None);
    assert_eq!(projection.occurrences[0].global_name, "Outer");
    assert_eq!(projection.source_invocation(&graph, OccurrenceId(0)), None);

    for (index, cluster_name, step) in [(0, "west_cluster", -1), (1, "east_cluster", 0)] {
        let cluster = projection
            .occurrences
            .iter()
            .position(|occurrence| occurrence.source == Some(EndpointRef("Cluster".into(), index)))
            .map(OccurrenceId)
            .expect("cluster occurrence");
        assert_eq!(
            projection.occurrences[cluster.0].parent,
            Some(OccurrenceId(0))
        );
        assert_eq!(
            projection
                .source_invocation(&graph, cluster)
                .expect("cluster invocation")
                .step,
            step
        );
        let stage = projection
            .occurrences
            .iter()
            .position(|occurrence| occurrence.parent == Some(cluster))
            .map(OccurrenceId)
            .expect("stage occurrence");
        assert_eq!(
            projection.occurrences[cluster.0].global_name,
            format!("{cluster_name}_Outer")
        );
        assert_eq!(
            projection.occurrences[stage.0].source,
            Some(EndpointRef("compute_stage_west_cluster_Outer".into(), 0))
        );
        assert_eq!(
            projection
                .source_invocation(&graph, stage)
                .expect("stage invocation")
                .step,
            0
        );
        assert_eq!(
            projection
                .occurrences
                .iter()
                .filter(|occurrence| occurrence.parent == Some(stage))
                .count(),
            2,
            "both original leaf occurrences must survive"
        );
        for (id, leaf) in projection
            .occurrences
            .iter()
            .enumerate()
            .filter(|(_, occurrence)| occurrence.parent == Some(stage))
        {
            let source = leaf.source.as_ref().expect("original leaf endpoint");
            assert_eq!(source.1, 0, "original index belongs to the repeated stage");
            assert_eq!(
                leaf.flat_endpoint,
                Some(EndpointRef(source.0.clone(), index))
            );
            let invocation = projection
                .source_invocation(&graph, OccurrenceId(id))
                .expect("leaf invocation");
            assert_eq!(
                invocation.step, 0,
                "children still join their detached owner"
            );
            assert_eq!(
                invocation
                    .args
                    .values()
                    .next()
                    .expect("stream binding")
                    .name(),
                Some("q")
            );
            let expected_name = if source.0 == "Source" {
                format!("worker_compute_stage_{cluster_name}_Outer")
            } else {
                format!("Sink_{index}")
            };
            assert_eq!(leaf.global_name, expected_name);
        }
    }
}

#[test]
fn flatten_uses_instance_paths_for_nested_fifos_and_named_leaves() {
    // The nested definition equals its west instance's full scope path;
    // only graph.top identifies the root scope.
    let mut graph = TaskGraph::from_json(NESTED_CLUSTERS_GRAPH_JSON).expect("parse");
    let mut direct = graph.tasks["compute_stage_west_cluster_Outer"].tasks["Source"][0].clone();
    direct.args.get_mut("out").expect("output binding").arg =
        crate::ArgSource::Name("direct_out".into());
    let mut direct_port = graph.tasks["Source"]
        .port("out")
        .expect("output port")
        .clone();
    direct_port.name = "direct_out".into();
    let top = graph.tasks.get_mut("Outer").expect("top task");
    top.tasks.insert("Source".into(), vec![direct]);
    top.ports.push(direct_port);
    top.fifos.insert(
        "direct_out".into(),
        crate::InterconnectDefinition {
            depth: None,
            consumed_by: None,
            produced_by: Some(EndpointRef("Source".into(), 0)),
        },
    );

    let flattened = flatten(&graph).expect("flatten");
    let top = &flattened.tasks["Outer"];
    assert_eq!(
        top.tasks["Source"][0].name.as_deref(),
        Some("worker_compute_stage_west_cluster_Outer")
    );
    assert_eq!(
        top.tasks["Source"][1].name.as_deref(),
        Some("worker_compute_stage_east_cluster_Outer")
    );
    assert_eq!(top.tasks["Source"][2].name.as_deref(), Some("worker"));
    for instance in &top.tasks["Sink"] {
        assert_eq!(instance.name, None);
    }
    assert_eq!(
        top.fifos["direct_out"].produced_by,
        Some(EndpointRef("Source".to_owned(), 2))
    );

    let west_fifo = "q_compute_stage_west_cluster_Outer";
    let east_fifo = "q_compute_stage_east_cluster_Outer";
    assert_eq!(top.tasks["Source"][0].args["out"].name(), Some(west_fifo));
    assert_eq!(top.tasks["Sink"][0].args["in"].name(), Some(west_fifo));
    assert_eq!(top.tasks["Source"][1].args["out"].name(), Some(east_fifo));
    assert_eq!(top.tasks["Sink"][1].args["in"].name(), Some(east_fifo));

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
        "schema_version": 4,
        "cflags": [],
        "top": "Outer",
        "target": "xilinx-hls",
        "tasks": {
            "Outer": {
                "readable_name": "Outer",
                "srcs": ["Outer.cpp"], "include_dirs": [], "defines": [], "level": "upper", "synth": "hls",
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
                "readable_name": "Stage",
                "srcs": ["Stage.cpp"], "include_dirs": [], "defines": [], "level": "upper", "synth": "hls",
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
                "readable_name": "Leaf",
                "srcs": ["Leaf.cpp"], "include_dirs": [], "defines": [], "level": "lower", "synth": "hls",
                "ports": [
                    {"cat": "istream", "name": "pkt_in", "type": "uint64_t", "width": 64}
                ]
            }
        }
    }"#;
    let g = TaskGraph::from_json(json).expect("parse");
    let out = flatten(&g).expect("flatten");
    let top = out.tasks.get("Outer").expect("top survives");
    let leaf = &top.tasks["Leaf"][0];
    assert_eq!(
        leaf.args["pkt_in"].name(),
        Some("qs[7]_Outer"),
        "leaf indexed stream arg must resolve through Stage.in_q[3] -> qs[7]"
    );

    let fifo = top.fifos.get("qs[7]_Outer").expect("fifo renamed");
    assert_eq!(fifo.consumed_by, Some(EndpointRef("Leaf".to_string(), 0)));
}
