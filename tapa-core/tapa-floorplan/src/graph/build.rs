//! `FloorGraph` construction: task clustering, FIFO co-location, and the
//! optional memory/control interface expansion.

use std::collections::{BTreeMap, HashMap};

use tapa_ir::{
    axi_pipeline_instance_name, floorplanned_fifo_storage_depth, global_controller_instance_name,
    local_controller_instance_name, Area, AxiChannel, AxiEndpoint, ControlChannel, MemoryBank,
    TaskGraph,
};

use crate::graph::floor_graph::{
    fifo_area, AxiNet, CoLocatedInstance, ControlInterface, ControlNet, GraphError,
    MemoryInterface, PlacementEdge, Stream, Vertex, CONTROL_S_AXI_INSTANCE,
};

use super::validate::{
    expected_memory_interfaces, occupied_rtl_names, reserve_generated_name, validate_control_names,
    validate_memory_interface_shape,
};

/// The widths flowing each way across one endpoint pair, accumulated while the
/// graph is built and summed into a [`PlacementEdge`] at the end.
#[derive(Debug, Clone, Copy, Default)]
struct DirectedWidth {
    /// Low-index endpoint to high-index endpoint.
    forward: u32,
    /// High-index endpoint to low-index endpoint.
    reverse: u32,
}

/// Transient accumulator for one `FloorGraph` construction pass.
///
/// Owns the collections the task-vertex, FIFO-clustering, and
/// interface-expansion passes all append to, so each pass is a `&mut self`
/// method rather than a many-argument free function threading the same
/// accumulators. [`FloorGraphBuilder::finish`] yields the pieces `FloorGraph`
/// assembles from.
#[derive(Default)]
pub(super) struct FloorGraphBuilder {
    vertices: Vec<Vertex>,
    index: HashMap<String, usize>,
    /// (definition name, instance index) → vertex index, for endpoint resolution.
    task_endpoints: HashMap<(String, u32), usize>,
    /// `(low, high)` endpoint pair → the widths flowing low→high and high→low.
    placement_widths: BTreeMap<(usize, usize), DirectedWidth>,
    co_located: Vec<CoLocatedInstance>,
}

/// The accumulated graph pieces a finished [`FloorGraphBuilder`] yields.
///
/// Vertex/edge insertion order — which feeds the canonical placement-model
/// fingerprint — is fixed by the time `finish` runs; `finish` itself only
/// collapses the ordered `placement_widths` map into the placement edge list.
pub(super) struct BuiltGraph {
    pub(super) vertices: Vec<Vertex>,
    pub(super) index: HashMap<String, usize>,
    pub(super) placement_edges: Vec<PlacementEdge>,
    pub(super) co_located: Vec<CoLocatedInstance>,
}

impl FloorGraphBuilder {
    /// Insert one placement vertex per task instance.
    ///
    /// Duplicate canonical names are rejected: they would silently collapse
    /// onto one key in the published `regions` map. Also records the
    /// `(definition, instance index)` → vertex endpoint map used by the FIFO
    /// clustering and control passes.
    pub(super) fn add_task_vertices(
        &mut self,
        flat: &TaskGraph,
        top: &tapa_ir::Task,
    ) -> Result<(), GraphError> {
        for (def_name, instances) in &top.tasks {
            let def = flat
                .tasks
                .get(def_name)
                .ok_or_else(|| GraphError::MissingTaskDef(def_name.clone()))?;
            // One policy for a missing annotation, everywhere: no area means
            // no resources. `synth: ignore` tasks (custom RTL) never get an
            // HLS estimate, and refusing to plan them would be worse than
            // planning them as weightless. A value that is present but not a
            // count is rejected when the graph is parsed, not here.
            let area = def.self_area.unwrap_or_default();
            for (idx, inst) in instances.iter().enumerate() {
                let name = inst.canonical_name(def_name, idx).into_owned();
                let vertex_index = self.vertices.len();
                // Duplicate canonical names silently collapse onto one key in
                // the published `regions` map; always reject them, with or
                // without distributed control.
                if self.index.contains_key(&name) {
                    return Err(GraphError::DuplicateCanonicalName(name));
                }
                self.index.insert(name.clone(), vertex_index);
                let inst_idx = u32::try_from(idx).expect("instance count fits u32");
                self.task_endpoints
                    .insert((def_name.clone(), inst_idx), vertex_index);
                self.vertices.push(Vertex {
                    name,
                    area,
                    required_tag: None,
                    materialize: true,
                });
            }
        }
        Ok(())
    }

