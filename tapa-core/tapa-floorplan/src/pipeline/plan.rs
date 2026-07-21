//! The pipeline plan.
//!
//! From a placement, find the stream channels that cross slot boundaries,
//! route them, and turn each path into a typed [`PipelineRoute`] with a
//! distribution scheme and exact per-register slot regions.
//!
//! `Single` places one body register per intermediate slot, `Double` two per
//! hop, and `SingleHDoubleV` one per horizontal hop and two per vertical (SLR)
//! hop.

use std::collections::BTreeMap;

use tapa_ir::{Area, PipelineRoute, PipelineScheme, RoutedChannel};

use crate::device::model::{Coor, Device, Resource};
use crate::graph::{fifo_area, FloorGraph};
use crate::partition::ilp::scaled_area;
#[cfg(test)]
use crate::partition::ilp::MAX_USAGE_LIMIT;
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
    /// Generated pipeline metadata violated an accounting invariant.
    #[error("cannot account for generated pipeline `{link}`: {detail}")]
    Accounting { link: String, detail: String },
    /// Realized pipeline resources exceed the placement retry envelope.
    #[error(
        "generated pipelines use {used} {resource} in {region}, exceeding the placement capacity limit of {limit}; try a lower --usage-limit, a lighter --pp-scheme, or another --partition-strategy"
    )]
    RealizedCapacity {
        region: String,
        resource: &'static str,
        used: u64,
        limit: u64,
    },
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
    parse_region_or_slot(region)
        .filter(|coor| coor.width() == 1 && coor.height() == 1)
        .map(|coor| (coor.dl_x, coor.dl_y))
        .ok_or_else(|| PipelineError::BadRegion(region.to_string()))
}

fn parse_region_or_slot(region: &str) -> Option<Coor> {
    if let Some(coor) = Coor::from_region_name(region) {
        return Some(coor);
    }
    let rest = region.strip_prefix("SLOT_X")?;
    let (x, y) = rest.split_once('Y')?;
    Some(Coor::slot(x.parse().ok()?, y.parse().ok()?))
}

