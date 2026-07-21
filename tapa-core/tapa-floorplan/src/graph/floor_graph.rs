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

use tapa_ir::{Area, TaskGraph};

/// One placeable instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vertex {
    /// Canonical instance name — the key it will carry in
    /// [`FloorplanResult::regions`](tapa_ir::FloorplanResult::regions).
    pub name: String,
    /// Resource footprint, including any destination-side FIFO storage
    /// clustered into this task.
    pub area: Area,
    /// This instance binds a top-level M-AXI (`mmap`/`async_mmap`) port, so it
    /// drives external memory and must be pinned to a memory-bearing slot (on
    /// u280, HBM lives in SLR0). The partition ILP restricts it accordingly.
    pub needs_hbm: bool,
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
}

/// A real RTL instance whose area and placement belong to a host vertex.
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

    /// Build the placement graph from an already-*flattened* graph (every leaf
    /// instance directly under the top task).
    pub fn build(flat: &TaskGraph) -> Result<Self, GraphError> {
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
                index.insert(name.clone(), vertex_index);
                let inst_idx = u32::try_from(idx).expect("instance count fits u32");
                endpoints.insert((def_name.clone(), inst_idx), vertex_index);
                let needs_hbm = inst.args.values().any(|arg| arg.cat.is_direct_mmap());
                vertices.push(Vertex {
                    name,
                    area,
                    needs_hbm,
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

            vertices[host].area =
                checked_add_area(vertices[host].area, fifo_area(data_width, depth))
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
                    });
                    let endpoints = (src.min(dst), src.max(dst));
                    let width = placement_widths.entry(endpoints).or_default();
                    *width = width
                        .checked_add(physical_width)
                        .ok_or_else(|| GraphError::PlacementWidthOverflow(fifo_name.clone()))?;
                }
            }
        }

        let placement_edges = placement_widths
            .into_iter()
            .map(|((src, dst), width)| PlacementEdge { src, dst, width })
            .collect();

        Ok(Self {
            vertices,
            placement_edges,
            streams,
            index,
            co_located,
        })
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
fn port_width(flat: &TaskGraph, def_name: &str, port_name: &str) -> Option<u32> {
    flat.tasks
        .get(def_name)?
        .ports
        .iter()
        .find(|p| p.name == port_name)
        .map(|p| p.width)
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
                // 50/60 task area + 53/10 for a 33-bit, depth-2 FIFO.
                lut: 103,
                ff: 70,
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