    /// Cluster each internal FIFO into its consumer-side host.
    ///
    /// The consumer-side Tail hosts each FIFO (producer fallback for a
    /// one-sided stream), and task endpoints connect directly. This keeps
    /// placement and routing on one logical topology. External passthrough
    /// FIFOs (no depth) are skipped, and every FIFO storage area is charged
    /// to its host vertex. Returns the directed logical streams in
    /// `top.fifos` order.
    pub(super) fn cluster_internal_fifos(
        &mut self,
        flat: &TaskGraph,
        top: &tapa_ir::Task,
    ) -> Result<Vec<Stream>, GraphError> {
        let mut streams = Vec::new();
        let fifo_widths = index_fifo_arg_widths(flat, top);
        for (fifo_name, fifo) in &top.fifos {
            let Some(depth) = fifo.depth else {
                continue; // external passthrough FIFO — not a placed instance
            };
            let data_width = resolve_fifo_data_width(&fifo_widths, fifo_name)?;
            let physical_width = data_width
                .checked_add(2) // valid/write and ready/full_n
                .ok_or_else(|| GraphError::UnresolvedFifoWidth(fifo_name.clone()))?;

            // A present endpoint reference must resolve; `None` alone marks
            // a deliberately one-sided (e.g. external) stream.
            let src = resolve_fifo_endpoint(
                &self.task_endpoints,
                fifo_name,
                "producer",
                fifo.produced_by.as_ref(),
            )?;
            let dst = resolve_fifo_endpoint(
                &self.task_endpoints,
                fifo_name,
                "consumer",
                fifo.consumed_by.as_ref(),
            )?;
            let host = dst
                .or(src)
                .ok_or_else(|| GraphError::UnanchoredFifo(fifo_name.clone()))?;

            self.vertices[host].area = self.vertices[host]
                .area
                .checked_add(fifo_area(data_width, floorplanned_fifo_storage_depth(depth)))
                .ok_or_else(|| GraphError::ResourceOverflow(fifo_name.clone()))?;
            self.co_located.push(CoLocatedInstance {
                name: fifo_name.clone(),
                host,
            });

            if let (Some(src), Some(dst)) = (src, dst) {
                if src != dst {
                    streams.push(Stream {
                        link: fifo_name.clone(),
                        src,
                        dst,
                        width: physical_width,
                        data_width,
                        depth,
                    });
                    self.add_placement_width(src, dst, physical_width)
                        .ok_or_else(|| GraphError::PlacementWidthOverflow(fifo_name.clone()))?;
                }
            }
        }
        Ok(streams)
    }