fn canonical_slot_region(region: &str) -> Result<String, PipelineError> {
    let (x, y) = region_cell(region)?;
    Ok(Coor::slot(x, y).region_name())
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

/// Plan every cross-slot stream route for a placed design.
pub fn plan_routes(
    graph: &FloorGraph,
    regions: &BTreeMap<String, String>,
    device: &Device,
    scheme: PipelineScheme,
    solver: &dyn Solver,
    opts: &SolveOpts,
) -> Result<Vec<PipelineRoute>, PipelineError> {
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

    let pipeline_routes = streams
        .iter()
        .zip(routes)
        .map(|(net, route)| {
            let reg_regions = pipeline_reg_regions(&route, scheme);
            PipelineRoute {
                channel: RoutedChannel::Stream {
                    fifo: net.link.clone(),
                },
                route: route.iter().map(|&cell| slot_tag(cell)).collect(),
                scheme,
                reg_regions,
            }
        })
        .collect();
    Ok(pipeline_routes)
}

/// Replace pre-routing FIFO estimates with the resources of the generated
/// Head/Body/Tail pipelines, then enforce the placement retry envelope.
pub(crate) fn realize_slot_usage(
    graph: &FloorGraph,
    regions: &BTreeMap<String, String>,
    baseline: &BTreeMap<String, Area>,
    routes: &[PipelineRoute],
    device: &Device,
    capacity_limit: f64,
) -> Result<BTreeMap<String, Area>, PipelineError> {
    let streams: BTreeMap<&str, _> = graph
        .streams()
        .iter()
        .map(|stream| (stream.link.as_str(), stream))
        .collect();
    let mut usage = baseline.clone();

    for route in routes {
        let RoutedChannel::Stream { fifo } = &route.channel else {
            continue;
        };
        let stream = streams
            .get(fifo.as_str())
            .ok_or_else(|| PipelineError::Accounting {
                link: fifo.clone(),
                detail: "no matching stream metadata".to_string(),
            })?;
        account_stream_pipeline(&mut usage, graph, regions, route, stream, fifo)?;
    }

    validate_realized_usage(&usage, device, capacity_limit)?;
    Ok(usage)
}

fn account_stream_pipeline(
    usage: &mut BTreeMap<String, Area>,
    graph: &FloorGraph,
    regions: &BTreeMap<String, String>,
    route: &PipelineRoute,
    stream: &crate::graph::Stream,
    fifo: &str,
) -> Result<(), PipelineError> {
    let head_region = route
        .route
        .first()
        .ok_or_else(|| accounting_error(fifo, "route has no Head region"))?;
    let tail_region = route
        .route
        .last()
        .ok_or_else(|| accounting_error(fifo, "route has no Tail region"))?;
    let head_region = canonical_slot_region(head_region)?;
    let tail_region = canonical_slot_region(tail_region)?;
    verify_route_endpoint(graph, regions, stream.src, &head_region, fifo, "Head")?;
    verify_route_endpoint(graph, regions, stream.dst, &tail_region, fifo, "Tail")?;

    let original_fifo = fifo_area(stream.data_width, stream.depth);
    subtract_usage(usage, &tail_region, original_fifo, fifo)?;

    let body_level = u32::try_from(route.reg_regions.len())
        .map_err(|_| accounting_error(fifo, "Body level exceeds u32"))?;
    let extra_depth = body_level
        .checked_mul(2)
        .and_then(|level| level.checked_add(6))
        .ok_or_else(|| accounting_error(fifo, "Tail depth overflows u32"))?;
    let real_depth = stream
        .depth
        .checked_add(extra_depth)
        .ok_or_else(|| accounting_error(fifo, "Tail depth overflows u32"))?;
    add_usage(
        usage,
        &tail_region,
        fifo_area(stream.data_width, real_depth),
        fifo,
    )?;

    // Head and each Body explicitly register ready, valid, and DATA_WIDTH
    // data bits. The Head gate's valid qualification is the only additional
    // combinational expression with a defensible LUT cost.
    let register_width = stream
        .data_width
        .checked_add(2)
        .filter(|width| *width == stream.width)
        .ok_or_else(|| accounting_error(fifo, "inconsistent stream width metadata"))?;
    let register_area = Area {
        ff: u64::from(register_width),
        ..Area::default()
    };
    add_usage(
        usage,
        &head_region,
        Area {
            lut: 1,
            ..register_area
        },
        fifo,
    )?;
    for region in &route.reg_regions {
        add_usage(usage, &canonical_slot_region(region)?, register_area, fifo)?;
    }
    Ok(())
}

fn verify_route_endpoint(
    graph: &FloorGraph,
    regions: &BTreeMap<String, String>,
    vertex: usize,
    route_region: &str,
    link: &str,
    endpoint: &str,
) -> Result<(), PipelineError> {
    let vertex_name = &graph.vertex(vertex).name;
    let placed_region = regions
        .get(vertex_name)
        .ok_or_else(|| accounting_error(link, &format!("missing region for `{vertex_name}`")))?;
    let placed_region = canonical_slot_region(placed_region)?;
    if placed_region != route_region {
        return Err(accounting_error(
            link,
            &format!("{endpoint} route region `{route_region}` differs from `{placed_region}`"),
        ));
    }
    Ok(())
}

fn accounting_error(link: &str, detail: &str) -> PipelineError {
    PipelineError::Accounting {
        link: link.to_string(),
        detail: detail.to_string(),
    }
}

fn subtract_usage(
    usage: &mut BTreeMap<String, Area>,
    region: &str,
    area: Area,
    link: &str,
) -> Result<(), PipelineError> {
    let current = usage
        .get_mut(region)
        .ok_or_else(|| accounting_error(link, &format!("missing baseline usage for `{region}`")))?;
    *current = checked_sub_area(*current, area).ok_or_else(|| {
        accounting_error(
            link,
            &format!("baseline usage in `{region}` does not contain its FIFO estimate"),
        )
    })?;
    Ok(())
}

fn add_usage(
    usage: &mut BTreeMap<String, Area>,
    region: &str,
    area: Area,
    link: &str,
) -> Result<(), PipelineError> {
    let current = usage.entry(region.to_string()).or_default();
    *current = checked_add_area(*current, area).ok_or_else(|| {
        accounting_error(link, &format!("resource count overflows in `{region}`"))
    })?;
    Ok(())
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

fn checked_sub_area(lhs: Area, rhs: Area) -> Option<Area> {
    Some(Area {
        lut: lhs.lut.checked_sub(rhs.lut)?,
        ff: lhs.ff.checked_sub(rhs.ff)?,
        bram_18k: lhs.bram_18k.checked_sub(rhs.bram_18k)?,
        dsp: lhs.dsp.checked_sub(rhs.dsp)?,
        uram: lhs.uram.checked_sub(rhs.uram)?,
    })
}

fn validate_realized_usage(
    usage: &BTreeMap<String, Area>,
    device: &Device,
    capacity_limit: f64,
) -> Result<(), PipelineError> {
    for (region, used_area) in usage {
        let coor = parse_region_or_slot(region)
            .filter(|coor| coor.width() == 1 && coor.height() == 1)
            .ok_or_else(|| PipelineError::BadRegion(region.clone()))?;
        let capacity = device
            .island_area(&coor)
            .ok_or_else(|| PipelineError::BadRegion(region.clone()))?;
        let limit = scaled_area(capacity, capacity_limit);
        for resource in Resource::ALL {
            let used = resource.amount(used_area);
            let allowed = resource.amount(&limit);
            if used > allowed {
                return Err(PipelineError::RealizedCapacity {
                    region: region.clone(),
                    resource: resource.name(),
                    used,
                    limit: allowed,
                });
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::select::select_device;

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

    fn one_stream_graph(depth: u32, producer_area: Area, consumer_area: Area) -> FloorGraph {
        let design = serde_json::json!({
            "cflags": [], "top": "Top", "target": "xilinx-hls",
            "tasks": {
                "Top": {
                    "readable_name": "Top", "code": "void Top() {}",
                    "level": "upper", "synth": "hls", "ports": [],
                    "tasks": {
                        "Producer": [{"args": {"out": {"arg": "q", "cat": "ostream"}}, "step": 0}],
                        "Consumer": [{"args": {"in": {"arg": "q", "cat": "istream"}}, "step": 0}]
                    },
                    "fifos": {
                        "q": {"depth": depth, "produced_by": ["Producer", 0], "consumed_by": ["Consumer", 0]}
                    }
                },
                "Producer": {
                    "readable_name": "Producer", "code": "void Producer() {}",
                    "level": "lower", "synth": "hls",
                    "ports": [{"cat": "ostream", "name": "out", "type": "int", "width": 32}],
                    "self_area": {"LUT": producer_area.lut, "FF": producer_area.ff}
                },
                "Consumer": {
                    "readable_name": "Consumer", "code": "void Consumer() {}",
                    "level": "lower", "synth": "hls",
                    "ports": [{"cat": "istream", "name": "in", "type": "int", "width": 32}],
                    "self_area": {"LUT": consumer_area.lut, "FF": consumer_area.ff}
                }
            }
        });
        let design = tapa_ir::TaskGraph::from_json(&design.to_string()).expect("parse graph");
        let flat = tapa_ir::flatten(&design).expect("flatten graph");
        FloorGraph::build(&flat).expect("floor graph")
    }

    fn baseline_usage(
        graph: &FloorGraph,
        regions: &BTreeMap<String, String>,
    ) -> BTreeMap<String, Area> {
        let mut usage = BTreeMap::new();
        for vertex in graph.vertices() {
            let region = regions.get(&vertex.name).expect("vertex region").clone();
            let entry = usage.entry(region).or_default();
            *entry = checked_add_area(*entry, vertex.area).expect("test area fits");
        }
        usage
    }

    fn stream_route(route: &[&str], reg_regions: &[&str]) -> PipelineRoute {
        PipelineRoute {
            channel: RoutedChannel::Stream {
                fifo: "q_Top".to_string(),
            },
            route: route.iter().map(ToString::to_string).collect(),
            scheme: PipelineScheme::Single,
            reg_regions: reg_regions.iter().map(ToString::to_string).collect(),
        }
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
    fn realized_usage_replaces_fifo_and_places_pipeline_registers() {
        let graph = one_stream_graph(
            120,
            Area {
                lut: 10,
                ff: 20,
                ..Area::default()
            },
            Area {
                lut: 50,
                ff: 60,
                ..Area::default()
            },
        );
        let source = Coor::slot(0, 0).region_name();
        let body = Coor::slot(0, 1).region_name();
        let destination = Coor::slot(0, 2).region_name();
        let regions = BTreeMap::from([
            ("Producer_0".to_string(), source.clone()),
            ("Consumer_0".to_string(), destination.clone()),
        ]);
        let baseline = baseline_usage(&graph, &regions);
        let route = stream_route(&["SLOT_X0Y0", "SLOT_X0Y1", "SLOT_X0Y2"], &["SLOT_X0Y1"]);

        let realized = realize_slot_usage(
            &graph,
            &regions,
            &baseline,
            &[route],
            &select_device("u280").expect("u280"),
            MAX_USAGE_LIMIT,
        )
        .expect("realized usage");

        assert_eq!(
            fifo_area(33, 128),
            Area {
                lut: 33,
                bram_18k: 1,
                ..Area::default()
            },
            "depth 120 plus one Body crosses the Tail's BRAM threshold"
        );
        assert_eq!(
            realized[&source],
            Area {
                lut: 11,
                ff: 55,
                ..Area::default()
            },
            "source task plus one Head gate and 35 Head registers"
        );
        assert_eq!(
            realized[&body],
            Area {
                ff: 35,
                ..Area::default()
            }
        );
        assert_eq!(
            realized[&destination],
            Area {
                lut: 83,
                ff: 60,
                bram_18k: 1,
                ..Area::default()
            },
            "destination task plus the expanded Tail, without the old FIFO"
        );
    }

    #[test]
    fn realized_usage_fails_above_the_retry_envelope() {
        let graph = one_stream_graph(2, Area::default(), Area::default());
        let device = select_device("u280").expect("u280");
        let source = Coor::slot(0, 0).region_name();
        let destination = Coor::slot(1, 0).region_name();
        let regions = BTreeMap::from([
            ("Producer_0".to_string(), source.clone()),
            ("Consumer_0".to_string(), destination),
        ]);
        let mut baseline = baseline_usage(&graph, &regions);
        let ff_limit = scaled_area(
            device.slot(0, 0).expect("source slot").area,
            MAX_USAGE_LIMIT,
        )
        .ff;
        baseline.entry(source.clone()).or_default().ff = ff_limit - 34;
        let route = stream_route(&["SLOT_X0Y0", "SLOT_X1Y0"], &[]);

        let error = realize_slot_usage(
            &graph,
            &regions,
            &baseline,
            &[route],
            &device,
            MAX_USAGE_LIMIT,
        )
        .expect_err("the Head exceeds the FF envelope");
        assert!(matches!(
            error,
            PipelineError::RealizedCapacity {
                ref region,
                resource: "FF",
                used,
                limit,
            } if region == &source && used == ff_limit + 1 && limit == ff_limit
        ));
        assert!(error.to_string().contains("--usage-limit"));
    }

    #[test]
    fn non_crossing_fifo_usage_is_unchanged() {
        let graph = one_stream_graph(120, Area::default(), Area::default());
        let region = Coor::slot(0, 0).region_name();
        let regions = BTreeMap::from([
            ("Producer_0".to_string(), region.clone()),
            ("Consumer_0".to_string(), region),
        ]);
        let baseline = baseline_usage(&graph, &regions);

        let realized = realize_slot_usage(
            &graph,
            &regions,
            &baseline,
            &[],
            &select_device("u280").expect("u280"),
            MAX_USAGE_LIMIT,
        )
        .expect("non-crossing usage");
        assert_eq!(realized, baseline);
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
