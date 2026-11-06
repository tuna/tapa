use super::*;

use serde_json::{json, Value};

fn leaf(ports: &Value, synth: &str) -> Value {
    json!({"level": "lower", "srcs": ["d.cpp"], "include_dirs": [], "defines": [],
           "readable_name": "", "synth": synth, "ports": ports})
}

fn stream(cat: &str, name: &str, width: u32) -> Value {
    json!({"cat": cat, "name": name, "type": "float", "width": width})
}

fn scalar(name: &str) -> Value {
    json!({"cat": "scalar", "name": name, "type": "int", "width": 32})
}

fn graph(tasks: Value) -> TaskGraph {
    let mut tasks = tasks;
    for (name, task) in tasks.as_object_mut().expect("tasks") {
        task["readable_name"] = json!(name);
        if task.get("srcs").is_none() {
            task["srcs"] = json!(["d.cpp"]);
            task["include_dirs"] = json!([]);
            task["defines"] = json!([]);
        }
    }
    serde_json::from_value(json!({"schema_version": 4, "top": "Top",
        "target": "xilinx-vitis", "tasks": tasks}))
    .expect("graph fixture")
}

fn arg(name: &str, cat: &str) -> Value {
    json!({"arg": name, "cat": cat})
}

/// A PL loader feeds the AI Engine task `add`; a PL storer drains it.
fn vadd(add: &Value, add_width: u32) -> TaskGraph {
    graph(json!({
        "Top": {"level": "upper", "synth": "hls", "ports": [scalar("n")],
            "tasks": {
                "Load": [{"args": {"out": arg("a_q", "ostream")}}],
                "Add": [{"args": {"a": arg("a_q", "istream"), "c": arg("c_q", "ostream"),
                                  "n": arg("n", "scalar")}}],
                "Store": [{"args": {"in": arg("c_q", "istream")}}]},
            "fifos": {
                "a_q": {"depth": 2, "produced_by": ["Load", 0], "consumed_by": ["Add", 0]},
                "c_q": {"depth": 2, "produced_by": ["Add", 0], "consumed_by": ["Store", 0]}}},
        "Load": leaf(&json!([stream("ostream", "out", add_width)]), "hls"),
        "Add": add.clone(),
        "Store": leaf(&json!([stream("istream", "in", add_width)]), "hls"),
    }))
}

#[test]
fn a_pl_only_graph_has_no_partition() {
    let g = vadd(&leaf(&json!([]), "hls"), 32);
    assert_eq!(AiePartition::derive(&g).expect("derive"), None);
}

#[test]
fn boundary_streams_become_plios_and_scalars_runtime_parameters() {
    let add = leaf(
        &json!([
            {"cat": "istream", "name": "a", "type": "vec4f", "width": 128, "frame": 64},
            stream("ostream", "c", 32),
            scalar("n"),
        ]),
        "aie",
    );
    let partition = AiePartition::derive(&vadd(&add, 32))
        .expect("derive")
        .expect("aie");
    assert_eq!(
        partition.streams,
        [
            AieStream {
                name: "a_q".into(),
                fifo: "a_q".into(),
                words: 4,
                frame: Some(64),
                kind: AieStreamKind::Plio {
                    input: true,
                    width: 128
                },
            },
            AieStream {
                name: "c_q".into(),
                fifo: "c_q".into(),
                words: 1,
                frame: None,
                kind: AieStreamKind::Plio {
                    input: false,
                    width: 32
                },
            },
        ]
    );
    let [kernel] = partition.kernels.as_slice() else {
        panic!("one kernel")
    };
    assert_eq!(
        (kernel.name.as_str(), kernel.call.as_str()),
        ("Add_0", "Add")
    );
    assert_eq!(
        kernel.ports.iter().map(|p| &p.kind).collect::<Vec<_>>(),
        [
            &AiePortKind::Streams {
                input: true,
                plural: false,
                streams: vec![0],
                words: 4,
                frame: Some(64)
            },
            &AiePortKind::Streams {
                input: false,
                plural: false,
                streams: vec![1],
                words: 1,
                frame: None
            },
            &AiePortKind::Scalar {
                ctype: "int".into(),
                width: 32,
                value: ScalarBinding::Runtime("n".into())
            },
        ]
    );
    assert!(!partition.run_forever);
}