    /// Expand exact memory interfaces into bank terminals and directed AXI nets.
    #[allow(
        clippy::too_many_lines,
        reason = "memory graph construction validates and expands every interface in one pass"
    )]
    pub(super) fn add_memory_interfaces(
        &mut self,
        flat: &TaskGraph,
        top: &tapa_ir::Task,
        memory: &[MemoryInterface],
    ) -> Result<Vec<AxiNet>, GraphError> {
        let expected = expected_memory_interfaces(flat, top, &self.task_endpoints)?;
        let mut provided = BTreeMap::<AxiEndpoint, &MemoryInterface>::new();
        for interface in memory {
            if provided
                .insert(interface.endpoint.clone(), interface)
                .is_some()
            {
                return Err(GraphError::DuplicateMemoryInterface {
                    instance: interface.endpoint.instance.clone(),
                    port: interface.endpoint.port.clone(),
                });
            }
        }

        for endpoint in expected.keys() {
            if !provided.contains_key(endpoint) {
                return Err(GraphError::MissingMemoryInterface {
                    instance: endpoint.instance.clone(),
                    port: endpoint.port.clone(),
                    top_port: endpoint.top_port.clone(),
                });
            }
        }
        for endpoint in provided.keys() {
            if !expected.contains_key(endpoint) {
                return Err(GraphError::UnknownMemoryInterface {
                    instance: endpoint.instance.clone(),
                    port: endpoint.port.clone(),
                    top_port: endpoint.top_port.clone(),
                });
            }
        }

        let mut occupied = occupied_rtl_names(top, &self.vertices, &self.co_located)?;
        for (endpoint, expected_endpoint) in &expected {
            let interface = provided[endpoint];
            validate_memory_interface_shape(interface, *expected_endpoint)?;
            for (channel, _) in interface.channel_widths.enabled_channels() {
                reserve_generated_name(
                    &mut occupied,
                    axi_pipeline_instance_name(endpoint, channel),
                    &format!("{channel:?} AXI pipeline for `{}`", endpoint.top_port),
                )?;
            }
            if let Some(bridge) = &interface.bridge_instance {
                reserve_generated_name(
                    &mut occupied,
                    bridge.clone(),
                    &format!("async mmap bridge for `{}`", endpoint.top_port),
                )?;
                // The bridge is generated above the HLS leaf, and current task
                // metadata has no standalone post-synthesis bridge area to charge
                // without inventing an estimate. Co-location still makes the
                // leaf-to-bridge FIFO wires local and constrains the true AXI
                // route source.
                self.co_located.push(CoLocatedInstance {
                    name: bridge.clone(),
                    host: expected_endpoint.task_vertex,
                });
            }
        }

        let mut terminals = BTreeMap::<MemoryBank, usize>::new();
        let mut nets = Vec::with_capacity(memory.len().saturating_mul(5));
        for (endpoint, expected_endpoint) in &expected {
            let interface = provided[endpoint];
            let task_vertex = expected_endpoint.task_vertex;
            let enabled_channels = interface
                .channel_widths
                .enabled_channels()
                .collect::<Vec<_>>();
            if enabled_channels.is_empty() {
                continue;
            }
            let terminal = if let Some(&terminal) = terminals.get(&interface.bank) {
                terminal
            } else {
                let name = bank_terminal_name(interface.bank);
                if self.index.contains_key(&name) {
                    return Err(GraphError::DuplicateVertex(name));
                }
                let terminal = self.vertices.len();
                self.vertices.push(Vertex {
                    name: name.clone(),
                    area: Area::default(),
                    required_tag: Some(interface.bank.to_string()),
                    materialize: false,
                });
                self.index.insert(name, terminal);
                terminals.insert(interface.bank, terminal);
                terminal
            };

            for (channel, width) in enabled_channels {
                let Some(payload_width) = width.checked_sub(2).filter(|width| *width > 0) else {
                    return Err(GraphError::InvalidAxiWidth {
                        instance: endpoint.instance.clone(),
                        port: endpoint.port.clone(),
                        channel,
                        width,
                    });
                };
                let (src, dst) = match channel {
                    AxiChannel::ReadAddress | AxiChannel::WriteAddress | AxiChannel::WriteData => {
                        (task_vertex, terminal)
                    }
                    AxiChannel::ReadData | AxiChannel::WriteResponse => (terminal, task_vertex),
                };
                self.add_placement_width(src, dst, width).ok_or_else(|| {
                    GraphError::PlacementWidthOverflow(format!(
                        "{}.{} {channel:?}",
                        endpoint.instance, endpoint.port
                    ))
                })?;
                nets.push(AxiNet {
                    endpoint: endpoint.clone(),
                    bank: interface.bank,
                    channel,
                    src,
                    dst,
                    width,
                    payload_width,
                });
            }
        }
        Ok(nets)
    }

    /// Expand distributed control into the global controller vertex and control nets.
    pub(super) fn add_control_interface(
        &mut self,
        flat: &TaskGraph,
        top: &tapa_ir::Task,
        memory: &[MemoryInterface],
        control: ControlInterface,
        global_anchor: Option<&str>,
    ) -> Result<Vec<ControlNet>, GraphError> {
        validate_control_names(top, memory, control, &self.vertices, &self.co_located)?;

        let global_name = global_controller_instance_name().to_string();
        if self.index.contains_key(&global_name) {
            return Err(GraphError::GeneratedNameCollision {
                generated: global_name,
                existing: "placement vertex".to_string(),
            });
        }
        let global = self.vertices.len();
        self.vertices.push(Vertex {
            name: global_name.clone(),
            // Generated control logic is charged zero area: these blocks are
            // generated after leaf HLS synthesis, so no leaf's self_area covers
            // them and there is no post-synthesis model to charge without
            // inventing numbers. The gap is deliberate and small — a handshake
            // FSM per controller against ~200k LUTs per slot — and bounded:
            // the routed control-pipeline registers, the dominant added logic,
            // ARE accounted in realize_slot_usage, and the usage-limit envelope
            // absorbs the rest. Same trade-off as async-mmap bridges.
            area: Area::default(),
            required_tag: global_anchor.map(ToString::to_string),
            materialize: true,
        });
        self.index.insert(global_name, global);
        if control.has_s_axi_control {
            self.co_located.push(CoLocatedInstance {
                name: CONTROL_S_AXI_INSTANCE.to_string(),
                host: global,
            });
        }

        let mut nets = Vec::new();
        for (definition, instances) in &top.tasks {
            let task = flat
                .tasks
                .get(definition)
                .ok_or_else(|| GraphError::MissingTaskDef(definition.clone()))?;
            for (instance_index, instance) in instances.iter().enumerate() {
                let canonical = instance
                    .canonical_name(definition, instance_index)
                    .into_owned();
                let endpoint_index =
                    u32::try_from(instance_index).expect("instance count fits u32");
                let child = self.task_endpoints[&(definition.clone(), endpoint_index)];
                self.co_located.push(CoLocatedInstance {
                    name: local_controller_instance_name(&canonical),
                    host: child,
                });

                let launch_width = control_launch_width(task, instance, &canonical)?;
                self.add_control_net(
                    &mut nets,
                    &canonical,
                    ControlChannel::Launch,
                    global,
                    child,
                    launch_width,
                )?;
                self.add_control_net(
                    &mut nets,
                    &canonical,
                    ControlChannel::Reset,
                    global,
                    child,
                    1,
                )?;
                if instance.step >= 0 {
                    self.add_control_net(
                        &mut nets,
                        &canonical,
                        ControlChannel::Completion,
                        child,
                        global,
                        1,
                    )?;
                }
            }
        }
        Ok(nets)
    }

    /// Yield the accumulated graph pieces for `FloorGraph` assembly.
    pub(super) fn finish(self) -> BuiltGraph {
        let placement_edges = self
            .placement_widths
            .into_iter()
            .map(|((src, dst), widths)| PlacementEdge {
                src,
                dst,
                width: widths.forward + widths.reverse,
                forward_width: widths.forward,
                reverse_width: widths.reverse,
            })
            .collect();
        BuiltGraph {
            vertices: self.vertices,
            index: self.index,
            placement_edges,
            co_located: self.co_located,
        }
    }

    /// Accumulate one directed channel's width onto its endpoint pair.
    ///
    /// `None` on overflow, which the caller names.
    fn add_placement_width(&mut self, src: usize, dst: usize, width: u32) -> Option<()> {
        let entry = self
            .placement_widths
            .entry((src.min(dst), src.max(dst)))
            .or_default();
        let directed = if src < dst {
            &mut entry.forward
        } else {
            &mut entry.reverse
        };
        *directed = directed.checked_add(width)?;
        Some(())
    }

    /// Append one control net, accumulating its placement-crossing width.
    fn add_control_net(
        &mut self,
        nets: &mut Vec<ControlNet>,
        instance: &str,
        channel: ControlChannel,
        src: usize,
        dst: usize,
        width: u32,
    ) -> Result<(), GraphError> {
        self.add_placement_width(src, dst, width)
            .ok_or_else(|| GraphError::ControlWidthOverflow {
                instance: instance.to_string(),
                channel,
            })?;
        nets.push(ControlNet {
            instance: instance.to_string(),
            channel,
            src,
            dst,
            width,
        });
        Ok(())
    }
}

