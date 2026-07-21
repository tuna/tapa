//! The [`FloorGraph`]: the placement graph the floorplan ILP runs on, built
//! from a *flattened* [`TaskGraph`].
//!
//! Vertices are every top-level RTL instance after flattening — leaf task
//! instances (area from `Task::self_area`) and internal FIFO instances (area
//! from an analytic SRL/BRAM formula). Edges are the stream connections
//! between them, weighted by wire width (payload + the eot bit). Clustering
//! (`super::cluster`) later folds each FIFO into an endpoint so nothing floats
//! outside a slot after read-back.

use std::collections::HashMap;

use tapa_ir::{Area, TaskGraph};

/// What a [`Vertex`] represents in the netlist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VertexKind {
    /// A leaf task instance.
    Task,
    /// An internal (depth-carrying) FIFO instance.
    Fifo,
}

/// One placeable instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vertex {
    /// Canonical instance name — the key it will carry in
    /// [`FloorplanResult::regions`](tapa_ir::FloorplanResult::regions).
    pub name: String,
    pub kind: VertexKind,
    /// Resource footprint.
    pub area: Area,
    /// This instance binds a top-level M-AXI (`mmap`/`async_mmap`) port, so it
    /// drives external memory and must be pinned to a memory-bearing slot (on
    /// u280, HBM lives in SLR0). The partition ILP restricts it accordingly.
    pub needs_hbm: bool,
}

/// A directed, width-weighted connection between two vertices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Edge {
    /// Source vertex index.
    pub src: usize,
    /// Destination vertex index.
    pub dst: usize,
    /// Bundled wire width (payload + eot bit).
    pub width: u32,
}

/// The placement graph: vertices (instances) and width-weighted stream edges.
#[derive(Debug, Clone)]
pub struct FloorGraph {
    vertices: Vec<Vertex>,
    edges: Vec<Edge>,
    index: HashMap<String, usize>,
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
}

impl FloorGraph {
    /// All vertices, in creation order (tasks first, then FIFOs).
    #[must_use]
    pub fn vertices(&self) -> &[Vertex] {
        &self.vertices
    }

    /// All edges.
    #[must_use]
    pub fn edges(&self) -> &[Edge] {
        &self.edges
    }

    /// The vertex at `index`.
    #[must_use]
    pub fn vertex(&self, index: usize) -> &Vertex {
        &self.vertices[index]
    }

    /// The index of the vertex named `name`.
    #[must_use]
    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.index.get(name).copied()
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
                    kind: VertexKind::Task,
                    area,
                    needs_hbm,
                });
            }
        }

        // Internal FIFO vertices + their stream edges.
        let mut edges = Vec::new();
        for (fifo_name, fifo) in &top.fifos {
            let Some(depth) = fifo.depth else {
                continue; // external passthrough FIFO — not a placed instance
            };
            let width = resolve_fifo_width(flat, top, fifo_name)
                .ok_or_else(|| GraphError::UnresolvedFifoWidth(fifo_name.clone()))?;

            let fifo_index = vertices.len();
            index.insert(fifo_name.clone(), fifo_index);
            vertices.push(Vertex {
                name: fifo_name.clone(),
                kind: VertexKind::Fifo,
                area: fifo_area(width, depth),
                needs_hbm: false,
            });

            if let Some(producer) = &fifo.produced_by {
                if let Some(&src) = endpoints.get(&(producer.0.clone(), producer.1)) {
                    edges.push(Edge {
                        src,
                        dst: fifo_index,
                        width,
                    });
                }
            }
            if let Some(consumer) = &fifo.consumed_by {
                if let Some(&dst) = endpoints.get(&(consumer.0.clone(), consumer.1)) {
                    edges.push(Edge {
                        src: fifo_index,
                        dst,
                        width,
                    });
                }
            }
        }

        Ok(Self {
            vertices,
            edges,
            index,
        })
    }
}

/// Resolve a FIFO's wire width (payload + eot bit) from whichever endpoint's
/// stream port binds to it. The instance's definition name is known from the
/// iteration, so the owning port is looked up directly.
fn resolve_fifo_width(flat: &TaskGraph, top: &tapa_ir::Task, fifo_name: &str) -> Option<u32> {
    for (def_name, instances) in &top.tasks {
        for inst in instances {
            for (port_name, arg) in &inst.args {
                if arg.arg == fifo_name && arg.cat.is_stream() {
                    if let Some(width) = port_width(flat, def_name, port_name) {
                        return Some(width + 1);
                    }
                }
            }
        }
    }
    None
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

/// Analytic FIFO area, ported from RapidStream's `infer_area.py`.
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
    fn build_vadd_vertices_and_edges() {
        let flat = tapa_ir::flatten(&vadd_graph()).expect("flatten");
        let graph = FloorGraph::build(&flat).expect("build floor graph");

        // Three vertices: A_0, B_0, and the fifo.
        assert_eq!(graph.vertices().len(), 3, "A, B, and the FIFO");
        let a = graph.index_of("A_0").expect("A_0 vertex");
        let b = graph.index_of("B_0").expect("B_0 vertex");
        let fifo = graph.index_of("fifo_VecAdd").expect("fifo vertex");

        assert_eq!(graph.vertex(a).kind, VertexKind::Task);
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
                lut: 50,
                ff: 60,
                ..Area::default()
            }
        );
        assert_eq!(graph.vertex(fifo).kind, VertexKind::Fifo);

        // Edges A_0 -> fifo -> B_0, each 33 bits wide (32 payload + eot).
        let edges = graph.edges();
        assert_eq!(edges.len(), 2, "producer->fifo and fifo->consumer");
        assert!(edges
            .iter()
            .any(|e| e.src == a && e.dst == fifo && e.width == 33));
        assert!(edges
            .iter()
            .any(|e| e.src == fifo && e.dst == b && e.width == 33));
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