fn upper_task_graph() -> TaskGraph {
    graph(json!({
        "Top": {"level": "upper", "synth": "hls", "ports": [scalar("n")],
            "tasks": {
                "Load": [{"args": {"out": arg("a_q", "ostream")}}],
                "Pipe": [{"step": -1, "args": {"a": arg("a_q", "istream"),
                                               "c": arg("c_q", "ostream")}}],
                "Store": [{"args": {"in": arg("c_q", "istream")}}]},
            "fifos": {
                "a_q": {"depth": 2, "produced_by": ["Load", 0], "consumed_by": ["Pipe", 0]},
                "c_q": {"depth": 2, "produced_by": ["Pipe", 0], "consumed_by": ["Store", 0]}}},
        "Load": leaf(&json!([stream("ostream", "out", 32)]), "hls"),
        "Store": leaf(&json!([stream("istream", "in", 32)]), "hls"),
        "Pipe": {"level": "upper", "synth": "aie",
            "ports": [stream("istream", "a", 32), stream("ostream", "c", 32)],
            "tasks": {"Inc": [
                {"args": {"in": arg("a", "istream"), "out": arg("mid", "ostream"),
                          "k": {"arg": {"width": 32, "value": 1}, "cat": "scalar"}}},
                {"args": {"in": arg("mid", "istream"), "out": arg("c", "ostream"),
                          "k": {"arg": {"width": 32, "value": 2}, "cat": "scalar"}}}]},
            "fifos": {"mid": {"depth": 8, "produced_by": ["Inc", 0],
                              "consumed_by": ["Inc", 1]}}},
        "Inc": leaf(&json!([stream("istream", "in", 64), stream("ostream", "out", 64),
                           scalar("k")]), "aie"),
    }))
}

#[test]
fn an_upper_task_flattens_into_kernels_and_nets() {
    let mut g = upper_task_graph();
    // A PL-only stream in the top scope may reuse the AIE-local name `mid`.
    let top = g.tasks.get_mut("Top").expect("top");
    top.tasks.get_mut("Load").expect("load").push(
        serde_json::from_value(json!({"args": {"out": arg("mid", "ostream")}}))
            .expect("load instance"),
    );
    top.tasks.get_mut("Store").expect("store").push(
        serde_json::from_value(json!({"args": {"in": arg("mid", "istream")}}))
            .expect("store instance"),
    );
    top.fifos.insert(
        "mid".into(),
        serde_json::from_value(json!({"depth": 2, "produced_by": ["Load", 1],
                                      "consumed_by": ["Store", 1]}))
        .expect("PL fifo"),
    );
    let partition = AiePartition::derive(&g).expect("derive").expect("aie");
    assert!(partition.run_forever, "the detached invoke runs forever");
    assert_eq!(
        partition
            .kernels
            .iter()
            .map(|k| k.name.as_str())
            .collect::<Vec<_>>(),
        ["Pipe_0__Inc_0", "Pipe_0__Inc_1"]
    );
    let net = partition
        .streams
        .iter()
        .find(|s| s.fifo == "mid")
        .expect("net");
    assert_eq!(net.name, "Pipe_0__mid");
    assert_eq!(
        net.kind,
        AieStreamKind::Net { depth_words: 16 },
        "8 elements of 2 words"
    );
    assert_eq!(
        partition.kernels[1].ports[2].kind,
        AiePortKind::Scalar {
            ctype: "int".into(),
            width: 32,
            value: ScalarBinding::Constant(WireValue {
                width: 32,
                value: 2
            })
        }
    );
    let AiePortKind::Streams { streams, .. } = &partition.kernels[1].ports[1].kind else {
        panic!("stream port")
    };
    assert_eq!(
        partition.streams[streams[0]].name, "c_q",
        "the upper's port binding"
    );
    let pl = partition.pl_projection(&g).expect("project");
    let mid = &pl.tasks["Top"].fifos["mid"];
    assert_eq!(mid.produced_by, Some(crate::EndpointRef("Load".into(), 1)));
    assert_eq!(mid.consumed_by, Some(crate::EndpointRef("Store".into(), 1)));
}

#[test]
fn an_upper_task_rejects_sequential_children() {
    let mut g = upper_task_graph();
    g.tasks
        .get_mut("Pipe")
        .expect("pipe")
        .tasks
        .get_mut("Inc")
        .expect("inc")[0]
        .step = 1;
    let err = AiePartition::derive(&g).expect_err("sequential AIE child");
    assert!(err.0.contains("invoked sequentially"), "{err}");
}