fn control_launch_width(
    task: &tapa_ir::Task,
    instance: &tapa_ir::TaskInstance,
    instance_name: &str,
) -> Result<u32, GraphError> {
    let mut width = if instance.step < 0 { 1_u32 } else { 2_u32 };

    for port in task.ports.iter().filter(|port| port.cat.is_scalar()) {
        let argument = instance
            .args
            .get(&port.name)
            .ok_or_else(|| GraphError::ScalarMetadata {
                instance: instance_name.to_string(),
                port: port.name.clone(),
                detail: "child scalar port has no instance argument".to_string(),
            })?;
        if !argument.cat.is_scalar() {
            return Err(GraphError::ScalarMetadata {
                instance: instance_name.to_string(),
                port: port.name.clone(),
                detail: format!(
                    "child port is scalar but its instance argument is {}",
                    argument.cat.as_str()
                ),
            });
        }
        if port.width == 0 {
            return Err(GraphError::ScalarMetadata {
                instance: instance_name.to_string(),
                port: port.name.clone(),
                detail: "scalar width must be greater than zero".to_string(),
            });
        }
        width = width
            .checked_add(port.width)
            .ok_or_else(|| GraphError::ControlWidthOverflow {
                instance: instance_name.to_string(),
                channel: ControlChannel::Launch,
            })?;
    }

    for (port_name, argument) in &instance.args {
        if argument.cat.is_scalar()
            && task
                .ports
                .iter()
                .find(|port| port.name == *port_name)
                .is_none_or(|port| !port.cat.is_scalar())
        {
            return Err(GraphError::ScalarMetadata {
                instance: instance_name.to_string(),
                port: port_name.clone(),
                detail: "instance argument is scalar but child scalar port metadata is missing"
                    .to_string(),
            });
        }
        if argument.cat.is_direct_mmap() {
            width = width
                .checked_add(64)
                .ok_or_else(|| GraphError::ControlWidthOverflow {
                    instance: instance_name.to_string(),
                    channel: ControlChannel::Launch,
                })?;
        }
    }
    Ok(width)
}

