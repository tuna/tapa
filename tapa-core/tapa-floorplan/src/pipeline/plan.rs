//! The pipeline plan.
//!
//! From a placement, find the stream channels that cross slot boundaries,
//! route them, and turn each route into a [`Crossing`] (register `level`,
//! distribution `scheme`, and per-register slot regions).
//!
//! `Single` places one body register per intermediate slot, `Double` two per
//! hop, and `SingleHDoubleV` one per horizontal hop and two per vertical (SLR)
//! hop.

use std::collections::BTreeMap;

use tapa_ir::{Crossing, CrossingKind, PipelineScheme};

use crate::device::model::{Coor, Device};
use crate::graph::FloorGraph;
use crate::route::ilp::{route_nets, slot_tag, RouteError, RouteNet};
use crate::route::paths::Cell;
use crate::solver::{SolveOpts, Solver};

/// Why the pipeline plan could not be produced.
#[derive(Debug, thiserror::Error)]
pub enum PipelineError {
    /// Routing the cross-slot nets failed.
    #[error(transparent)]
    Route(#[from] RouteError),
    /// A region tag could not be parsed back to a slot.
    #[error("region tag `{0}` is not a slot")]
    BadRegion(String),
}

/// The number of Body pipeline cells a route implies under `scheme`.
///
/// Head and Tail are not included. In particular, an adjacent Single or
/// Single-H/Double-V horizontal crossing has zero Body cells while retaining
/// its separately generated Head and Tail.
#[must_use]
pub fn pipeline_level(route: &[Cell], scheme: PipelineScheme) -> u32 {
    u32::try_from(pipeline_reg_regions(route, scheme).len()).unwrap_or(u32::MAX)
}

/// Return the exact ordered Body-cell regions for `route` and `scheme`.
///
/// - Single uses only the intermediate route slots.
/// - Double places one cell on each side of every boundary.
/// - Single-H/Double-V duplicates every route slot incident to a vertical
///   boundary, then removes the first Head and last Tail entries.
#[must_use]
pub fn pipeline_reg_regions(route: &[Cell], scheme: PipelineScheme) -> Vec<String> {
    if route.len() < 2 {
        return Vec::new();
    }

    match scheme {
        PipelineScheme::Single => route[1..route.len() - 1]
            .iter()
            .map(|&cell| slot_tag(cell))
            .collect(),
        PipelineScheme::Double => route
            .windows(2)
            .flat_map(|hop| [slot_tag(hop[0]), slot_tag(hop[1])])
            .collect(),
        PipelineScheme::SingleHDoubleV => {
            let mut multiplicity = vec![1usize; route.len()];
            for (index, hop) in route.windows(2).enumerate() {
                if hop[0].1 != hop[1].1 {
                    multiplicity[index] = 2;
                    multiplicity[index + 1] = 2;
                }
            }
            let expanded: Vec<String> = route
                .iter()
                .zip(multiplicity)
                .flat_map(|(&cell, count)| std::iter::repeat_n(slot_tag(cell), count))
                .collect();
            expanded[1..expanded.len() - 1].to_vec()
        }
    }
}

/// Parse a single-slot region tag into its grid cell.
fn region_cell(region: &str) -> Result<Cell, PipelineError> {
    Coor::from_region_name(region)
        .map(|coor| (coor.dl_x, coor.dl_y))
        .ok_or_else(|| PipelineError::BadRegion(region.to_string()))
}

/// A cross-slot stream awaiting routing: its FIFO name, endpoints, and width.
struct StreamNet {
    link: String,
    src: Cell,
    dst: Cell,
    width: u32,
}

/// Find every logical stream edge whose endpoints landed in different
/// regions. The placement graph has already clustered the stream's physical
/// FIFO into its consumer, so this is also the topology codegen implements.
fn cross_slot_streams(
    graph: &FloorGraph,
    regions: &BTreeMap<String, String>,
) -> Result<Vec<StreamNet>, PipelineError> {
    let mut nets = Vec::new();
    for edge in graph.streams() {
        let src_region = regions
            .get(&graph.vertex(edge.src).name)
            .ok_or_else(|| PipelineError::BadRegion(graph.vertex(edge.src).name.clone()))?;
        let dst_region = regions
            .get(&graph.vertex(edge.dst).name)
            .ok_or_else(|| PipelineError::BadRegion(graph.vertex(edge.dst).name.clone()))?;
        if src_region == dst_region {
            continue; // co-located: no crossing
        }
        nets.push(StreamNet {
            link: edge.link.clone(),
            src: region_cell(src_region)?,
            dst: region_cell(dst_region)?,
            width: edge.width,
        });
    }
    Ok(nets)
}

/// Plan every cross-slot stream crossing for a placed design.
pub fn plan_crossings(
    graph: &FloorGraph,
    regions: &BTreeMap<String, String>,
    device: &Device,
    scheme: PipelineScheme,
    solver: &dyn Solver,
    opts: &SolveOpts,
) -> Result<Vec<Crossing>, PipelineError> {
    let streams = cross_slot_streams(graph, regions)?;
    if streams.is_empty() {
        return Ok(Vec::new());
    }

    let route_nets_input: Vec<RouteNet> = streams
        .iter()
        .map(|net| RouteNet {
            src: net.src,
            dst: net.dst,
            width: net.width,
        })
        .collect();
    let routes = route_nets(&route_nets_input, device, solver, opts)?;

    let crossings = streams
        .iter()
        .zip(routes)
        .map(|(net, route)| {
            let reg_regions = pipeline_reg_regions(&route, scheme);
            let level = u32::try_from(reg_regions.len()).unwrap_or(u32::MAX);
            Crossing {
                kind: CrossingKind::Stream,
                link: net.link.clone(),
                route: route.iter().map(|&cell| slot_tag(cell)).collect(),
                level,
                scheme,
                reg_regions,
            }
        })
        .collect();
    Ok(crossings)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn two_task_stream_graph() -> FloorGraph {
        let design = tapa_ir::TaskGraph::from_json(
            r#"{
                "cflags": [], "top": "Top", "target": "xilinx-hls",
                "tasks": {
                    "Top": {
                        "readable_name": "Top", "code": "void Top() {}",
                        "level": "upper", "synth": "hls", "ports": [],
                        "tasks": {
                            "Producer": [{"args": {
                                "out32": {"arg": "q32", "cat": "ostream"},
                                "out64": {"arg": "q64", "cat": "ostream"}
                            }, "step": 0}],
                            "Consumer": [{"args": {
                                "in32": {"arg": "q32", "cat": "istream"},
                                "in64": {"arg": "q64", "cat": "istream"}
                            }, "step": 0}]
                        },
                        "fifos": {
                            "q32": {"depth": 2, "produced_by": ["Producer", 0], "consumed_by": ["Consumer", 0]},
                            "q64": {"depth": 2, "produced_by": ["Producer", 0], "consumed_by": ["Consumer", 0]}
                        }
                    },
                    "Producer": {
                        "readable_name": "Producer", "code": "void Producer() {}",
                        "level": "lower", "synth": "hls",
                        "ports": [
                            {"cat": "ostream", "name": "out32", "type": "int", "width": 32},
                            {"cat": "ostream", "name": "out64", "type": "long", "width": 64}
                        ]
                    },
                    "Consumer": {
                        "readable_name": "Consumer", "code": "void Consumer() {}",
                        "level": "lower", "synth": "hls",
                        "ports": [
                            {"cat": "istream", "name": "in32", "type": "int", "width": 32},
                            {"cat": "istream", "name": "in64", "type": "long", "width": 64}
                        ]
                    }
                }
            }"#,
        )
        .expect("parse graph");
        let flat = tapa_ir::flatten(&design).expect("flatten graph");
        FloorGraph::build(&flat).expect("floor graph")
    }