#[test]
fn unsupported_shapes_are_rejected_with_the_reason() {
    let wide = leaf(
        &json!([
            stream("istream", "a", 256),
            stream("ostream", "c", 256),
            scalar("n")
        ]),
        "aie",
    );
    let err = AiePartition::derive(&vadd(&wide, 256)).expect_err("256-bit boundary");
    assert!(err.0.contains("at most 128 bits"), "{err}");
}

/// `vadd` with its whole body moved into the PL upper task `Mid`.
fn vadd_below(step: i64) -> TaskGraph {
    let add = leaf(
        &json!([
            stream("istream", "a", 32),
            stream("ostream", "c", 32),
            scalar("n")
        ]),
        "aie",
    );
    let mut g = vadd(&add, 32);
    let mut mid = g.tasks.remove("Top").expect("top");
    mid.synth = SynthTarget::Hls;
    let mut top = mid.clone();
    top.tasks = BTreeMap::from([(
        "Mid".into(),
        vec![
            serde_json::from_value(json!({"step": step, "args": {"n": arg("n", "scalar")}}))
                .expect("Mid instance"),
        ],
    )]);
    top.fifos = BTreeMap::new();
    g.tasks.insert("Mid".into(), mid);
    g.tasks.insert("Top".into(), top);
    g
}

/// An AI Engine leaf below a PL upper task: the upper task mixes the
/// fabrics, so it is inlined and its FIFOs become PLIOs the top declares.
#[test]
fn an_upper_task_mixing_fabrics_is_inlined_into_its_parent() {
    let g = vadd_below(0);
    let partition = AiePartition::derive(&g).expect("derive").expect("aie");
    assert_eq!(
        partition
            .kernels
            .iter()
            .map(|k| k.name.as_str())
            .collect::<Vec<_>>(),
        ["Mid_0__Add_0"]
    );
    assert_eq!(
        partition
            .streams
            .iter()
            .map(|s| (s.name.as_str(), s.kind))
            .collect::<Vec<_>>(),
        [
            (
                "Mid_0__a_q",
                AieStreamKind::Plio {
                    input: true,
                    width: 32
                }
            ),
            (
                "Mid_0__c_q",
                AieStreamKind::Plio {
                    input: false,
                    width: 32
                }
            ),
        ]
    );
    let pl = partition.pl_projection(&g).expect("project");
    assert!(!pl.tasks.contains_key("Mid"), "the inlined task is gone");
    let top = &pl.tasks["Top"];
    assert_eq!(
        top.tasks.keys().map(String::as_str).collect::<Vec<_>>(),
        ["Load", "Store"]
    );
    assert_eq!(
        top.fifos["Mid_0__a_q"].produced_by,
        Some(crate::EndpointRef("Load".into(), 0))
    );
    assert_eq!(
        top.ports
            .iter()
            .filter_map(|p| p.aie_plio.map(|_| p.name.as_str()))
            .collect::<Vec<_>>(),
        ["Mid_0__a_q", "Mid_0__c_q"]
    );

    let err = AiePartition::derive(&vadd_below(1)).expect_err("sequential mixed upper");
    assert!(err.0.contains("invoked sequentially (step 1)"), "{err}");

    // A host program finds each inlined instance by where its parent
    // invokes it, so the positions must tell the parent's invokes apart.
    let mut twice = vadd_below(0);
    let mids = twice
        .tasks
        .get_mut("Top")
        .and_then(|top| top.tasks.get_mut("Mid"))
        .expect("Mid instances");
    mids.push(mids[0].clone());
    let err = crate::inlined_instances(&twice).expect_err("same position");
    assert!(err.0.contains("re-run `tapa analyze`"), "{err}");
    twice
        .tasks
        .get_mut("Top")
        .expect("top")
        .tasks
        .get_mut("Mid")
        .expect("Mid")[0]
        .position = 1;
    let found: Vec<_> = crate::inlined_instances(&twice)
        .expect("inlined")
        .into_iter()
        .map(|i| (i.parent, i.position, i.name))
        .collect();
    assert_eq!(
        found,
        [
            (String::new(), 1, "Mid_0".into()),
            (String::new(), 0, "Mid_1".into())
        ]
    );
}