fn bank_terminal_name(bank: MemoryBank) -> String {
    format!(
        "__tapa_bank_{}_{index}",
        bank_kind_name(bank),
        index = bank.index
    )
}

fn bank_kind_name(bank: MemoryBank) -> &'static str {
    match bank.kind {
        tapa_ir::MemoryKind::Hbm => "hbm",
        tapa_ir::MemoryKind::Ddr => "ddr",
    }
}

/// Resolve the FIFO storage width (payload + eot) from either endpoint.
/// One FIFO's endpoint port widths gathered from the bound instance args.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct FifoArgWidths {
    producer: Option<u32>,
    consumer: Option<u32>,
}

/// Gather every FIFO's producer/consumer port widths in one pass over the
/// bound args. The producer is the authoritative side: codegen sizes the FIFO
/// RTL from the producer's `_dout` port.
pub(super) fn index_fifo_arg_widths(
    flat: &TaskGraph,
    top: &tapa_ir::Task,
) -> BTreeMap<String, FifoArgWidths> {
    let mut index = BTreeMap::<String, FifoArgWidths>::new();
    for (def_name, instances) in &top.tasks {
        for inst in instances {
            for (port_name, arg) in &inst.args {
                if !arg.cat.is_stream() {
                    continue;
                }
                let Some(width) = port_width(flat, def_name, port_name) else {
                    continue;
                };
                // Streams always bind to a named FIFO, never to a constant.
                let Some(fifo) = arg.name() else {
                    continue;
                };
                let entry = index.entry(fifo.to_owned()).or_default();
                let side = if arg.cat.is_output_stream() {
                    &mut entry.producer
                } else {
                    &mut entry.consumer
                };
                // A well-formed graph binds one instance per side; the first
                // binding wins deterministically if malformed.
                if side.is_none() {
                    *side = Some(width);
                }
            }
        }
    }
    index
}

/// Resolve one declared FIFO endpoint to its task vertex. `None` marks a
/// deliberately one-sided stream; a present but unresolvable reference is
/// malformed IR and fails rather than silently dropping the connection.
pub(super) fn resolve_fifo_endpoint(
    endpoints: &HashMap<(String, u32), usize>,
    fifo_name: &str,
    role: &'static str,
    endpoint: Option<&tapa_ir::EndpointRef>,
) -> Result<Option<usize>, GraphError> {
    endpoint
        .map(|reference| {
            let key = (reference.0.clone(), reference.1);
            endpoints
                .get(&key)
                .copied()
                .ok_or_else(|| GraphError::DanglingFifoEndpoint {
                    fifo: fifo_name.to_string(),
                    role,
                    definition: reference.0.clone(),
                    index: reference.1,
                })
        })
        .transpose()
}

/// The FIFO storage width (payload + 1 eot bit, matching
/// `tapa_protocol::stream_data_wire_width`), resolved from the producer's
/// port and cross-checked against the consumer's when both are bound.
pub(super) fn resolve_fifo_data_width(
    index: &BTreeMap<String, FifoArgWidths>,
    fifo_name: &str,
) -> Result<u32, GraphError> {
    let widths = index.get(fifo_name).copied().unwrap_or_default();
    let payload = match (widths.producer, widths.consumer) {
        (Some(producer), Some(consumer)) => {
            if producer != consumer {
                return Err(GraphError::FifoWidthMismatch {
                    fifo: fifo_name.to_string(),
                    producer,
                    consumer,
                });
            }
            producer
        }
        (Some(producer), None) => producer,
        (None, Some(consumer)) => consumer,
        (None, None) => return Err(GraphError::UnresolvedFifoWidth(fifo_name.to_string())),
    };
    payload
        .checked_add(1)
        .ok_or_else(|| GraphError::UnresolvedFifoWidth(fifo_name.to_string()))
}

