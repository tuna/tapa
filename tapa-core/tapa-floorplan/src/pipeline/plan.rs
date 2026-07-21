//! The pipeline plan.
//!
//! From a placement, find the stream channels that cross slot boundaries,
//! route them, and turn each route into a [`Crossing`] (register `level`,
//! distribution `scheme`, and per-register slot regions).
//!
//! Level formulas are ported from RapidStream's `gen_pp_template.py`: `Single`
//! places one body register per intermediate slot, `Double` two per hop, and
//! `SingleHDoubleV` one per horizontal hop and two per vertical (SLR) hop.

use std::collections::BTreeMap;

use tapa_ir::{Crossing, CrossingKind, PipelineScheme};

use crate::device::model::{Coor, Device};
use crate::graph::{FloorGraph, VertexKind};
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

/// The number of pipeline register stages a route implies under `scheme`.
#[must_use]
pub fn pipeline_level(route: &[Cell], scheme: PipelineScheme) -> u32 {
    let slots = route.len();
    match scheme {
        PipelineScheme::Single => u32::try_from(slots.saturating_sub(2)).unwrap_or(0),
        PipelineScheme::Double => u32::try_from(slots.saturating_sub(1) * 2).unwrap_or(0),
        PipelineScheme::SingleHDoubleV => route
            .windows(2)
            .map(|hop| if hop[0].1 == hop[1].1 { 1 } else { 2 })
            .sum(),
    }
}

/// Assign each of `level` body registers a slot along `route`, spread from the
/// first intermediate slot toward the destination.
#[must_use]
pub fn reg_regions(route: &[Cell], level: u32) -> Vec<String> {
    if level == 0 || route.is_empty() {
        return Vec::new();
    }
    let n = route.len();
    let level = level as usize;
    (0..level)
        .map(|j| {
            let idx = (1 + j * n / level).min(n - 1);
            slot_tag(route[idx])
        })
        .collect()
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

/// Find every internal FIFO whose producer and consumer landed in different
/// regions — those are the stream channels that must be pipelined.
fn cross_slot_streams(
    graph: &FloorGraph,
    regions: &BTreeMap<String, String>,
) -> Result<Vec<StreamNet>, PipelineError> {
    let mut nets = Vec::new();
    for (fi, fifo) in graph.vertices().iter().enumerate() {
        if fifo.kind != VertexKind::Fifo {
            continue;
        }
        // producer -> fifo -> consumer, from the FloorGraph edges.
        let producer = graph.edges().iter().find(|e| e.dst == fi);
        let consumer = graph.edges().iter().find(|e| e.src == fi);
        let (Some(producer), Some(consumer)) = (producer, consumer) else {
            continue; // one endpoint is a top-level port, not a placed instance
        };
        let src_region = regions
            .get(&graph.vertex(producer.src).name)
            .ok_or_else(|| PipelineError::BadRegion(graph.vertex(producer.src).name.clone()))?;
        let dst_region = regions
            .get(&graph.vertex(consumer.dst).name)
            .ok_or_else(|| PipelineError::BadRegion(graph.vertex(consumer.dst).name.clone()))?;
        if src_region == dst_region {
            continue; // co-located: no crossing
        }
        nets.push(StreamNet {
            link: fifo.name.clone(),
            src: region_cell(src_region)?,
            dst: region_cell(dst_region)?,
            width: producer.width,
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
            let level = pipeline_level(&route, scheme);
            Crossing {
                kind: CrossingKind::Stream,
                link: net.link.clone(),
                route: route.iter().map(|&cell| slot_tag(cell)).collect(),
                level,
                scheme,
                reg_regions: reg_regions(&route, level),
            }
        })
        .collect();
    Ok(crossings)
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn reg_regions_spread_along_the_route() {
        let route = vec![(0, 0), (0, 1), (0, 2)];
        let regions = reg_regions(&route, 2);
        assert_eq!(regions.len(), 2, "one region per body register");
        for region in &regions {
            assert!(region.starts_with("SLOT_X0Y"), "on the route: {region}");
        }
    }

    #[test]
    fn zero_level_has_no_registers() {
        assert!(reg_regions(&[(0, 0), (0, 1)], 0).is_empty());
    }
}