    #[test]
    fn routing_retains_parallel_streams_aggregated_by_placement() {
        let graph = two_task_stream_graph();
        let mut regions = BTreeMap::from([
            ("Producer_0".to_string(), Coor::slot(0, 0).region_name()),
            ("Consumer_0".to_string(), Coor::slot(1, 0).region_name()),
        ]);
        graph
            .materialize_co_locations(&mut regions)
            .expect("FIFO aliases destination");

        assert_eq!(regions["q32_Top"], regions["Consumer_0"]);
        assert_eq!(regions["q64_Top"], regions["Consumer_0"]);
        assert_eq!(graph.placement_edges().len(), 1);
        assert_eq!(graph.placement_edges()[0].width, 35 + 67);
        let streams = cross_slot_streams(&graph, &regions).expect("cross-slot streams");
        assert_eq!(streams.len(), 2);
        assert!(streams
            .iter()
            .all(|stream| (stream.src, stream.dst) == ((0, 0), (1, 0))));
        assert!(streams
            .iter()
            .any(|stream| stream.link == "q32_Top" && stream.width == 35));
        assert!(streams
            .iter()
            .any(|stream| stream.link == "q64_Top" && stream.width == 67));
    }

    #[test]
    fn single_scheme_levels_by_intermediate_slots() {
        let route = vec![(0, 0), (0, 1), (0, 2)];
        assert_eq!(
            pipeline_level(&route, PipelineScheme::Single),
            1,
            "one intermediate"
        );
    }

    #[test]
    fn double_scheme_two_per_hop() {
        let route = vec![(0, 0), (0, 1), (0, 2)];
        assert_eq!(
            pipeline_level(&route, PipelineScheme::Double),
            4,
            "two hops * 2"
        );
    }

    #[test]
    fn single_h_double_v_weights_vertical_hops() {
        // One vertical hop (y changes) then one horizontal hop (x changes).
        let route = vec![(0, 0), (0, 1), (1, 1)];
        assert_eq!(
            pipeline_level(&route, PipelineScheme::SingleHDoubleV),
            3,
            "2 + 1"
        );
    }

    #[test]
    fn double_regions_put_one_body_cell_on_each_side_of_every_boundary() {
        let route = vec![(0, 0), (0, 1), (0, 2)];
        assert_eq!(
            pipeline_reg_regions(&route, PipelineScheme::Double),
            ["SLOT_X0Y0", "SLOT_X0Y1", "SLOT_X0Y1", "SLOT_X0Y2"]
        );
    }

    #[test]
    fn adjacent_single_crossing_has_head_and_tail_but_no_body() {
        let route = [(0, 0), (1, 0)];
        assert_eq!(pipeline_level(&route, PipelineScheme::Single), 0);
        assert!(pipeline_reg_regions(&route, PipelineScheme::Single).is_empty());
    }

    #[test]
    fn hybrid_horizontal_crossing_has_no_body() {
        let route = [(0, 0), (1, 0)];
        assert_eq!(pipeline_level(&route, PipelineScheme::SingleHDoubleV), 0);
        assert!(pipeline_reg_regions(&route, PipelineScheme::SingleHDoubleV).is_empty());
    }

    #[test]
    fn hybrid_duplicates_slots_incident_to_vertical_hops() {
        let route = [(0, 0), (1, 0), (1, 1), (0, 1)];
        assert_eq!(
            pipeline_reg_regions(&route, PipelineScheme::SingleHDoubleV),
            ["SLOT_X1Y0", "SLOT_X1Y0", "SLOT_X1Y1", "SLOT_X1Y1"]
        );
    }
}