/// The bit width of `port_name` on task `def_name`.
///
/// For a `tapa::istreams`/`ostreams`/`mmaps` argument the instance names one
/// channel (e.g. `fifo_B_in[2]`) but the task definition declares a single
/// base port (`fifo_B_in`, `cat = istreams`). Fall back to the base name by
/// stripping any trailing `[N]` index when the exact channel name is absent.
fn port_width(flat: &TaskGraph, def_name: &str, port_name: &str) -> Option<u32> {
    let ports = &flat.tasks.get(def_name)?.ports;
    find_port(ports, port_name).map(|p| p.width)
}

/// Find a port by name, trying the exact channel name first then the base
/// (index-stripped) name for array-channel ports.
pub(super) fn find_port<'a>(ports: &'a [tapa_ir::Port], name: &str) -> Option<&'a tapa_ir::Port> {
    ports.iter().find(|p| p.name == name).or_else(|| {
        let base = name.split('[').next().unwrap_or(name);
        if base.is_empty() || base == name {
            None
        } else {
            ports.iter().find(|p| p.name == base)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::floor_graph::FloorGraph;
    use crate::graph::query::tests::vadd_graph;

    #[test]
    fn producer_consumer_width_mismatch_fails_closed() {
        let json = r#"{
            "cflags": [], "top": "Top", "target": "xilinx-hls",
            "tasks": {
                "Top": {
                    "readable_name": "Top", "code": "void Top() {}", "level": "upper", "synth": "hls",
                    "ports": [],
                    "tasks": {
                        "A": [{"args": {"out": {"arg": "q", "cat": "ostream"}}, "step": 0}],
                        "B": [{"args": {"in": {"arg": "q", "cat": "istream"}}, "step": 0}]
                    },
                    "fifos": {"q": {"depth": 2, "consumed_by": ["B", 0], "produced_by": ["A", 0]}}
                },
                "A": {"readable_name": "A", "code": "void A() {}", "level": "lower", "synth": "hls",
                    "ports": [{"cat": "ostream", "name": "out", "type": "uint32", "width": 32}]},
                "B": {"readable_name": "B", "code": "void B() {}", "level": "lower", "synth": "hls",
                    "ports": [{"cat": "istream", "name": "in", "type": "uint64", "width": 64}]}
            }
        }"#;
        let design = TaskGraph::from_json(json).expect("parse");
        let flat = tapa_ir::flatten(&design).expect("flatten");
        let error = FloorGraph::build(&flat).expect_err("32 vs 64 must fail");
        assert!(
            matches!(
                error,
                GraphError::FifoWidthMismatch {
                    producer: 32,
                    consumer: 64,
                    ..
                }
            ),
            "got {error}"
        );
    }

    #[test]
    fn dangling_fifo_endpoints_fail_closed() {
        let flat = tapa_ir::flatten(&vadd_graph()).expect("flatten");
        let top = flat.top.clone();
        let fifo_name = flat.tasks[&top].fifos.keys().next().expect("fifo").clone();

        // A producer reference to an instance index that does not exist.
        let mut broken = flat.clone();
        broken
            .tasks
            .get_mut(&top)
            .expect("top")
            .fifos
            .get_mut(&fifo_name)
            .expect("fifo")
            .produced_by = Some(tapa_ir::EndpointRef("A".to_string(), 7));
        let error = FloorGraph::build(&broken).expect_err("unknown producer instance");
        assert!(
            matches!(
                error,
                GraphError::DanglingFifoEndpoint {
                    role: "producer",
                    index: 7,
                    ..
                }
            ),
            "got {error}"
        );

        // A consumer reference to a definition that does not exist.
        let mut broken = flat;
        broken
            .tasks
            .get_mut(&top)
            .expect("top")
            .fifos
            .get_mut(&fifo_name)
            .expect("fifo")
            .consumed_by = Some(tapa_ir::EndpointRef("NotATask".to_string(), 0));
        let error = FloorGraph::build(&broken).expect_err("unknown consumer definition");
        assert!(
            matches!(
                error,
                GraphError::DanglingFifoEndpoint {
                    role: "consumer",
                    ..
                } if error.to_string().contains("NotATask")
            ),
            "got {error}"
        );
    }
}
