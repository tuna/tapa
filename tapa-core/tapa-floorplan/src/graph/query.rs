//! `FloorGraph`'s inherent methods: the read-only accessors over a built
//! graph, the placement-result materialization, and the constructors. The
//! `FloorGraphBuilder` construction machinery lives in the sibling `build`
//! module and shared validation in `validate`; the methods stay in one impl
//! block here so the rustdoc/public-api view of the type is unchanged by the
//! split.

use std::collections::BTreeMap;

use tapa_ir::TaskGraph;

use super::build::FloorGraphBuilder;
use super::validate::{occupied_rtl_names, validate_sanitized_instance_names};
use crate::graph::floor_graph::{
    AxiNet, ControlInterface, ControlNet, FloorGraph, GraphError, MemoryInterface, PlacementEdge,
    Stream, Vertex,
};

impl FloorGraph {
    /// All placeable task-rooted clusters, in creation order.
    #[must_use]
    pub fn vertices(&self) -> &[Vertex] {
        &self.vertices
    }

    /// Width-summed, unordered task-pair edges used by placement.
    #[must_use]
    pub fn placement_edges(&self) -> &[PlacementEdge] {
        &self.placement_edges
    }

    /// Directed, named FIFO streams used by routing and code generation.
    #[must_use]
    pub fn streams(&self) -> &[Stream] {
        &self.streams
    }

    /// Directed external-memory channels used by routing and code generation.
    #[must_use]
    pub fn axi_nets(&self) -> &[AxiNet] {
        &self.axi_nets
    }

    /// Directed distributed-control channels used by routing and codegen.
    #[must_use]
    pub fn control_nets(&self) -> &[ControlNet] {
        &self.control_nets
    }

    /// The vertex at `index`.
    #[must_use]
    pub fn vertex(&self, index: usize) -> &Vertex {
        &self.vertices[index]
    }