/// A PL leaf inside an AI Engine upper task runs on the PL: the upper's tag
/// is only the default its leaves inherit.
#[test]
fn a_pl_leaf_in_an_aie_upper_task_runs_on_the_pl() {
    let g = graph(json!({
        "Top": {"level": "upper", "synth": "hls", "ports": [scalar("n")],
            "tasks": {
                "Load": [{"args": {"out": arg("a_q", "ostream")}}],
                "Pipe": [{"args": {"a": arg("a_q", "istream"), "c": arg("c_q", "ostream")}}],
                "Store": [{"args": {"in": arg("c_q", "istream")}}]},
            "fifos": {
                "a_q": {"depth": 2, "produced_by": ["Load", 0], "consumed_by": ["Pipe", 0]},
                "c_q": {"depth": 2, "produced_by": ["Pipe", 0], "consumed_by": ["Store", 0]}}},
        "Load": leaf(&json!([stream("ostream", "out", 32)]), "hls"),
        "Store": leaf(&json!([stream("istream", "in", 32)]), "hls"),
        "Pipe": {"level": "upper", "synth": "aie",
            "ports": [stream("istream", "a", 32), stream("ostream", "c", 32)],
            "tasks": {
                "Inc": [{"args": {"in": arg("a", "istream"), "out": arg("mid", "ostream")}}],
                "Dbl": [{"args": {"in": arg("mid", "istream"), "out": arg("c", "ostream")}}]},
            "fifos": {"mid": {"depth": 8, "produced_by": ["Inc", 0],
                              "consumed_by": ["Dbl", 0]}}},
        "Inc": leaf(&json!([stream("istream", "in", 32), stream("ostream", "out", 64)]), "aie"),
        "Dbl": leaf(&json!([stream("istream", "in", 64), stream("ostream", "out", 32)]), "hls"),
    }));
    let partition = AiePartition::derive(&g).expect("derive").expect("aie");
    assert_eq!(
        partition
            .kernels
            .iter()
            .map(|k| k.name.as_str())
            .collect::<Vec<_>>(),
        ["Pipe_0__Inc_0"]
    );
    assert_eq!(
        partition
            .streams
            .iter()
            .map(|s| (s.name.as_str(), s.kind))
            .collect::<Vec<_>>(),
        [
            (
                "a_q",
                AieStreamKind::Plio {
                    input: true,
                    width: 32
                }
            ),
            (
                "Pipe_0__mid",
                AieStreamKind::Plio {
                    input: false,
                    width: 64
                }
            ),
        ]
    );
    let pl = partition.pl_projection(&g).expect("project");
    let top = &pl.tasks["Top"];
    assert_eq!(
        top.tasks["Dbl"][0].name.as_deref(),
        Some("Pipe_0__Dbl_0"),
        "the PL leaf joins the PL kernel"
    );
    assert_eq!(
        top.fifos["c_q"].produced_by,
        Some(crate::EndpointRef("Dbl".into(), 0))
    );
}

#[test]
fn the_pl_projection_turns_plios_into_top_axis_ports() {
    let add = leaf(
        &json!([
            stream("istream", "a", 32),
            stream("ostream", "c", 32),
            scalar("n")
        ]),
        "aie",
    );
    let g = vadd(&add, 32);
    let partition = AiePartition::derive(&g).expect("derive").expect("aie");
    let pl = partition.pl_projection(&g).expect("project");
    assert!(!pl.tasks.contains_key("Add"), "the AI Engine task is gone");
    let top = &pl.tasks["Top"];
    assert!(!top.tasks.contains_key("Add"), "and so is its instance");
    let a_q = &top.fifos["a_q"];
    assert_eq!(
        (a_q.depth, a_q.consumed_by.as_ref()),
        (None, None),
        "an external FIFO"
    );
    assert_eq!(a_q.produced_by, Some(crate::EndpointRef("Load".into(), 0)));
    let ports: Vec<_> = top
        .ports
        .iter()
        .map(|p| (p.name.as_str(), p.cat, p.width, p.aie_plio))
        .collect();
    assert_eq!(
        ports,
        [
            ("n", ArgCategory::Scalar, 32, None),
            ("a_q", ArgCategory::Ostream, 32, Some(32)),
            ("c_q", ArgCategory::Istream, 32, Some(32)),
        ],
        "the top writes what the AI Engine reads, and reads what it writes"
    );
}

