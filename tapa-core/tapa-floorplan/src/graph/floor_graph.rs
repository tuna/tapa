//! The [`FloorGraph`]: the placement graph the floorplan ILP runs on, built
//! from a *flattened* [`TaskGraph`].
//!
//! Vertices are clusters rooted at flattened leaf-task instances.  Each
//! internal FIFO is clustered into its consumer (producer fallback), because
//! the pipelined stream's storage is implemented at its destination.  The
//! FIFO's area is charged to that cluster and its name is retained as a
//! co-location alias for the final XDC. Placement aggregates stream widths by
//! task pair, while routing retains each named stream independently.

use std::collections::{BTreeMap, HashMap};

use tapa_ir::port::{sanitize_array_name, sanitize_identifier_name};
use tapa_ir::{
    async_mmap_bridge_instance_name, axi_pipeline_instance_name, control_pipeline_instance_name,
    floorplanned_fifo_storage_depth, global_controller_instance_name,
    local_controller_instance_name, Area, AxiChannel, AxiChannelWidths, AxiEndpoint,
    ControlChannel, MemoryBank, TaskGraph,
};

const CONTROL_S_AXI_INSTANCE: &str = "control_s_axi_U";

/// One supported direct M-AXI endpoint and its exact external bank.
///
/// This is transient planner input derived from parsed RTL plus the link
/// configuration. It is never stored in the work-state graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryInterface {
    pub endpoint: AxiEndpoint,
    pub bank: MemoryBank,
    pub channel_widths: AxiChannelWidths,
    /// Stable generated bridge hierarchy for FIFO-style async mmap. Direct
    /// compact M-AXI children have no bridge.
    pub bridge_instance: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ExpectedMemoryEndpoint {
    task_vertex: usize,
    child_category: tapa_ir::ArgCategory,
}

/// Opt-in metadata for the transient distributed-control planning graph.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ControlInterface {
    /// The generated kernel contains the exact top-level S-AXI control block.
    pub has_s_axi_control: bool,
}

/// One placeable instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vertex {
    /// Canonical instance name — the key it will carry in
    /// [`FloorplanResult::regions`](tapa_ir::FloorplanResult::regions).
    pub name: String,
    /// Resource footprint, including any destination-side FIFO storage
    /// clustered into this task.
    pub area: Area,
    /// Exact device tag required by a fixed external terminal. Ordinary RTL
    /// instances have no tag restriction.
    pub required_tag: Option<String>,
    /// Whether this vertex names real generated RTL and belongs in the public
    /// placement result. External terminals are transient solver vertices.
    pub materialize: bool,
}

/// One undirected connection used by the placement ILP.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlacementEdge {
    /// Lower-index endpoint.
    pub src: usize,
    /// Higher-index endpoint.
    pub dst: usize,
    /// Sum of the physical widths of all streams between the endpoints.
    pub width: u32,
}

/// One directed, named stream retained for routing and code generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stream {
    /// Internal FIFO/interconnect name used by codegen.
    pub link: String,
    /// Source vertex index.
    pub src: usize,
    /// Destination vertex index.
    pub dst: usize,
    /// Physical stream width: payload + eot + forward and reverse handshake.
    pub width: u32,
    /// FIFO storage width: payload + eot.
    pub(crate) data_width: u32,
    /// Requested FIFO depth before pipeline in-flight capacity is added.
    pub(crate) depth: u32,
}

/// One directed AXI channel retained for routing and code generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AxiNet {
    pub endpoint: AxiEndpoint,
    pub bank: MemoryBank,
    pub channel: AxiChannel,
    pub src: usize,
    pub dst: usize,
    /// Payload plus `VALID` and `READY`.
    pub width: u32,
    /// Concatenated payload width passed to the handshake pipeline.
    pub payload_width: u32,
}

/// One directed control channel retained for routing and code generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlNet {
    /// Canonical flattened child instance name.
    pub instance: String,
    pub channel: ControlChannel,
    pub src: usize,
    pub dst: usize,
    pub width: u32,
}

/// A real RTL instance whose placement aliases a host vertex. Any resource
/// cost represented by the planner has already been charged to that host.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CoLocatedInstance {
    name: String,
    host: usize,
}

