//! The [`FloorGraph`]: the placement graph the floorplan ILP runs on, built
//! from a *flattened* [`TaskGraph`](tapa_ir::TaskGraph).
//!
//! Vertices are clusters rooted at flattened leaf-task instances.  Each
//! internal FIFO is clustered into its consumer (producer fallback), because
//! the pipelined stream's storage is implemented at its destination.  The
//! FIFO's area is charged to that cluster and its name is retained as a
//! co-location alias for the final XDC. Placement aggregates stream widths by
//! task pair, while routing retains each named stream independently.

use tapa_ir::{Area, AxiChannel, AxiChannelWidths, AxiEndpoint, ControlChannel, MemoryBank};

pub(crate) const CONTROL_S_AXI_INSTANCE: &str = "control_s_axi_U";

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
pub(crate) struct ExpectedMemoryEndpoint {
    pub task_vertex: usize,
    pub child_category: tapa_ir::ArgCategory,
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
    /// Sum of the physical widths of all channels between the endpoints.
    pub width: u32,
    /// The part of [`width`](Self::width) flowing `src` → `dst`.
    ///
    /// A channel's generated Head registers live in its *source* slot, so the
    /// two directions are charged to different regions when the edge crosses.
    pub forward_width: u32,
    /// The part of [`width`](Self::width) flowing `dst` → `src`.
    pub reverse_width: u32,
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
pub(crate) struct CoLocatedInstance {
    pub name: String,
    pub host: usize,
    /// For the half of a frame channel at `host`, the vertex at the
    /// channel's other end: codegen copies the channel, and instantiates
    /// this half, only when that end lands in another slot.
    pub apart_from: Option<usize>,
}

/// The transient graph view shared by placement and stream routing.
#[derive(Debug, Clone)]
pub struct FloorGraph {
    pub(crate) vertices: Vec<Vertex>,
    pub(crate) placement_edges: Vec<PlacementEdge>,
    pub(crate) streams: Vec<Stream>,
    pub(crate) axi_nets: Vec<AxiNet>,
    pub(crate) control_nets: Vec<ControlNet>,
    pub(crate) index: std::collections::HashMap<String, usize>,
    pub(crate) co_located: Vec<CoLocatedInstance>,
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
    /// A FIFO's producer and consumer ports disagree on the payload width, so
    /// the planner cannot model the same wires codegen will emit.
    #[error(
        "stream `{fifo}` has producer port width {producer} but consumer port width {consumer}"
    )]
    FifoWidthMismatch {
        fifo: String,
        producer: u32,
        consumer: u32,
    },
    /// Two logical objects (task instances, FIFOs, co-located aliases) claim
    /// the same placement/RTL name, which one `regions` key cannot represent.
    #[error("name `{name}` is claimed by both {first} and {second}")]
    NameConflict {
        name: String,
        first: String,
        second: String,
    },
    /// An internal FIFO has no placeable task endpoint to own its storage.
    #[error("internal stream `{0}` has no placeable endpoint")]
    UnanchoredFifo(String),
    /// A FIFO names a producer/consumer endpoint that is not a flattened
    /// task instance — malformed IR, not a deliberately one-sided stream.
    #[error(
        "stream `{fifo}` names {role} `{definition}[{index}]`, which is not a flattened task instance"
    )]
    DanglingFifoEndpoint {
        /// The FIFO with the broken reference.
        fifo: String,
        /// `producer` or `consumer`.
        role: &'static str,
        /// Task definition the reference names.
        definition: String,
        /// Instance index within that definition.
        index: u32,
    },
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
    /// A stream's generated RTL instance would not be a legal Verilog
    /// identifier, so neither codegen nor the XDC cell patterns can name it.
    #[error("stream `{fifo}` generates RTL instance `{rtl}`, which is not a legal identifier")]
    UnrepresentableStreamName { fifo: String, rtl: String },
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

/// Analytic FIFO area used by the placement resource model.
///
/// It follows the storage `fifo.v` and `relay_station.v` select: a FIFO at
/// least 36 bits wide and 4096 deep is pinned to URAM
/// (`ram_style = "ultra"`), any other is [`ram_area`]. `width` is the full
/// data width (payload + eot bit).
#[must_use]
pub fn fifo_area(width: u32, depth: u32) -> Area {
    let area = ram_area(width, depth);
    if width >= 36 && depth >= 4096 {
        // Vivado tiles the memory into 72-bit x 4096-entry URAM288 blocks.
        Area {
            bram_18k: 0,
            uram: u64::from(width.div_ceil(72)) * u64::from(depth.div_ceil(4096)),
            ..area
        }
    } else {
        area
    }
}

/// Analytic area of `depth` x `width` storage outside URAM.
///
/// Depth `< 128` infers SRL/distributed RAM; deeper infers block RAM. Frame
/// channel banks (`ram_style = "auto"`) are charged this directly: Vivado
/// does not follow the FIFOs' URAM rule for them. Every term is exact
/// integer arithmetic equivalent to the upstream float expressions.
#[must_use]
pub(crate) fn ram_area(width: u32, depth: u32) -> Area {
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
    use tapa_ir::floorplanned_fifo_storage_depth;

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

    /// `fifo.v`/`relay_station.v` pin a FIFO of at least 36 bits by 4096
    /// entries to URAM. Vivado 2025.2 synthesis of those FIFOs (xcu55c)
    /// placed them in 1, 2, 2, 4 and 4 URAM288 and no block RAM; just below
    /// either threshold the FIFO stays in block RAM.
    #[test]
    fn wide_deep_fifo_area_is_uram() {
        for ((width, depth), uram) in [
            ((36, 4096), 1),
            ((65, 8192), 2),
            ((144, 4096), 2),
            ((73, 4097), 4),
            ((64, 16384), 4),
        ] {
            let area = fifo_area(width, depth);
            assert_eq!((area.uram, area.bram_18k), (uram, 0), "{width}x{depth}");
        }
        for (width, depth) in [(35, 4096), (36, 4095)] {
            assert_eq!(fifo_area(width, depth), ram_area(width, depth));
            assert_eq!(fifo_area(width, depth).uram, 0);
        }
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