/// A hybrid top left with only stream ports packages as `ap_ctrl_none`, a
/// free-running kernel XRT can neither start nor wait on; the projection
/// rejects it, while the same shape with no AI Engine task stays a legal
/// PL-only stream top (`derive` finds no partition to project).
#[test]
fn a_hybrid_stream_only_top_is_rejected_for_lacking_kernel_control() {
    let stream_only = |add_synth: &str| {
        let add = leaf(
            &json!([
                stream("istream", "a", 32),
                stream("ostream", "c", 32),
                scalar("n"),
            ]),
            add_synth,
        );
        let mut g = vadd(&add, 32);
        let top = g.tasks.get_mut("Top").expect("top");
        top.ports.clear();
        top.tasks.get_mut("Add").expect("add")[0].args.insert(
            "n".into(),
            serde_json::from_value(json!({"arg": {"width": 32, "value": 64}, "cat": "scalar"}))
                .expect("constant"),
        );
        g
    };

    let g = stream_only("aie");
    let partition = AiePartition::derive(&g).expect("derive").expect("aie");
    let err = partition.pl_projection(&g).expect_err("stream-only top");
    assert!(err.0.contains("the top task `Top`"), "{err}");
    assert!(err.0.contains("ap_ctrl_hs"), "{err}");

    assert_eq!(
        AiePartition::derive(&stream_only("hls")).expect("derive"),
        None,
        "a PL-only stream top is unaffected"
    );
}

/// Only a kernel a stream cycle runs through keeps its stream accesses in
/// program order; a pipeline stays free to overlap them.
#[test]
fn kernels_on_a_stream_cycle_are_ordered() {
    let add = leaf(
        &json!([stream("istream", "a", 32), stream("ostream", "c", 32)]),
        "aie",
    );
    let chain = AiePartition::derive(&vadd(&add, 32))
        .expect("derive")
        .expect("aie");
    assert!(!chain.kernels[0].ordered);

    // `Add` writes `c_q` to the PL task `Echo`, which writes `b_q` back.
    let through_pl = graph(json!({
        "Top": {"level": "upper", "synth": "hls", "ports": [],
            "tasks": {
                "Load": [{"args": {"out": arg("a_q", "ostream")}}],
                "Add": [{"args": {"a": arg("a_q", "istream"), "b": arg("b_q", "istream"),
                                  "c": arg("c_q", "ostream")}}],
                "Echo": [{"args": {"in": arg("c_q", "istream"), "out": arg("b_q", "ostream")}}]},
            "fifos": {
                "a_q": {"depth": 2, "produced_by": ["Load", 0], "consumed_by": ["Add", 0]},
                "b_q": {"depth": 2, "produced_by": ["Echo", 0], "consumed_by": ["Add", 0]},
                "c_q": {"depth": 2, "produced_by": ["Add", 0], "consumed_by": ["Echo", 0]}}},
        "Load": leaf(&json!([stream("ostream", "out", 32)]), "hls"),
        "Add": leaf(&json!([stream("istream", "a", 32), stream("istream", "b", 32),
                            stream("ostream", "c", 32)]), "aie"),
        "Echo": leaf(&json!([stream("istream", "in", 32), stream("ostream", "out", 32)]), "hls"),
    }));
    let partition = AiePartition::derive(&through_pl)
        .expect("derive")
        .expect("aie");
    assert!(partition.kernels[0].ordered);

    // Two kernels exchanging over nets, as cannon's PEs do.
    let ring = graph(json!({
        "Top": {"level": "upper", "synth": "hls", "ports": [],
            "tasks": {"Pe": [
                {"args": {"out": arg("x", "ostream"), "in": arg("y", "istream")}},
                {"args": {"out": arg("y", "ostream"), "in": arg("x", "istream")}}]},
            "fifos": {
                "x": {"depth": 2, "produced_by": ["Pe", 0], "consumed_by": ["Pe", 1]},
                "y": {"depth": 2, "produced_by": ["Pe", 1], "consumed_by": ["Pe", 0]}}},
        "Pe": leaf(&json!([stream("ostream", "out", 32), stream("istream", "in", 32)]), "aie"),
    }));
    let partition = AiePartition::derive(&ring).expect("derive").expect("aie");
    assert!(partition.kernels.iter().all(|kernel| kernel.ordered));
}