/// The transient graph view shared by placement and stream routing.
#[derive(Debug, Clone)]
pub struct FloorGraph {
    vertices: Vec<Vertex>,
    placement_edges: Vec<PlacementEdge>,
    streams: Vec<Stream>,
    axi_nets: Vec<AxiNet>,
    control_nets: Vec<ControlNet>,
    index: HashMap<String, usize>,
    co_located: Vec<CoLocatedInstance>,
}

/// Why a [`FloorGraph`] could not be built from a graph.
#[derive(Debug, thiserror::Error)]
pub enum GraphError {
    /// `graph.top` is not present in `graph.tasks`.
    #[error("flattened graph is missing its top task `{0}`")]
    MissingTop(String),
    /// An instance names a task definition that is absent.
    #[error("task definition `{0}` referenced by an instance is missing")]
    MissingTaskDef(String),
    /// A FIFO's wire width could not be resolved from either endpoint's port.
    #[error("could not resolve the wire width of stream `{0}`")]
    UnresolvedFifoWidth(String),
    /// An internal FIFO has no placeable task endpoint to own its storage.
    #[error("internal stream `{0}` has no placeable endpoint")]
    UnanchoredFifo(String),
    /// Clustering a FIFO made a resource counter overflow.
    #[error("resource accounting overflow while clustering stream `{0}`")]
    ResourceOverflow(String),
    /// Summing parallel stream widths exceeded the supported counter width.
    #[error("physical width overflow while aggregating stream `{0}`")]
    PlacementWidthOverflow(String),
    /// A solver result omitted the host of a co-located RTL instance.
    #[error("floorplan result is missing host vertex `{host}` for `{instance}`")]
    MissingHostRegion { instance: String, host: String },
    /// A direct mmap endpoint has no exact RTL/connectivity planner input.
    #[error(
        "direct M-AXI endpoint `{instance}.{port}` (top port `{top_port}`) has no memory binding"
    )]
    MissingMemoryInterface {
        instance: String,
        port: String,
        top_port: String,
    },
    /// Planner input names an endpoint not present in the flattened design.
    #[error(
        "memory binding names unknown direct M-AXI endpoint `{instance}.{port}` (top port `{top_port}`)"
    )]
    UnknownMemoryInterface {
        instance: String,
        port: String,
        top_port: String,
    },
    /// The same endpoint appeared more than once in planner input.
    #[error("direct M-AXI endpoint `{instance}.{port}` is bound more than once")]
    DuplicateMemoryInterface { instance: String, port: String },
    /// Shared or hierarchical memory infrastructure is not modeled.
    #[error("unsupported external memory connection `{port}`: {detail}")]
    UnsupportedMemoryInterface { port: String, detail: String },
    /// A routed channel must contain payload plus valid/ready.
    #[error("invalid {channel:?} width {width} for `{instance}.{port}`; expected at least 3 bits")]
    InvalidAxiWidth {
        instance: String,
        port: String,
        channel: AxiChannel,
        width: u32,
    },
    /// A generated transient name collided with real RTL.
    #[error("duplicate placement vertex name `{0}`")]
    DuplicateVertex(String),
    /// Two child instances resolve to the same canonical logical name.
    #[error("duplicate flattened instance canonical name `{0}`")]
    DuplicateCanonicalName(String),
    /// Distinct logical instance names collapse to the same RTL identifier.
    #[error(
        "flattened instances `{first}` and `{second}` both sanitize to RTL name `{sanitized}`"
    )]
    SanitizedNameCollision {
        first: String,
        second: String,
        sanitized: String,
    },
    /// A generated bridge/controller/pipeline name aliases existing RTL.
    #[error("generated RTL name `{generated}` collides with {existing}")]
    GeneratedNameCollision { generated: String, existing: String },
    /// A child scalar argument does not agree with its task-port metadata.
    #[error("invalid scalar metadata for `{instance}.{port}`: {detail}")]
    ScalarMetadata {
        instance: String,
        port: String,
        detail: String,
    },
    /// A generated control bundle exceeds the supported physical width.
    #[error("physical width overflow while building {channel:?} control for `{instance}`")]
    ControlWidthOverflow {
        instance: String,
        channel: ControlChannel,
    },
}

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
    #[allow(
        clippy::too_many_lines,
        reason = "one pass builds the task, stream, memory, and optional control graph in order"
    )]
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

        let mut vertices = Vec::new();
        let mut index = HashMap::new();
        // (definition name, instance index) → vertex index, to resolve
        // FIFO endpoints to the vertices they connect.
        let mut endpoints: HashMap<(String, u32), usize> = HashMap::new();

        // Task-instance vertices.
        for (def_name, instances) in &top.tasks {
            let def = flat
                .tasks
                .get(def_name)
                .ok_or_else(|| GraphError::MissingTaskDef(def_name.clone()))?;
            let area = Area::from_annotations(&def.self_area);
            for (idx, inst) in instances.iter().enumerate() {
                let name = inst.canonical_name(def_name, idx).into_owned();
                let vertex_index = vertices.len();
                if control.is_some() && index.contains_key(&name) {
                    return Err(GraphError::DuplicateCanonicalName(name));
                }
                index.insert(name.clone(), vertex_index);
                let inst_idx = u32::try_from(idx).expect("instance count fits u32");
                endpoints.insert((def_name.clone(), inst_idx), vertex_index);
                vertices.push(Vertex {
                    name,
                    area,
                    required_tag: None,
                    materialize: true,
                });
            }
        }

        // Cluster each internal FIFO into its consumer-side Tail (producer
        // fallback for a one-sided stream), and connect task endpoints
        // directly. This keeps placement and routing on one logical topology.
        let mut placement_widths = BTreeMap::<(usize, usize), u32>::new();
        let mut streams = Vec::new();
        let mut co_located = Vec::new();
        for (fifo_name, fifo) in &top.fifos {
            let Some(depth) = fifo.depth else {
                continue; // external passthrough FIFO — not a placed instance
            };
            let data_width = resolve_fifo_data_width(flat, top, fifo_name)
                .ok_or_else(|| GraphError::UnresolvedFifoWidth(fifo_name.clone()))?;
            let physical_width = data_width
                .checked_add(2) // valid/write and ready/full_n
                .ok_or_else(|| GraphError::UnresolvedFifoWidth(fifo_name.clone()))?;

            let src = fifo
                .produced_by
                .as_ref()
                .and_then(|ep| endpoints.get(&(ep.0.clone(), ep.1)).copied());
            let dst = fifo
                .consumed_by
                .as_ref()
                .and_then(|ep| endpoints.get(&(ep.0.clone(), ep.1)).copied());
            let host = dst
                .or(src)
                .ok_or_else(|| GraphError::UnanchoredFifo(fifo_name.clone()))?;

            vertices[host].area = checked_add_area(
                vertices[host].area,
                fifo_area(data_width, floorplanned_fifo_storage_depth(depth)),
            )
            .ok_or_else(|| GraphError::ResourceOverflow(fifo_name.clone()))?;
            co_located.push(CoLocatedInstance {
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
                    let endpoints = (src.min(dst), src.max(dst));
                    let width = placement_widths.entry(endpoints).or_default();
                    *width = width
                        .checked_add(physical_width)
                        .ok_or_else(|| GraphError::PlacementWidthOverflow(fifo_name.clone()))?;
                }
            }
        }

        let axi_nets = add_memory_interfaces(
            flat,
            top,
            memory,
            &mut vertices,
            &mut index,
            &endpoints,
            &mut placement_widths,
            &mut co_located,
        )?;

        let control_nets = if let Some(control) = control.filter(|_| !top.tasks.is_empty()) {
            add_control_interface(
                flat,
                top,
                memory,
                control,
                global_anchor,
                &mut vertices,
                &mut index,
                &endpoints,
                &mut placement_widths,
                &mut co_located,
            )?
        } else {
            Vec::new()
        };

        let placement_edges = placement_widths
            .into_iter()
            .map(|((src, dst), width)| PlacementEdge { src, dst, width })
            .collect();

        Ok(Self {
            vertices,
            placement_edges,
            streams,
            axi_nets,
            control_nets,
            index,
            co_located,
        })
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "control graph construction updates the same transient graph collections as memory"
)]
fn add_control_interface(
    flat: &TaskGraph,
    top: &tapa_ir::Task,
    memory: &[MemoryInterface],
    control: ControlInterface,
    global_anchor: Option<&str>,
    vertices: &mut Vec<Vertex>,
    index: &mut HashMap<String, usize>,
    task_endpoints: &HashMap<(String, u32), usize>,
    placement_widths: &mut BTreeMap<(usize, usize), u32>,
    co_located: &mut Vec<CoLocatedInstance>,
) -> Result<Vec<ControlNet>, GraphError> {
    validate_control_names(flat, top, memory, control, vertices, co_located)?;

    let global_name = global_controller_instance_name().to_string();
    if index.contains_key(&global_name) {
        return Err(GraphError::GeneratedNameCollision {
            generated: global_name,
            existing: "placement vertex".to_string(),
        });
    }
    let global = vertices.len();
    vertices.push(Vertex {
        name: global_name.clone(),
        area: Area::default(),
        required_tag: global_anchor.map(ToString::to_string),
        materialize: true,
    });
    index.insert(global_name, global);
    if control.has_s_axi_control {
        co_located.push(CoLocatedInstance {
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
            let endpoint_index = u32::try_from(instance_index).expect("instance count fits u32");
            let child = task_endpoints[&(definition.clone(), endpoint_index)];
            co_located.push(CoLocatedInstance {
                name: local_controller_instance_name(&canonical),
                host: child,
            });

            let launch_width = control_launch_width(task, instance, &canonical)?;
            add_control_net(
                &mut nets,
                placement_widths,
                &canonical,
                ControlChannel::Launch,
                global,
                child,
                launch_width,
            )?;
            add_control_net(
                &mut nets,
                placement_widths,
                &canonical,
                ControlChannel::Reset,
                global,
                child,
                1,
            )?;
            if instance.step >= 0 {
                add_control_net(
                    &mut nets,
                    placement_widths,
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

fn add_control_net(
    nets: &mut Vec<ControlNet>,
    placement_widths: &mut BTreeMap<(usize, usize), u32>,
    instance: &str,
    channel: ControlChannel,
    src: usize,
    dst: usize,
    width: u32,
) -> Result<(), GraphError> {
    let pair = (src.min(dst), src.max(dst));
    let placement_width = placement_widths.entry(pair).or_default();
    *placement_width =
        placement_width
            .checked_add(width)
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

fn validate_control_names(
    flat: &TaskGraph,
    top: &tapa_ir::Task,
    memory: &[MemoryInterface],
    control: ControlInterface,
    vertices: &[Vertex],
    co_located: &[CoLocatedInstance],
) -> Result<(), GraphError> {
    let mut sanitized_instances = BTreeMap::<String, String>::new();
    let mut occupied = BTreeMap::<String, String>::new();
    let mut canonical_instances = Vec::new();
    for (definition, instances) in &top.tasks {
        if !flat.tasks.contains_key(definition) {
            return Err(GraphError::MissingTaskDef(definition.clone()));
        }
        for (instance_index, instance) in instances.iter().enumerate() {
            let canonical = instance
                .canonical_name(definition, instance_index)
                .into_owned();
            let sanitized = sanitize_identifier_name(&canonical);
            if let Some(first) = sanitized_instances.insert(sanitized.clone(), canonical.clone()) {
                if first != canonical {
                    return Err(GraphError::SanitizedNameCollision {
                        first,
                        second: canonical,
                        sanitized,
                    });
                }
            }
            occupied.insert(canonical.clone(), format!("instance `{canonical}`"));
            occupied.insert(sanitized, format!("instance `{canonical}`"));
            canonical_instances.push((canonical, instance.step < 0));
        }
    }
    for vertex in vertices {
        occupied
            .entry(vertex.name.clone())
            .or_insert_with(|| format!("placement vertex `{}`", vertex.name));
        occupied
            .entry(sanitize_identifier_name(&vertex.name))
            .or_insert_with(|| format!("placement vertex `{}`", vertex.name));
    }
    for fifo in top.fifos.keys() {
        occupied.insert(fifo.clone(), format!("stream `{fifo}`"));
        let rtl_name = format!("{}_fifo", sanitize_array_name(fifo));
        occupied.insert(rtl_name, format!("stream `{fifo}`"));
    }
    for alias in co_located {
        occupied.insert(
            alias.name.clone(),
            format!("co-located RTL instance `{}`", alias.name),
        );
        occupied.insert(
            sanitize_identifier_name(&alias.name),
            format!("co-located RTL instance `{}`", alias.name),
        );
    }
    for interface in memory {
        for (channel, _) in interface.channel_widths.enabled_channels() {
            let name = axi_pipeline_instance_name(&interface.endpoint, channel);
            occupied.insert(
                name,
                format!("AXI pipeline for `{}`", interface.endpoint.top_port),
            );
        }
    }

    reserve_generated_name(
        &mut occupied,
        global_controller_instance_name().to_string(),
        "global controller",
    )?;
    if control.has_s_axi_control {
        reserve_generated_name(
            &mut occupied,
            CONTROL_S_AXI_INSTANCE.to_string(),
            "S-AXI control block",
        )?;
    }
    for (canonical, autorun) in canonical_instances {
        reserve_generated_name(
            &mut occupied,
            local_controller_instance_name(&canonical),
            &format!("local controller for `{canonical}`"),
        )?;
        let channels = [ControlChannel::Launch, ControlChannel::Reset]
            .into_iter()
            .chain((!autorun).then_some(ControlChannel::Completion));
        for channel in channels {
            reserve_generated_name(
                &mut occupied,
                control_pipeline_instance_name(&canonical, channel),
                &format!("{channel:?} pipeline for `{canonical}`"),
            )?;
        }
    }
    Ok(())
}

fn reserve_generated_name(
    occupied: &mut BTreeMap<String, String>,
    generated: String,
    owner: &str,
) -> Result<(), GraphError> {
    if let Some(existing) = occupied.get(&generated) {
        return Err(GraphError::GeneratedNameCollision {
            generated,
            existing: existing.clone(),
        });
    }
    occupied.insert(generated, owner.to_string());
    Ok(())
}

#[allow(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "memory graph construction validates and updates the shared transient graph state"
)]
fn add_memory_interfaces(
    flat: &TaskGraph,
    top: &tapa_ir::Task,
    memory: &[MemoryInterface],
    vertices: &mut Vec<Vertex>,
    index: &mut HashMap<String, usize>,
    task_endpoints: &HashMap<(String, u32), usize>,
    placement_widths: &mut BTreeMap<(usize, usize), u32>,
    co_located: &mut Vec<CoLocatedInstance>,
) -> Result<Vec<AxiNet>, GraphError> {
    let expected = expected_memory_interfaces(flat, top, task_endpoints)?;
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

    let mut occupied = occupied_rtl_names(top, vertices, co_located);
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
            co_located.push(CoLocatedInstance {
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
            if index.contains_key(&name) {
                return Err(GraphError::DuplicateVertex(name));
            }
            let terminal = vertices.len();
            vertices.push(Vertex {
                name: name.clone(),
                area: Area::default(),
                required_tag: Some(interface.bank.to_string()),
                materialize: false,
            });
            index.insert(name, terminal);
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
            let pair = (src.min(dst), src.max(dst));
            let placement_width = placement_widths.entry(pair).or_default();
            *placement_width = placement_width.checked_add(width).ok_or_else(|| {
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

fn occupied_rtl_names(
    top: &tapa_ir::Task,
    vertices: &[Vertex],
    co_located: &[CoLocatedInstance],
) -> BTreeMap<String, String> {
    let mut occupied = BTreeMap::new();
    for vertex in vertices {
        for name in [vertex.name.clone(), sanitize_identifier_name(&vertex.name)] {
            occupied
                .entry(name)
                .or_insert_with(|| format!("placement vertex `{}`", vertex.name));
        }
    }
    for fifo in top.fifos.keys() {
        occupied.insert(fifo.clone(), format!("stream `{fifo}`"));
        occupied.insert(
            format!("{}_fifo", sanitize_array_name(fifo)),
            format!("stream `{fifo}`"),
        );
    }
    for alias in co_located {
        for name in [alias.name.clone(), sanitize_identifier_name(&alias.name)] {
            occupied.insert(name, format!("co-located RTL instance `{}`", alias.name));
        }
    }
    occupied
}

fn validate_memory_interface_shape(
    interface: &MemoryInterface,
    expected: ExpectedMemoryEndpoint,
) -> Result<(), GraphError> {
    let endpoint = &interface.endpoint;
    let unsupported = |detail: String| GraphError::UnsupportedMemoryInterface {
        port: endpoint.top_port.clone(),
        detail,
    };
    match (expected.child_category, &interface.bridge_instance) {
        (tapa_ir::ArgCategory::Mmap, Some(_)) => {
            return Err(unsupported(format!(
                "plain mmap endpoint `{}.{}` cannot own an async bridge",
                endpoint.instance, endpoint.port
            )));
        }
        (tapa_ir::ArgCategory::AsyncMmap, Some(actual)) => {
            let stable = async_mmap_bridge_instance_name(&endpoint.top_port);
            if *actual != stable {
                return Err(unsupported(format!(
                    "async bridge name `{actual}` does not match stable hierarchy `{stable}`"
                )));
            }
        }
        (tapa_ir::ArgCategory::Mmap | tapa_ir::ArgCategory::AsyncMmap, None) => {}
        _ => unreachable!("expected memory endpoint has a direct mmap category"),
    }

    let widths = interface.channel_widths;
    let read_enabled = (widths.read_address != 0, widths.read_data != 0);
    if read_enabled.0 != read_enabled.1 {
        return Err(unsupported(format!(
            "endpoint `{}.{}` has a partial read channel group",
            endpoint.instance, endpoint.port
        )));
    }
    let write_enabled = (
        widths.write_address != 0,
        widths.write_data != 0,
        widths.write_response != 0,
    );
    if write_enabled.0 != write_enabled.1 || write_enabled.0 != write_enabled.2 {
        return Err(unsupported(format!(
            "endpoint `{}.{}` has a partial write channel group",
            endpoint.instance, endpoint.port
        )));
    }
    if interface.bridge_instance.is_none() && (!read_enabled.0 || !write_enabled.0) {
        return Err(unsupported(format!(
            "direct compact M-AXI endpoint `{}.{}` must expose all five channels",
            endpoint.instance, endpoint.port
        )));
    }
    Ok(())
}

fn expected_memory_interfaces(
    flat: &TaskGraph,
    top: &tapa_ir::Task,
    task_endpoints: &HashMap<(String, u32), usize>,
) -> Result<BTreeMap<AxiEndpoint, ExpectedMemoryEndpoint>, GraphError> {
    let mut expected = BTreeMap::new();
    let mut top_port_users = BTreeMap::<String, Vec<String>>::new();

    for (definition, instances) in &top.tasks {
        let task = flat
            .tasks
            .get(definition)
            .ok_or_else(|| GraphError::MissingTaskDef(definition.clone()))?;
        for (instance_index, instance) in instances.iter().enumerate() {
            let instance_name = instance
                .canonical_name(definition, instance_index)
                .into_owned();
            let endpoint_index = u32::try_from(instance_index).expect("instance count fits u32");
            let task_vertex = task_endpoints[&(definition.clone(), endpoint_index)];
            for (child_port, argument) in &instance.args {
                if !argument.cat.is_direct_mmap() {
                    continue;
                }

                let child = find_port(&task.ports, child_port).ok_or_else(|| {
                    GraphError::UnsupportedMemoryInterface {
                        port: argument.arg.clone(),
                        detail: format!(
                            "child endpoint `{instance_name}.{child_port}` has no port metadata"
                        ),
                    }
                })?;
                let parent = find_port(&top.ports, &argument.arg).ok_or_else(|| {
                    GraphError::UnsupportedMemoryInterface {
                        port: argument.arg.clone(),
                        detail: "top-level mmap port metadata is missing".to_string(),
                    }
                })?;
                if child.cat != argument.cat || parent.cat != tapa_ir::ArgCategory::Mmap {
                    return Err(GraphError::UnsupportedMemoryInterface {
                        port: argument.arg.clone(),
                        detail: format!(
                            "child binding category '{}' must match child metadata '{}' and connect to a plain top mmap",
                            argument.cat.as_str(),
                            child.cat.as_str(),
                        ),
                    });
                }
                if child.chan_count.is_some()
                    || child.chan_size.is_some()
                    || parent.chan_count.is_some()
                    || parent.chan_size.is_some()
                {
                    return Err(GraphError::UnsupportedMemoryInterface {
                        port: argument.arg.clone(),
                        detail: "hierarchical mmap channels are not modeled".to_string(),
                    });
                }

                let endpoint = AxiEndpoint {
                    instance: instance_name.clone(),
                    port: child_port.clone(),
                    top_port: argument.arg.clone(),
                };
                top_port_users
                    .entry(argument.arg.clone())
                    .or_default()
                    .push(format!("{instance_name}.{child_port}"));
                expected.insert(
                    endpoint,
                    ExpectedMemoryEndpoint {
                        task_vertex,
                        child_category: child.cat,
                    },
                );
            }
        }
    }

    if let Some((top_port, users)) = top_port_users
        .into_iter()
        .find(|(_, users)| users.len() > 1)
    {
        return Err(GraphError::UnsupportedMemoryInterface {
            port: top_port,
            detail: format!("shared by {} child endpoints", users.join(", ")),
        });
    }
    Ok(expected)
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
fn resolve_fifo_data_width(flat: &TaskGraph, top: &tapa_ir::Task, fifo_name: &str) -> Option<u32> {
    for (def_name, instances) in &top.tasks {
        for inst in instances {
            for (port_name, arg) in &inst.args {
                if arg.arg == fifo_name && arg.cat.is_stream() {
                    if let Some(width) = port_width(flat, def_name, port_name) {
                        return width.checked_add(1);
                    }
                }
            }
        }
    }
    None
}

fn checked_add_area(lhs: Area, rhs: Area) -> Option<Area> {
    Some(Area {
        lut: lhs.lut.checked_add(rhs.lut)?,
        ff: lhs.ff.checked_add(rhs.ff)?,
        bram_18k: lhs.bram_18k.checked_add(rhs.bram_18k)?,
        dsp: lhs.dsp.checked_add(rhs.dsp)?,
        uram: lhs.uram.checked_add(rhs.uram)?,
    })
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
fn find_port<'a>(ports: &'a [tapa_ir::Port], name: &str) -> Option<&'a tapa_ir::Port> {
    ports.iter().find(|p| p.name == name).or_else(|| {
        let base = name.split('[').next().unwrap_or(name);
        if base.is_empty() || base == name {
            None
        } else {
            ports.iter().find(|p| p.name == base)
        }
    })
}

/// Analytic FIFO area used by the placement resource model.
///
/// Depth `< 128` infers an SRL/distributed-RAM FIFO; deeper infers a BRAM
/// FIFO. `width` is the full data width (payload + eot bit). Every term is
/// exact integer arithmetic equivalent to the upstream float expressions.
#[must_use]
pub fn fifo_area(width: u32, depth: u32) -> Area {
    let width = u64::from(width);
    let depth = u64::from(depth);
    let log2 = depth.max(1).ilog2();
    let log_term = 3 * u64::from(log2);

    if depth < 128 {
        // lut_ram = floor(width·((depth-1)/16 + 1)) = width + width·(depth-1)/16
        let lut_ram = width + width * depth.saturating_sub(1) / 16;
        let lut_logic = 15 + log_term;
        Area {
            lut: lut_ram + lut_logic,
            ff: 7 + log_term,
            bram_18k: 0,
            dsp: 0,
            uram: 0,
        }
    } else {
        let count = depth / 512 + 1;
        Area {
            lut: width * count,
            ff: 0,
            // int(width/36 · 2 · count) == width·count/18
            bram_18k: width * count / 18,
            dsp: 0,
            uram: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A two-leaf `A -> fifo -> B` design, mirroring the flatten test graph.
    fn vadd_graph() -> TaskGraph {
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
                    "self_area": {"LUT": 100, "FF": 200}
                },
                "B": {
                    "readable_name": "B", "code": "void B() {}", "level": "lower", "synth": "hls",
                    "ports": [{"cat": "istream", "name": "in", "type": "float", "width": 32}],
                    "self_area": {"LUT": 50, "FF": 60}
                }
            }
        }"#;
        tapa_ir::TaskGraph::from_json(json).expect("parse vadd graph")
    }

    fn mmap_graph() -> TaskGraph {
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
                        "self_area": {"LUT":10,"FF":20}
                    }
                }
            }"#,
        )
        .expect("parse mmap graph")
    }

    fn mmap_interface() -> MemoryInterface {
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

    fn async_mmap_graph(instance_name: &str) -> TaskGraph {
        let mut value = serde_json::to_value(mmap_graph()).expect("serialize mmap graph");
        value["tasks"]["Top"]["tasks"]["Reader"][0]["name"] = serde_json::json!(instance_name);
        value["tasks"]["Top"]["tasks"]["Reader"][0]["args"]["data"]["cat"] =
            serde_json::json!("async_mmap");
        value["tasks"]["Reader"]["ports"][0]["cat"] = serde_json::json!("async_mmap");
        TaskGraph::from_json(&value.to_string()).expect("parse async mmap graph")
    }

    fn async_mmap_interface(instance_name: &str, read: bool, write: bool) -> MemoryInterface {
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

    fn distributed_control_graph() -> TaskGraph {
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

    fn control_memory_interface() -> MemoryInterface {
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
    fn distributed_control_rejects_ambiguous_or_missing_names_and_scalars() {
        let cases = [
            (
                serde_json::json!({
                    "A": [{"name":"same","args":{}}, {"name":"same","args":{}}]
                }),
                "duplicate flattened instance canonical name",
            ),
            (
                serde_json::json!({
                    "A": [{"name":"same#name","args":{}}, {"name":"same?name","args":{}}]
                }),
                "both sanitize to RTL name",
            ),
            (
                serde_json::json!({
                    "A": [{"name":"__tapa_global_controller","args":{}}]
                }),
                "generated RTL name",
            ),
        ];
        for (tasks, expected) in cases {
            let mut value = serde_json::to_value(vadd_graph()).expect("serialize graph");
            value["tasks"]["VecAdd"]["tasks"] = tasks;
            value["tasks"]["VecAdd"]["fifos"] = serde_json::json!({});
            let design = TaskGraph::from_json(&value.to_string()).expect("parse case");
            let error = FloorGraph::build_with_interfaces(
                &design,
                &[],
                Some(ControlInterface::default()),
                None,
            )
            .expect_err("invalid control graph");
            assert!(error.to_string().contains(expected), "{error}");
        }

        let mut value = serde_json::to_value(distributed_control_graph()).expect("serialize graph");
        value["tasks"]["Top"]["tasks"]["Ticker"][0]["args"] = serde_json::json!({});
        let design = TaskGraph::from_json(&value.to_string()).expect("parse missing scalar");
        let flat = tapa_ir::flatten(&design).expect("flatten missing scalar");
        let error = FloorGraph::build_with_interfaces(
            &flat,
            &[control_memory_interface()],
            Some(ControlInterface::default()),
            None,
        )
        .expect_err("missing scalar must fail");
        assert!(matches!(error, GraphError::ScalarMetadata { .. }));
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

    #[test]
    fn async_bridge_name_collision_fails_closed() {
        let flat = tapa_ir::flatten(&async_mmap_graph("mem__m_axi")).expect("flatten");
        let error = FloorGraph::build_with_memory(
            &flat,
            &[async_mmap_interface("mem__m_axi", true, false)],
        )
        .expect_err("generated bridge must not alias its task");

        assert!(matches!(
            error,
            GraphError::GeneratedNameCollision { ref generated, .. }
                if generated == "mem__m_axi"
        ));
    }

    #[test]
    fn direct_mmap_without_exact_interface_fails_closed() {
        let flat = tapa_ir::flatten(&mmap_graph()).expect("flatten mmap graph");
        let error = FloorGraph::build(&flat).expect_err("binding is required");
        assert!(matches!(
            error,
            GraphError::MissingMemoryInterface {
                ref instance,
                ref port,
                ref top_port,
            } if instance == "Reader_0" && port == "data" && top_port == "mem"
        ));
    }

    #[test]
    fn srl_fifo_area_matches_upstream() {
        // depth 100, width 32: lut_ram = 32 + 32·99/16 = 32 + 198 = 230,
        // lut_logic = 15 + 3·6 = 33, ff = 7 + 18 = 25.
        let area = fifo_area(32, 100);
        assert_eq!(
            area,
            Area {
                lut: 263,
                ff: 25,
                bram_18k: 0,
                dsp: 0,
                uram: 0
            }
        );
    }

    #[test]
    fn floorplanned_shallow_fifo_area_includes_ready_feedback_capacity() {
        assert_eq!(
            fifo_area(33, floorplanned_fifo_storage_depth(64)),
            fifo_area(33, 69),
        );
        assert_eq!(
            fifo_area(33, floorplanned_fifo_storage_depth(65)),
            fifo_area(33, 65),
            "deep co-located FIFOs retain their existing implementation",
        );
    }

    #[test]
    fn bram_fifo_area_matches_upstream() {
        // depth 1024, width 144: count = 1024/512 + 1 = 3, lut = 432,
        // bram_18k = 144·3/18 = 24.
        let area = fifo_area(144, 1024);
        assert_eq!(
            area,
            Area {
                lut: 432,
                ff: 0,
                bram_18k: 24,
                dsp: 0,
                uram: 0
            }
        );
    }
}