    /// The index of the placeable task cluster named `name`.
    ///
    /// Co-located FIFO aliases are deliberately absent: placement constraints
    /// target their host task rather than creating a second placement degree
    /// of freedom.
    #[must_use]
    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.index.get(name).copied()
    }

    /// Add the real FIFO/relay instance names to an atomic placement.
    ///
    /// FIFO area is already included in the host vertex, so this only expands
    /// the name-to-region view consumed by XDC/codegen; it does not alter slot
    /// utilization.
    pub(crate) fn materialize_co_locations(
        &self,
        regions: &mut BTreeMap<String, String>,
    ) -> Result<(), GraphError> {
        for instance in &self.co_located {
            let host = &self.vertices[instance.host].name;
            let region =
                regions
                    .get(host)
                    .cloned()
                    .ok_or_else(|| GraphError::MissingHostRegion {
                        instance: instance.name.clone(),
                        host: host.clone(),
                    })?;
            // Build-time name validation should make this unreachable, but
            // never silently overwrite another object's placement.
            if let Some(existing) = regions.get(&instance.name) {
                return Err(GraphError::NameConflict {
                    name: instance.name.clone(),
                    first: format!("the instance already placed in `{existing}`"),
                    second: format!("co-located alias of host `{host}`"),
                });
            }
            regions.insert(instance.name.clone(), region);
        }
        Ok(())
    }

    /// Remove transient external terminals before publishing placement data.
    pub(crate) fn remove_transient_regions(&self, regions: &mut BTreeMap<String, String>) {
        regions.retain(|name, _| {
            self.index
                .get(name)
                .is_none_or(|&vertex| self.vertices[vertex].materialize)
        });
    }
    /// Build the placement graph from an already-*flattened* graph (every leaf
    /// instance directly under the top task).
    pub fn build(flat: &TaskGraph) -> Result<Self, GraphError> {
        Self::build_with_memory(flat, &[])
    }

    /// Build the placement graph with exact direct-M_AXI bank endpoints.
    pub fn build_with_memory(
        flat: &TaskGraph,
        memory: &[MemoryInterface],
    ) -> Result<Self, GraphError> {
        Self::build_with_interfaces(flat, memory, None, None)
    }

    /// Build the placement graph with optional distributed-control metadata.
    ///
    /// Orchestration only — build-graph → cluster → interfaces → finish. The
    /// step order fixes vertex/edge insertion order, which feeds the
    /// canonical placement-model fingerprint; do not re-order.
    pub(crate) fn build_with_interfaces(
        flat: &TaskGraph,
        memory: &[MemoryInterface],
        control: Option<ControlInterface>,
        global_anchor: Option<&str>,
    ) -> Result<Self, GraphError> {
        let top = flat
            .tasks
            .get(&flat.top)
            .ok_or_else(|| GraphError::MissingTop(flat.top.clone()))?;

        let mut builder = FloorGraphBuilder::default();
        builder.add_task_vertices(flat, top)?;

        // Distinct logical instances that collapse to one RTL identifier
        // cannot both be constrained, with or without distributed control.
        validate_sanitized_instance_names(top)?;

        let streams = builder.cluster_internal_fifos(flat, top)?;

        let axi_nets = builder.add_memory_interfaces(flat, top, memory)?;

        let control_nets = if let Some(control) = control.filter(|_| !top.tasks.is_empty()) {
            builder.add_control_interface(flat, top, memory, control, global_anchor)?
        } else {
            Vec::new()
        };

        let built = builder.finish();

        // Every public result key — task canonical names, FIFO names and
        // their `{name}_fifo` RTL instances, co-located aliases — must be
        // unique both literally and after RTL sanitization, independently of
        // optional memory/control interfaces.
        occupied_rtl_names(top, &built.vertices, &built.co_located)?;

        Ok(Self {
            vertices: built.vertices,
            placement_edges: built.placement_edges,
            streams,
            axi_nets,
            control_nets,
            index: built.index,
            co_located: built.co_located,
        })
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::graph::floor_graph::CONTROL_S_AXI_INSTANCE;
    use tapa_ir::{
        async_mmap_bridge_instance_name, global_controller_instance_name,
        local_controller_instance_name, Area, AxiChannel, AxiChannelWidths, AxiEndpoint,
        ControlChannel, MemoryBank,
    };

    /// A two-leaf `A -> fifo -> B` design, mirroring the flatten test graph.
    pub fn vadd_graph() -> TaskGraph {
        let json = r#"{
            "cflags": [],
            "top": "VecAdd",
            "target": "xilinx-hls",
            "tasks": {
                "VecAdd": {
                    "readable_name": "VecAdd",
                    "code": "void VecAdd() {}",
                    "level": "upper",
                    "synth": "hls",
                    "ports": [],
                    "tasks": {
                        "A": [{"args": {"out": {"arg": "fifo", "cat": "ostream"}}, "step": 0}],
                        "B": [{"args": {"in": {"arg": "fifo", "cat": "istream"}}, "step": 0}]
                    },
                    "fifos": {
                        "fifo": {"depth": 2, "consumed_by": ["B", 0], "produced_by": ["A", 0]}
                    }
                },
                "A": {
                    "readable_name": "A", "code": "void A() {}", "level": "lower", "synth": "hls",
                    "ports": [{"cat": "ostream", "name": "out", "type": "float", "width": 32}],
                    "self_area": {"lut": 100, "ff": 200}
                },
                "B": {
                    "readable_name": "B", "code": "void B() {}", "level": "lower", "synth": "hls",
                    "ports": [{"cat": "istream", "name": "in", "type": "float", "width": 32}],
                    "self_area": {"lut": 50, "ff": 60}
                }
            }
        }"#;
        tapa_ir::TaskGraph::from_json(json).expect("parse vadd graph")
    }

    pub fn mmap_graph() -> TaskGraph {
        TaskGraph::from_json(
            r#"{
                "cflags": [], "top": "Top", "target": "xilinx-hls",
                "tasks": {
                    "Top": {
                        "readable_name": "Top", "code": "", "level": "upper", "synth": "hls",
                        "ports": [{"cat":"mmap","name":"mem","type":"int*","width":32}],
                        "tasks": {"Reader": [{"args":{"data":{"arg":"mem","cat":"mmap"}},"step":0}]},
                        "fifos": {}
                    },
                    "Reader": {
                        "readable_name": "Reader", "code": "", "level": "lower", "synth": "hls",
                        "ports": [{"cat":"mmap","name":"data","type":"int*","width":32}],
                        "self_area": {"lut":10,"ff":20}
                    }
                }
            }"#,
        )
        .expect("parse mmap graph")
    }

    pub fn mmap_interface() -> MemoryInterface {
        MemoryInterface {
            endpoint: AxiEndpoint {
                instance: "Reader_0".to_string(),
                port: "data".to_string(),
                top_port: "mem".to_string(),
            },
            bank: MemoryBank {
                kind: tapa_ir::MemoryKind::Hbm,
                index: 0,
            },
            channel_widths: AxiChannelWidths {
                read_address: 80,
                read_data: 38,
                write_address: 80,
                write_data: 39,
                write_response: 5,
            },
            bridge_instance: None,
        }
    }

    pub fn async_mmap_graph(instance_name: &str) -> TaskGraph {
        let mut value = serde_json::to_value(mmap_graph()).expect("serialize mmap graph");
        value["tasks"]["Top"]["tasks"]["Reader"][0]["name"] = serde_json::json!(instance_name);
        value["tasks"]["Top"]["tasks"]["Reader"][0]["args"]["data"]["cat"] =
            serde_json::json!("async_mmap");
        value["tasks"]["Reader"]["ports"][0]["cat"] = serde_json::json!("async_mmap");
        TaskGraph::from_json(&value.to_string()).expect("parse async mmap graph")
    }

    pub fn async_mmap_interface(instance_name: &str, read: bool, write: bool) -> MemoryInterface {
        MemoryInterface {
            endpoint: AxiEndpoint {
                instance: instance_name.to_string(),
                port: "data".to_string(),
                top_port: "mem".to_string(),
            },
            bank: MemoryBank {
                kind: tapa_ir::MemoryKind::Hbm,
                index: 0,
            },
            channel_widths: AxiChannelWidths {
                read_address: if read { 80 } else { 0 },
                read_data: if read { 38 } else { 0 },
                write_address: if write { 80 } else { 0 },
                write_data: if write { 39 } else { 0 },
                write_response: if write { 5 } else { 0 },
            },
            bridge_instance: Some(async_mmap_bridge_instance_name("mem")),
        }
    }

    pub fn distributed_control_graph() -> TaskGraph {
        TaskGraph::from_json(
            r#"{
                "cflags": [], "top": "Top", "target": "xilinx-vitis",
                "tasks": {
                    "Top": {
                        "readable_name": "Top", "code": "", "level": "upper", "synth": "hls",
                        "ports": [
                            {"cat":"scalar","name":"n","type":"unsigned","width":32},
                            {"cat":"scalar","name":"mode","type":"char","width":8},
                            {"cat":"mmap","name":"mem","type":"int*","width":32}
                        ],
                        "tasks": {
                            "Worker": [{"name":"worker#0","args":{
                                "count":{"arg":"n","cat":"scalar"},
                                "data":{"arg":"mem","cat":"mmap"}
                            },"step":0}],
                            "Ticker": [{"name":"ticker[1]","args":{
                                "mode":{"arg":"mode","cat":"scalar"}
                            },"step":-1}]
                        },
                        "fifos": {}
                    },
                    "Worker": {
                        "readable_name":"Worker","code":"","level":"lower","synth":"hls",
                        "ports":[
                            {"cat":"scalar","name":"count","type":"unsigned","width":32},
                            {"cat":"mmap","name":"data","type":"int*","width":32}
                        ]
                    },
                    "Ticker": {
                        "readable_name":"Ticker","code":"","level":"lower","synth":"hls",
                        "ports":[{"cat":"scalar","name":"mode","type":"char","width":8}]
                    }
                }
            }"#,
        )
        .expect("parse control graph")
    }

    pub fn control_memory_interface() -> MemoryInterface {
        MemoryInterface {
            endpoint: AxiEndpoint {
                instance: "worker#0".to_string(),
                port: "data".to_string(),
                top_port: "mem".to_string(),
            },
            bank: MemoryBank {
                kind: tapa_ir::MemoryKind::Hbm,
                index: 0,
            },
            channel_widths: AxiChannelWidths {
                read_address: 80,
                read_data: 38,
                write_address: 80,
                write_data: 39,
                write_response: 5,
            },
            bridge_instance: None,
        }
    }

    #[test]
    fn distributed_control_tracks_exact_widths_and_autorun_inventory() {
        let flat = tapa_ir::flatten(&distributed_control_graph()).expect("flatten");
        let graph = FloorGraph::build_with_interfaces(
            &flat,
            &[control_memory_interface()],
            Some(ControlInterface {
                has_s_axi_control: true,
            }),
            Some("S_AXI_CONTROL"),
        )
        .expect("build control graph");

        let global = graph
            .index_of(global_controller_instance_name())
            .expect("global controller");
        assert_eq!(graph.vertex(global).area, Area::default());
        assert_eq!(
            graph.vertex(global).required_tag.as_deref(),
            Some("S_AXI_CONTROL")
        );
        let worker = graph.index_of("worker#0").expect("worker");
        let ticker = graph.index_of("ticker[1]").expect("ticker");
        let channels = graph
            .control_nets()
            .iter()
            .map(|net| {
                (
                    net.instance.as_str(),
                    net.channel,
                    net.src,
                    net.dst,
                    net.width,
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            channels,
            [
                ("ticker[1]", ControlChannel::Launch, global, ticker, 9),
                ("ticker[1]", ControlChannel::Reset, global, ticker, 1),
                ("worker#0", ControlChannel::Launch, global, worker, 98),
                ("worker#0", ControlChannel::Reset, global, worker, 1),
                ("worker#0", ControlChannel::Completion, worker, global, 1),
            ]
        );

        let mut regions = BTreeMap::from([
            (graph.vertex(global).name.clone(), "SLOT_X1Y1".to_string()),
            (graph.vertex(worker).name.clone(), "SLOT_X0Y0".to_string()),
            (graph.vertex(ticker).name.clone(), "SLOT_X0Y1".to_string()),
        ]);
        graph
            .materialize_co_locations(&mut regions)
            .expect("materialize controllers");
        assert_eq!(
            regions[CONTROL_S_AXI_INSTANCE],
            regions[&graph.vertex(global).name]
        );
        assert_eq!(
            regions[&local_controller_instance_name("worker#0")],
            regions["worker#0"]
        );
        assert_eq!(
            regions[&local_controller_instance_name("ticker[1]")],
            regions["ticker[1]"]
        );

        let worker_edge = graph
            .placement_edges()
            .iter()
            .find(|edge| (edge.src, edge.dst) == (global.min(worker), global.max(worker)))
            .expect("worker control edge");
        assert_eq!(worker_edge.width, 100, "98 launch + reset + completion");
        let ticker_edge = graph
            .placement_edges()
            .iter()
            .find(|edge| (edge.src, edge.dst) == (global.min(ticker), global.max(ticker)))
            .expect("ticker control edge");
        assert_eq!(ticker_edge.width, 10, "9 launch + reset, no completion");
    }

    #[test]
    fn disabled_control_keeps_the_baseline_graph_unchanged() {
        let flat = tapa_ir::flatten(&vadd_graph()).expect("flatten");
        let baseline = FloorGraph::build(&flat).expect("baseline");
        let explicit = FloorGraph::build_with_interfaces(&flat, &[], None, None)
            .expect("explicit disabled graph");
        assert_eq!(baseline.vertices, explicit.vertices);
        assert_eq!(baseline.placement_edges, explicit.placement_edges);
        assert_eq!(baseline.streams, explicit.streams);
        assert_eq!(baseline.axi_nets, explicit.axi_nets);
        assert!(baseline.control_nets().is_empty());
        assert!(explicit.control_nets().is_empty());
    }

    #[test]
    fn enabled_control_is_a_noop_without_child_instances() {
        let design = TaskGraph::from_json(
            r#"{
                "cflags": [], "top": "Leaf", "target": "xilinx-hls",
                "tasks": {"Leaf": {
                    "readable_name":"Leaf","code":"","level":"lower","synth":"hls",
                    "ports":[],"tasks":{},"fifos":{}
                }}
            }"#,
        )
        .expect("parse leaf top");
        let graph = FloorGraph::build_with_interfaces(
            &design,
            &[],
            Some(ControlInterface {
                has_s_axi_control: true,
            }),
            Some("S_AXI_CONTROL"),
        )
        .expect("empty control topology");
        assert!(graph.vertices().is_empty());
        assert!(graph.control_nets().is_empty());
        assert!(graph.index_of(global_controller_instance_name()).is_none());
    }

    #[test]
    fn build_clusters_fifo_at_consumer_and_routes_the_logical_stream() {
        let flat = tapa_ir::flatten(&vadd_graph()).expect("flatten");
        let graph = FloorGraph::build(&flat).expect("build floor graph");

        // FIFO storage is part of the destination cluster, not an independently
        // placeable waypoint.
        assert_eq!(graph.vertices().len(), 2, "only A and B are placeable");
        let a = graph.index_of("A_0").expect("A_0 vertex");
        let b = graph.index_of("B_0").expect("B_0 vertex");
        assert!(graph.index_of("fifo_VecAdd").is_none());

        assert_eq!(
            graph.vertex(a).area,
            Area {
                lut: 100,
                ff: 200,
                ..Area::default()
            }
        );
        assert_eq!(
            graph.vertex(b).area,
            Area {
                // 50/60 task area + 66/13 for registered-ready storage.
                lut: 116,
                ff: 73,
                ..Area::default()
            }
        );

        // Placement sees one physical stream bundle: 32 payload, eot,
        // write/valid, and full_n/ready. Routing retains its FIFO name and
        // direction.
        let placement_edges = graph.placement_edges();
        assert_eq!(placement_edges.len(), 1);
        assert_eq!(
            (
                placement_edges[0].src,
                placement_edges[0].dst,
                placement_edges[0].width
            ),
            (a.min(b), a.max(b), 35)
        );
        let streams = graph.streams();
        assert_eq!(streams.len(), 1);
        assert_eq!(streams[0].link, "fifo_VecAdd");
        assert_eq!(
            (streams[0].src, streams[0].dst, streams[0].width),
            (a, b, 35)
        );

        // Result/XDC still names and locates the actual FIFO/relay at the
        // destination that owns its area.
        let mut regions = BTreeMap::from([
            ("A_0".to_string(), "SLOT_X0Y0".to_string()),
            ("B_0".to_string(), "SLOT_X1Y0".to_string()),
        ]);
        graph
            .materialize_co_locations(&mut regions)
            .expect("materialize FIFO placement");
        assert_eq!(regions["fifo_VecAdd"], "SLOT_X1Y0");
    }

    #[test]
    fn placement_aggregates_parallel_streams_without_losing_routing_records() {
        let design = TaskGraph::from_json(
            r#"{
                "cflags": [], "top": "Top", "target": "xilinx-hls",
                "tasks": {
                    "Top": {
                        "readable_name": "Top", "code": "void Top() {}",
                        "level": "upper", "synth": "hls", "ports": [],
                        "tasks": {
                            "A": [{"args": {
                                "out32": {"arg": "q32", "cat": "ostream"},
                                "out64": {"arg": "q64", "cat": "ostream"},
                                "in8": {"arg": "reply", "cat": "istream"}
                            }, "step": 0}],
                            "B": [{"args": {
                                "in32": {"arg": "q32", "cat": "istream"},
                                "in64": {"arg": "q64", "cat": "istream"},
                                "out8": {"arg": "reply", "cat": "ostream"}
                            }, "step": 0}]
                        },
                        "fifos": {
                            "q32": {"depth": 2, "produced_by": ["A", 0], "consumed_by": ["B", 0]},
                            "q64": {"depth": 2, "produced_by": ["A", 0], "consumed_by": ["B", 0]},
                            "reply": {"depth": 2, "produced_by": ["B", 0], "consumed_by": ["A", 0]}
                        }
                    },
                    "A": {
                        "readable_name": "A", "code": "void A() {}", "level": "lower", "synth": "hls",
                        "ports": [
                            {"cat": "ostream", "name": "out32", "type": "int", "width": 32},
                            {"cat": "ostream", "name": "out64", "type": "long", "width": 64},
                            {"cat": "istream", "name": "in8", "type": "char", "width": 8}
                        ]
                    },
                    "B": {
                        "readable_name": "B", "code": "void B() {}", "level": "lower", "synth": "hls",
                        "ports": [
                            {"cat": "istream", "name": "in32", "type": "int", "width": 32},
                            {"cat": "istream", "name": "in64", "type": "long", "width": 64},
                            {"cat": "ostream", "name": "out8", "type": "char", "width": 8}
                        ]
                    }
                }
            }"#,
        )
        .expect("parse parallel graph");
        let flat = tapa_ir::flatten(&design).expect("flatten parallel graph");
        let graph = FloorGraph::build(&flat).expect("build parallel graph");
        let a = graph.index_of("A_0").expect("A");
        let b = graph.index_of("B_0").expect("B");

        assert_eq!(graph.streams().len(), 3);
        assert!(graph.streams().iter().any(|stream| {
            stream.link == "reply_Top" && (stream.src, stream.dst, stream.width) == (b, a, 11)
        }));
        assert_eq!(
            graph.placement_edges(),
            [PlacementEdge {
                src: a.min(b),
                dst: a.max(b),
                width: 35 + 67 + 11,
            }]
        );
    }

    #[test]
    fn exact_memory_terminal_adds_five_directed_channels_but_is_not_published() {
        let flat = tapa_ir::flatten(&mmap_graph()).expect("flatten mmap graph");
        let graph = FloorGraph::build_with_memory(&flat, &[mmap_interface()]).expect("build");
        let task = graph.index_of("Reader_0").expect("reader");
        let bank = graph.index_of("__tapa_bank_hbm_0").expect("bank");

        assert_eq!(graph.vertices().len(), 2, "one task plus one terminal");
        assert_eq!(graph.axi_nets().len(), 5);
        for net in graph.axi_nets() {
            match net.channel {
                AxiChannel::ReadAddress | AxiChannel::WriteAddress | AxiChannel::WriteData => {
                    assert_eq!((net.src, net.dst), (task, bank));
                }
                AxiChannel::ReadData | AxiChannel::WriteResponse => {
                    assert_eq!((net.src, net.dst), (bank, task));
                }
            }
            assert_eq!(net.payload_width + 2, net.width);
        }
        assert_eq!(
            graph.placement_edges(),
            [PlacementEdge {
                src: task.min(bank),
                dst: task.max(bank),
                width: 80 + 38 + 80 + 39 + 5,
            }]
        );

        let mut regions = BTreeMap::from([
            ("Reader_0".to_string(), "SLOT_X1Y1".to_string()),
            ("__tapa_bank_hbm_0".to_string(), "SLOT_X0Y0".to_string()),
        ]);
        graph.remove_transient_regions(&mut regions);
        assert_eq!(
            regions,
            BTreeMap::from([("Reader_0".to_string(), "SLOT_X1Y1".to_string())])
        );
    }

    #[test]
    fn async_mmap_routes_only_enabled_group_and_co_locates_bridge() {
        for (read, write, expected_channels, expected_width) in
            [(true, false, 2, 80 + 38), (false, true, 3, 80 + 39 + 5)]
        {
            let flat = tapa_ir::flatten(&async_mmap_graph("Reader_0")).expect("flatten");
            let graph = FloorGraph::build_with_memory(
                &flat,
                &[async_mmap_interface("Reader_0", read, write)],
            )
            .expect("build async mmap graph");
            let task = graph.index_of("Reader_0").expect("reader");
            let bank = graph.index_of("__tapa_bank_hbm_0").expect("bank");

            assert_eq!(graph.axi_nets().len(), expected_channels);
            assert_eq!(graph.placement_edges()[0].width, expected_width);
            assert!(graph.axi_nets().iter().all(|net| match net.channel {
                AxiChannel::ReadAddress | AxiChannel::ReadData => read,
                AxiChannel::WriteAddress | AxiChannel::WriteData | AxiChannel::WriteResponse =>
                    write,
            }));

            let mut regions = BTreeMap::from([
                ("Reader_0".to_string(), "SLOT_X1Y1".to_string()),
                ("__tapa_bank_hbm_0".to_string(), "SLOT_X0Y0".to_string()),
            ]);
            graph
                .materialize_co_locations(&mut regions)
                .expect("materialize bridge placement");
            assert_eq!(regions["mem__m_axi"], regions["Reader_0"]);
            graph.remove_transient_regions(&mut regions);
            assert!(!regions.contains_key("__tapa_bank_hbm_0"));
            assert_eq!(regions.len(), 2, "task and bridge remain public");
            assert_eq!((task, bank), (0, 1));
        }
    }

    #[test]
    fn unused_async_mmap_has_bridge_but_no_terminal_or_nets() {
        let flat = tapa_ir::flatten(&async_mmap_graph("Reader_0")).expect("flatten");
        let graph =
            FloorGraph::build_with_memory(&flat, &[async_mmap_interface("Reader_0", false, false)])
                .expect("unused async mmap remains legal");

        assert_eq!(graph.vertices().len(), 1);
        assert!(graph.index_of("__tapa_bank_hbm_0").is_none());
        assert!(graph.axi_nets().is_empty());
        assert!(graph.placement_edges().is_empty());
        let mut regions = BTreeMap::from([("Reader_0".to_string(), "SLOT_X1Y1".to_string())]);
        graph
            .materialize_co_locations(&mut regions)
            .expect("materialize unused bridge");
        assert_eq!(regions["mem__m_axi"], "SLOT_X1Y1");
    }
}
