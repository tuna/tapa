//! The pipeline plan.
//!
//! From a placement, find the stream channels that cross slot boundaries,
//! route them, and turn each path into a typed [`PipelineRoute`] with a
//! distribution scheme and exact per-register slot regions.
//!
//! `Single` places one Body register per intermediate slot, `Double` two per
//! hop, and `SingleHDoubleV` one per intermediate slot horizontally — the
//! separately generated Head and Tail cover the first and last hop — plus,
//! for every vertical (SLR) hop, one extra Body register in each of the hop's
//! two endpoint slots, i.e. two per vertical hop; a slot shared by two
//! vertical hops hosts one register per hop.

use std::collections::BTreeMap;

use tapa_ir::{
    floorplanned_fifo_storage_depth, Area, PipelineRoute, PipelineScheme, RoutedChannel,
};

use crate::device::model::{Coor, Device, Resource};
use crate::graph::{fifo_area, FloorGraph};
use crate::partition::ilp::scaled_area;
#[cfg(test)]
use crate::partition::ilp::MAX_USAGE_LIMIT;
use crate::route::ilp::{route_nets, slot_tag, RegisterBudget, RouteError, RouteNet};
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

/// Return the exact ordered Body-cell regions for `route` and `scheme`.
///
/// - Single uses only the intermediate route slots.
/// - Double places one cell on each side of every boundary.
/// - Single-H/Double-V duplicates every route slot incident to a vertical
///   boundary, then removes the first Head and last Tail entries.
#[must_use]
pub fn pipeline_reg_regions(route: &[Cell], scheme: PipelineScheme) -> Vec<String> {
    pipeline_reg_cells(route, scheme)
        .into_iter()
        .map(slot_tag)
        .collect()
}

/// [`pipeline_reg_regions`] as grid cells, for the routing budget that has to
/// count how many Body registers a candidate path would leave in each slot.
fn pipeline_reg_cells(route: &[Cell], scheme: PipelineScheme) -> Vec<Cell> {
    if route.len() < 2 {
        return Vec::new();
    }

    match scheme {
        PipelineScheme::Single => route[1..route.len() - 1].to_vec(),
        PipelineScheme::Double => route.windows(2).flat_map(|hop| [hop[0], hop[1]]).collect(),
        PipelineScheme::SingleHDoubleV => {
            let mut multiplicity = vec![1usize; route.len()];
            for (index, hop) in route.windows(2).enumerate() {
                if hop[0].1 != hop[1].1 {
                    multiplicity[index] = 2;
                    multiplicity[index + 1] = 2;
                }
            }
            let expanded: Vec<Cell> = route
                .iter()
                .zip(multiplicity)
                .flat_map(|(&cell, count)| std::iter::repeat_n(cell, count))
                .collect();
            expanded[1..expanded.len() - 1].to_vec()
        }
    }
}

/// Parse a single-slot region tag into its grid cell.
fn region_cell(region: &str) -> Result<Cell, PipelineError> {
    Coor::from_atomic_region_name(region)
        .map(|coor| (coor.dl_x, coor.dl_y))
        .ok_or_else(|| PipelineError::BadRegion(region.to_string()))
}

fn canonical_slot_region(region: &str) -> Result<String, PipelineError> {
    let (x, y) = region_cell(region)?;
    Ok(Coor::slot(x, y).region_name())
}

/// A cross-slot typed channel awaiting the shared routing solve.
struct PendingNet {
    channel: RoutedChannel,
    /// Additional typed channels intentionally carried over this exact solved
    /// path. Launch and reset share one forward routing degree of freedom.
    shared_channels: Vec<RoutedChannel>,
    src: Cell,
    dst: Cell,
    width: u32,
}

/// The slot cells an edge's endpoints landed in, or `None` when both landed
/// in the same slot — co-located, so nothing crosses.
fn crossing_cells(
    graph: &FloorGraph,
    regions: &BTreeMap<String, String>,
    src: usize,
    dst: usize,
) -> Result<Option<(Cell, Cell)>, PipelineError> {
    let region_of = |vertex: usize| {
        let name = &graph.vertex(vertex).name;
        regions
            .get(name)
            .ok_or_else(|| PipelineError::BadRegion(name.clone()))
    };
    let (src_region, dst_region) = (region_of(src)?, region_of(dst)?);
    if src_region == dst_region {
        return Ok(None);
    }
    Ok(Some((region_cell(src_region)?, region_cell(dst_region)?)))
}

/// Find every stream and AXI channel whose endpoints landed in different
/// slots. All channel classes enter one routing MILP so they compete for the
/// same physical boundary capacities.
fn cross_slot_nets(
    graph: &FloorGraph,
    regions: &BTreeMap<String, String>,
) -> Result<Vec<PendingNet>, PipelineError> {
    let mut nets = Vec::new();
    for edge in graph.streams() {
        let Some((src, dst)) = crossing_cells(graph, regions, edge.src, edge.dst)? else {
            continue;
        };
        nets.push(PendingNet {
            channel: RoutedChannel::Stream {
                fifo: edge.link.clone(),
            },
            shared_channels: Vec::new(),
            src,
            dst,
            width: edge.width,
        });
    }
    for edge in graph.axi_nets() {
        let Some((src, dst)) = crossing_cells(graph, regions, edge.src, edge.dst)? else {
            continue;
        };
        nets.push(PendingNet {
            channel: RoutedChannel::Axi {
                endpoint: edge.endpoint.clone(),
                bank: edge.bank,
                channel: edge.channel,
            },
            shared_channels: Vec::new(),
            src,
            dst,
            width: edge.width,
        });
    }
    for edge in graph.control_nets() {
        if edge.channel == tapa_ir::ControlChannel::Reset {
            continue; // emitted with the matching Launch route below
        }
        let Some((src, dst)) = crossing_cells(graph, regions, edge.src, edge.dst)? else {
            continue;
        };

        let mut shared_channels = Vec::new();
        let width = if edge.channel == tapa_ir::ControlChannel::Launch {
            let reset = graph
                .control_nets()
                .iter()
                .find(|candidate| {
                    candidate.instance == edge.instance
                        && candidate.channel == tapa_ir::ControlChannel::Reset
                        && candidate.src == edge.src
                        && candidate.dst == edge.dst
                })
                .ok_or_else(|| PipelineError::Accounting {
                    link: edge.instance.clone(),
                    detail: "Launch control has no matching Reset channel".to_string(),
                })?;
            shared_channels.push(RoutedChannel::Control {
                instance: reset.instance.clone(),
                channel: reset.channel,
            });
            edge.width
                .checked_add(reset.width)
                .ok_or_else(|| PipelineError::Accounting {
                    link: edge.instance.clone(),
                    detail: "combined Launch/Reset routing width overflows u32".to_string(),
                })?
        } else {
            edge.width
        };
        nets.push(PendingNet {
            channel: RoutedChannel::Control {
                instance: edge.instance.clone(),
                channel: edge.channel,
            },
            shared_channels,
            src,
            dst,
            width,
        });
    }
    Ok(nets)
}

/// Plan every cross-slot stream and AXI route for a placed design.
///
/// `baseline` is the placed-but-unrouted per-slot usage and `logic_capacity_limit`
/// the derating it was placed under; together they give the router the
/// flip-flops each slot can still spend on Body registers.
#[allow(
    clippy::too_many_arguments,
    reason = "routing needs the placement, the device, the scheme, and the resource envelope it must route inside"
)]
pub fn plan_routes(
    graph: &FloorGraph,
    regions: &BTreeMap<String, String>,
    baseline: &BTreeMap<String, Area>,
    device: &Device,
    scheme: PipelineScheme,
    logic_capacity_limit: f64,
    solver: &dyn Solver,
    opts: &SolveOpts,
) -> Result<Vec<PipelineRoute>, PipelineError> {
    let nets = cross_slot_nets(graph, regions)?;
    if nets.is_empty() {
        return Ok(Vec::new());
    }

    let route_nets_input: Vec<RouteNet> = nets
        .iter()
        .map(|net| RouteNet {
            src: net.src,
            dst: net.dst,
            width: net.width,
        })
        .collect();
    let bodies = |path: &[Cell]| pipeline_reg_cells(path, scheme);
    let budget = RegisterBudget {
        bodies: &bodies,
        available: register_budgets(baseline, device, logic_capacity_limit),
    };
    let routes = route_nets(&route_nets_input, device, Some(&budget), solver, opts)?;

    let mut pipeline_routes = Vec::new();
    for (net, route) in nets.iter().zip(routes) {
        let reg_regions = pipeline_reg_regions(&route, scheme);
        let route: Vec<String> = route.iter().map(|&cell| slot_tag(cell)).collect();
        for channel in std::iter::once(&net.channel).chain(&net.shared_channels) {
            pipeline_routes.push(PipelineRoute {
                channel: channel.clone(),
                route: route.clone(),
                scheme,
                reg_regions: reg_regions.clone(),
            });
        }
    }
    Ok(pipeline_routes)
}

/// The flip-flops each slot may still spend on routed Body registers.
///
/// Deliberately generous: it subtracts the placed usage but not the Head
/// registers or the routed Tail's growth, whose costs the endpoints already fix
/// and which no path choice can move. A budget that is too tight would reject
/// routes that realize fine; one that is merely loose still steers the router
/// away from slots that are full, and [`realize_slot_usage`] remains the exact
/// check.
fn register_budgets(
    baseline: &BTreeMap<String, Area>,
    device: &Device,
    logic_capacity_limit: f64,
) -> BTreeMap<Cell, u64> {
    device
        .slots
        .iter()
        .map(|slot| {
            let capacity = scaled_area(slot.area, logic_capacity_limit).ff;
            let placed = baseline
                .get(&slot.coor().region_name())
                .map_or(0, |area| area.ff);
            ((slot.x, slot.y), capacity.saturating_sub(placed))
        })
        .collect()
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
    realize_slot_usage_with_resource_caps(
        graph,
        regions,
        baseline,
        routes,
        device,
        capacity_limit,
        capacity_limit,
    )
}

/// Realize generated resources with independent logic and block-resource caps.
pub(crate) fn realize_slot_usage_with_resource_caps(
    graph: &FloorGraph,
    regions: &BTreeMap<String, String>,
    baseline: &BTreeMap<String, Area>,
    routes: &[PipelineRoute],
    device: &Device,
    logic_capacity_limit: f64,
    block_capacity_limit: f64,
) -> Result<BTreeMap<String, Area>, PipelineError> {
    let streams: BTreeMap<&str, _> = graph
        .streams()
        .iter()
        .map(|stream| (stream.link.as_str(), stream))
        .collect();
    let mut usage = baseline.clone();

    for route in routes {
        match &route.channel {
            RoutedChannel::Stream { fifo } => {
                let stream =
                    streams
                        .get(fifo.as_str())
                        .ok_or_else(|| PipelineError::Accounting {
                            link: fifo.clone(),
                            detail: "no matching stream metadata".to_string(),
                        })?;
                account_stream_pipeline(&mut usage, graph, regions, route, stream, fifo)?;
            }
            RoutedChannel::Axi {
                endpoint,
                bank,
                channel,
            } => {
                let link = format!("{}.{} {channel:?}", endpoint.instance, endpoint.port);
                let net = graph
                    .axi_nets()
                    .iter()
                    .find(|net| {
                        net.endpoint == *endpoint && net.bank == *bank && net.channel == *channel
                    })
                    .ok_or_else(|| accounting_error(&link, "no matching AXI channel metadata"))?;
                account_axi_pipeline(&mut usage, graph, regions, route, net, &link)?;
            }
            RoutedChannel::Control { instance, channel } => {
                let link = format!("{instance} {}", channel.rtl_name());
                let net = graph
                    .control_nets()
                    .iter()
                    .find(|net| net.instance == *instance && net.channel == *channel)
                    .ok_or_else(|| accounting_error(&link, "no matching control metadata"))?;
                account_control_pipeline(&mut usage, graph, regions, route, net, &link)?;
            }
        }
    }

    validate_realized_usage(&usage, device, logic_capacity_limit, block_capacity_limit)?;
    Ok(usage)
}

fn account_control_pipeline(
    usage: &mut BTreeMap<String, Area>,
    graph: &FloorGraph,
    regions: &BTreeMap<String, String>,
    route: &PipelineRoute,
    net: &crate::graph::ControlNet,
    link: &str,
) -> Result<(), PipelineError> {
    let head_region = route
        .route
        .first()
        .ok_or_else(|| accounting_error(link, "route has no Head region"))?;
    let tail_region = route
        .route
        .last()
        .ok_or_else(|| accounting_error(link, "route has no Tail region"))?;
    let head_region = canonical_slot_region(head_region)?;
    let tail_region = canonical_slot_region(tail_region)?;
    verify_route_endpoint(graph, regions, net.src, &head_region, link, "Head")?;
    verify_route_endpoint(graph, regions, net.dst, &tail_region, link, "Tail")?;

    let register_area = Area {
        ff: u64::from(net.width),
        ..Area::default()
    };
    add_usage(usage, &head_region, register_area, link)?;
    for region in &route.reg_regions {
        add_usage(usage, &canonical_slot_region(region)?, register_area, link)?;
    }
    add_usage(usage, &tail_region, register_area, link)?;
    Ok(())
}

fn account_axi_pipeline(
    usage: &mut BTreeMap<String, Area>,
    graph: &FloorGraph,
    regions: &BTreeMap<String, String>,
    route: &PipelineRoute,
    net: &crate::graph::AxiNet,
    link: &str,
) -> Result<(), PipelineError> {
    let head_region = route
        .route
        .first()
        .ok_or_else(|| accounting_error(link, "route has no Head region"))?;
    let tail_region = route
        .route
        .last()
        .ok_or_else(|| accounting_error(link, "route has no Tail region"))?;
    let head_region = canonical_slot_region(head_region)?;
    let tail_region = canonical_slot_region(tail_region)?;
    verify_route_endpoint(graph, regions, net.src, &head_region, link, "Head")?;
    verify_route_endpoint(graph, regions, net.dst, &tail_region, link, "Tail")?;

    let body_level = u32::try_from(route.reg_regions.len())
        .map_err(|_| accounting_error(link, "Body level exceeds u32"))?;
    let real_depth = body_level
        .checked_mul(2)
        .and_then(|level| level.checked_add(8))
        .ok_or_else(|| accounting_error(link, "Tail depth overflows u32"))?;
    add_usage(
        usage,
        &tail_region,
        fifo_area(net.payload_width, real_depth),
        link,
    )?;

    let register_area = Area {
        ff: u64::from(net.width),
        ..Area::default()
    };
    add_usage(
        usage,
        &head_region,
        Area {
            lut: 1,
            ..register_area
        },
        link,
    )?;
    for region in &route.reg_regions {
        add_usage(usage, &canonical_slot_region(region)?, register_area, link)?;
    }
    Ok(())
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

    let original_fifo = fifo_area(
        stream.data_width,
        floorplanned_fifo_storage_depth(stream.depth),
    );
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
    *current = current.checked_sub(area).ok_or_else(|| {
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
    *current = current.checked_add(area).ok_or_else(|| {
        accounting_error(link, &format!("resource count overflows in `{region}`"))
    })?;
    Ok(())
}

fn validate_realized_usage(
    usage: &BTreeMap<String, Area>,
    device: &Device,
    logic_capacity_limit: f64,
    block_capacity_limit: f64,
) -> Result<(), PipelineError> {
    for (region, used_area) in usage {
        let coor = Coor::from_region_or_slot_name(region)
            .filter(|coor| coor.width() == 1 && coor.height() == 1)
            .ok_or_else(|| PipelineError::BadRegion(region.clone()))?;
        let capacity = device
            .island_area(&coor)
            .ok_or_else(|| PipelineError::BadRegion(region.clone()))?;
        let logic_limit = scaled_area(capacity, logic_capacity_limit);
        let block_limit = scaled_area(capacity, block_capacity_limit);
        for resource in Resource::ALL {
            let used = resource.amount(used_area);
            let allowed = if resource.is_logic() {
                resource.amount(&logic_limit)
            } else {
                resource.amount(&block_limit)
            };
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
    use tapa_ir::global_controller_instance_name;

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
                    "self_area": {"lut": producer_area.lut, "ff": producer_area.ff}
                },
                "Consumer": {
                    "readable_name": "Consumer", "code": "void Consumer() {}",
                    "level": "lower", "synth": "hls",
                    "ports": [{"cat": "istream", "name": "in", "type": "int", "width": 32}],
                    "self_area": {"lut": consumer_area.lut, "ff": consumer_area.ff}
                }
            }
        });
        let design = tapa_ir::TaskGraph::from_json(&design.to_string()).expect("parse graph");
        let flat = tapa_ir::flatten(&design).expect("flatten graph");
        FloorGraph::build(&flat).expect("floor graph")
    }

    fn one_mmap_graph() -> FloorGraph {
        let design = tapa_ir::TaskGraph::from_json(
            r#"{
                "cflags": [], "top": "Top", "target": "xilinx-hls",
                "tasks": {
                    "Top": {"readable_name":"Top","code":"","level":"upper","synth":"hls",
                        "ports":[{"cat":"mmap","name":"mem","type":"int*","width":32}],
                        "tasks":{"Reader":[{"args":{"data":{"arg":"mem","cat":"mmap"}},"step":0}]},
                        "fifos":{}},
                    "Reader": {"readable_name":"Reader","code":"","level":"lower","synth":"hls",
                        "ports":[{"cat":"mmap","name":"data","type":"int*","width":32}],
                        "self_area":{"lut":10,"ff":20}}
                }
            }"#,
        )
        .expect("parse mmap graph");
        let flat = tapa_ir::flatten(&design).expect("flatten mmap graph");
        FloorGraph::build_with_memory(
            &flat,
            &[crate::graph::MemoryInterface {
                endpoint: tapa_ir::AxiEndpoint {
                    instance: "Reader_0".to_string(),
                    port: "data".to_string(),
                    top_port: "mem".to_string(),
                },
                bank: tapa_ir::MemoryBank {
                    kind: tapa_ir::MemoryKind::Hbm,
                    index: 0,
                },
                channel_widths: tapa_ir::AxiChannelWidths {
                    read_address: 80,
                    read_data: 38,
                    write_address: 80,
                    write_data: 39,
                    write_response: 5,
                },
                bridge_instance: None,
            }],
        )
        .expect("floor graph")
    }

    fn one_controlled_task_graph() -> FloorGraph {
        let design = tapa_ir::TaskGraph::from_json(
            r#"{
                "cflags": [], "top": "Top", "target": "xilinx-hls",
                "tasks": {
                    "Top": {"readable_name":"Top","code":"","level":"upper","synth":"hls",
                        "ports":[{"cat":"scalar","name":"n","type":"unsigned","width":32}],
                        "tasks":{"Worker":[{"name":"worker#0","args":{
                            "count":{"arg":"n","cat":"scalar"}
                        },"step":0}]},"fifos":{}},
                    "Worker": {"readable_name":"Worker","code":"","level":"lower","synth":"hls",
                        "ports":[{"cat":"scalar","name":"count","type":"unsigned","width":32}]}
                }
            }"#,
        )
        .expect("parse control graph");
        let flat = tapa_ir::flatten(&design).expect("flatten control graph");
        FloorGraph::build_with_interfaces(
            &flat,
            &[],
            Some(crate::graph::ControlInterface::default()),
            None,
        )
        .expect("floor graph")
    }

    fn baseline_usage(
        graph: &FloorGraph,
        regions: &BTreeMap<String, String>,
    ) -> BTreeMap<String, Area> {
        let mut usage: BTreeMap<String, Area> = BTreeMap::new();
        for vertex in graph.vertices() {
            let region = regions.get(&vertex.name).expect("vertex region").clone();
            let entry = usage.entry(region).or_default();
            *entry = entry.checked_add(vertex.area).expect("test area fits");
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
        let streams = cross_slot_nets(&graph, &regions).expect("cross-slot streams");
        assert_eq!(streams.len(), 2);
        assert!(streams
            .iter()
            .all(|stream| (stream.src, stream.dst) == ((0, 0), (1, 0))));
        assert!(streams.iter().any(|stream| {
            stream.channel
                == (RoutedChannel::Stream {
                    fifo: "q32_Top".to_string(),
                })
                && stream.width == 35
        }));
        assert!(streams.iter().any(|stream| {
            stream.channel
                == (RoutedChannel::Stream {
                    fifo: "q64_Top".to_string(),
                })
                && stream.width == 67
        }));
    }

    #[test]
    fn axi_channels_share_the_router_with_protocol_correct_directions() {
        let graph = one_mmap_graph();
        let regions = BTreeMap::from([
            ("Reader_0".to_string(), Coor::slot(1, 0).region_name()),
            (
                "__tapa_bank_hbm_0".to_string(),
                Coor::slot(0, 0).region_name(),
            ),
        ]);
        let nets = cross_slot_nets(&graph, &regions).expect("cross-slot AXI nets");
        assert_eq!(nets.len(), 5);
        for net in &nets {
            let RoutedChannel::Axi { channel, .. } = net.channel else {
                panic!("expected AXI net")
            };
            match channel {
                tapa_ir::AxiChannel::ReadAddress
                | tapa_ir::AxiChannel::WriteAddress
                | tapa_ir::AxiChannel::WriteData => {
                    assert_eq!((net.src, net.dst), ((1, 0), (0, 0)));
                }
                tapa_ir::AxiChannel::ReadData | tapa_ir::AxiChannel::WriteResponse => {
                    assert_eq!((net.src, net.dst), ((0, 0), (1, 0)));
                }
            }
        }

        let read_data = graph
            .axi_nets()
            .iter()
            .find(|net| net.channel == tapa_ir::AxiChannel::ReadData)
            .expect("read data");
        let route = PipelineRoute {
            channel: RoutedChannel::Axi {
                endpoint: read_data.endpoint.clone(),
                bank: read_data.bank,
                channel: read_data.channel,
            },
            route: vec!["SLOT_X0Y0".to_string(), "SLOT_X1Y0".to_string()],
            scheme: PipelineScheme::Single,
            reg_regions: Vec::new(),
        };
        let realized = realize_slot_usage(
            &graph,
            &regions,
            &BTreeMap::from([(
                Coor::slot(1, 0).region_name(),
                Area {
                    lut: 10,
                    ff: 20,
                    ..Area::default()
                },
            )]),
            &[route],
            &select_device("u280").expect("u280"),
            MAX_USAGE_LIMIT,
        )
        .expect("account AXI pipeline");
        assert_eq!(
            realized[&Coor::slot(0, 0).region_name()],
            Area {
                lut: 1,
                ff: 38,
                ..Area::default()
            },
            "the bank-side read-data Head is charged at the route source"
        );
        assert_eq!(
            realized[&Coor::slot(1, 0).region_name()],
            Area {
                lut: 10,
                ff: 20,
                ..Area::default()
            }
            .checked_add(fifo_area(36, 8))
            .expect("area fits"),
            "the child-side read-data Tail carries the two-entry AXI buffer plus grace"
        );
    }

    #[test]
    fn control_launch_and_reset_share_one_forward_router_net() {
        let graph = one_controlled_task_graph();
        let regions = BTreeMap::from([
            (
                global_controller_instance_name().to_string(),
                Coor::slot(0, 0).region_name(),
            ),
            ("worker#0".to_string(), Coor::slot(0, 2).region_name()),
        ]);
        let nets = cross_slot_nets(&graph, &regions).expect("control nets");
        assert_eq!(nets.len(), 2, "one forward bundle and one completion");
        assert_eq!(nets[0].width, 35, "34-bit launch plus one reset bit");
        assert_eq!(
            nets[0].channel,
            RoutedChannel::Control {
                instance: "worker#0".to_string(),
                channel: tapa_ir::ControlChannel::Launch,
            }
        );
        assert_eq!(
            nets[0].shared_channels,
            [RoutedChannel::Control {
                instance: "worker#0".to_string(),
                channel: tapa_ir::ControlChannel::Reset,
            }]
        );
        assert_eq!((nets[0].src, nets[0].dst), ((0, 0), (0, 2)));
        assert_eq!((nets[1].src, nets[1].dst), ((0, 2), (0, 0)));

        let colocated = BTreeMap::from([
            (
                global_controller_instance_name().to_string(),
                Coor::slot(1, 1).region_name(),
            ),
            ("worker#0".to_string(), Coor::slot(1, 1).region_name()),
        ]);
        assert!(
            cross_slot_nets(&graph, &colocated)
                .expect("co-located controls")
                .is_empty(),
            "co-located control remains a direct wire"
        );
    }

    #[test]
    fn published_launch_and_reset_routes_are_identical() {
        use crate::solver::{CbcSolver, SolverError};

        let graph = one_controlled_task_graph();
        let regions = BTreeMap::from([
            (
                global_controller_instance_name().to_string(),
                Coor::slot(0, 0).region_name(),
            ),
            ("worker#0".to_string(), Coor::slot(0, 2).region_name()),
        ]);
        let routes = match plan_routes(
            &graph,
            &regions,
            &BTreeMap::new(),
            &select_device("u280").expect("u280"),
            PipelineScheme::Single,
            MAX_USAGE_LIMIT,
            &CbcSolver::new(),
            &SolveOpts {
                threads: Some(1),
                ..SolveOpts::default()
            },
        ) {
            Ok(routes) => routes,
            Err(PipelineError::Route(RouteError::Solver(SolverError::Spawn { .. }))) => {
                crate::solver::missing_cbc()
            }
            Err(error) => panic!("routing failed: {error}"),
        };
        assert_eq!(routes.len(), 3);
        let launch = routes
            .iter()
            .find(|route| {
                matches!(
                    route.channel,
                    RoutedChannel::Control {
                        channel: tapa_ir::ControlChannel::Launch,
                        ..
                    }
                )
            })
            .expect("launch route");
        let reset = routes
            .iter()
            .find(|route| {
                matches!(
                    route.channel,
                    RoutedChannel::Control {
                        channel: tapa_ir::ControlChannel::Reset,
                        ..
                    }
                )
            })
            .expect("reset route");
        assert_eq!(launch.route, reset.route);
        assert_eq!(launch.reg_regions, reset.reg_regions);
        assert_eq!(launch.scheme, reset.scheme);
    }

    #[test]
    fn register_budgets_leave_a_slot_what_placement_did_not_take() {
        let device = select_device("u280").expect("u280");
        let capacity = scaled_area(device.slot(0, 1).expect("slot").area, 0.7).ff;
        let baseline = BTreeMap::from([(
            Coor::slot(0, 1).region_name(),
            Area {
                ff: capacity - 100,
                ..Area::default()
            },
        )]);

        let budgets = register_budgets(&baseline, &device, 0.7);

        assert_eq!(budgets[&(0, 1)], 100, "only what placement left over");
        assert_eq!(
            budgets[&(1, 1)],
            scaled_area(device.slot(1, 1).expect("slot").area, 0.7).ff,
            "an untouched slot keeps its whole derated budget",
        );
        assert_eq!(budgets.len(), device.slots.len(), "every slot gets a budget");

        let overfull = BTreeMap::from([(
            Coor::slot(0, 1).region_name(),
            Area {
                ff: capacity + 1,
                ..Area::default()
            },
        )]);
        assert_eq!(
            register_budgets(&overfull, &device, 0.7)[&(0, 1)],
            0,
            "a slot already past its derating offers nothing, it does not wrap",
        );
    }

    /// The router must route *around* a slot with no room rather than leave the
    /// overflow for `realize_slot_usage` to reject after the fact.
    #[test]
    fn routing_detours_around_a_slot_with_no_register_budget() {
        use crate::solver::{CbcSolver, SolverError};

        let graph = one_stream_graph(2, Area::default(), Area::default());
        let device = select_device("u280").expect("u280");
        let regions = BTreeMap::from([
            ("Producer_0".to_string(), Coor::slot(0, 0).region_name()),
            ("Consumer_0".to_string(), Coor::slot(0, 2).region_name()),
        ]);
        // Fill the only intermediate slot on the direct path.
        let baseline = BTreeMap::from([(
            Coor::slot(0, 1).region_name(),
            scaled_area(device.slot(0, 1).expect("slot").area, MAX_USAGE_LIMIT),
        )]);
        let opts = SolveOpts {
            threads: Some(1),
            ..SolveOpts::default()
        };

        let route = |baseline: &BTreeMap<String, Area>| {
            match plan_routes(
                &graph,
                &regions,
                baseline,
                &device,
                PipelineScheme::Single,
                MAX_USAGE_LIMIT,
                &CbcSolver::new(),
                &opts,
            ) {
                Ok(routes) => routes,
                Err(PipelineError::Route(RouteError::Solver(SolverError::Spawn { .. }))) => {
                    crate::solver::missing_cbc()
                }
                Err(error) => panic!("routing failed: {error}"),
            }
        };

        let unconstrained = route(&BTreeMap::new());
        assert_eq!(
            unconstrained[0].reg_regions,
            ["SLOT_X0Y1"],
            "with room to spare the straight path is shortest",
        );

        let constrained = route(&baseline);
        assert!(
            !constrained[0].reg_regions.contains(&"SLOT_X0Y1".to_string()),
            "the full slot must hold no Body registers: {:?}",
            constrained[0],
        );
        assert_eq!(
            constrained[0].route.first().map(String::as_str),
            Some("SLOT_X0Y0"),
        );
        assert_eq!(
            constrained[0].route.last().map(String::as_str),
            Some("SLOT_X0Y2"),
        );
    }

    #[test]
    fn control_pipeline_accounts_head_body_and_tail_flip_flops() {
        let graph = one_controlled_task_graph();
        let source = Coor::slot(0, 0).region_name();
        let body = Coor::slot(0, 1).region_name();
        let destination = Coor::slot(0, 2).region_name();
        let regions = BTreeMap::from([
            (
                global_controller_instance_name().to_string(),
                source.clone(),
            ),
            ("worker#0".to_string(), destination.clone()),
        ]);
        let control_route = |channel, reverse: bool| PipelineRoute {
            channel: RoutedChannel::Control {
                instance: "worker#0".to_string(),
                channel,
            },
            route: if reverse {
                vec![
                    "SLOT_X0Y2".to_string(),
                    "SLOT_X0Y1".to_string(),
                    "SLOT_X0Y0".to_string(),
                ]
            } else {
                vec![
                    "SLOT_X0Y0".to_string(),
                    "SLOT_X0Y1".to_string(),
                    "SLOT_X0Y2".to_string(),
                ]
            },
            scheme: PipelineScheme::Single,
            reg_regions: vec!["SLOT_X0Y1".to_string()],
        };
        let routes = [
            control_route(tapa_ir::ControlChannel::Launch, false),
            control_route(tapa_ir::ControlChannel::Reset, false),
            control_route(tapa_ir::ControlChannel::Completion, true),
        ];
        let realized = realize_slot_usage(
            &graph,
            &regions,
            &BTreeMap::from([
                (source.clone(), Area::default()),
                (destination.clone(), Area::default()),
            ]),
            &routes,
            &select_device("u280").expect("u280"),
            MAX_USAGE_LIMIT,
        )
        .expect("account control pipelines");
        for region in [&source, &body, &destination] {
            assert_eq!(
                realized[region],
                Area {
                    ff: 36,
                    ..Area::default()
                },
                "34 launch + reset + completion registers in {region}"
            );
        }

        let mut invalid = routes[0].clone();
        invalid.route.reverse();
        let error = realize_slot_usage(
            &graph,
            &regions,
            &BTreeMap::new(),
            &[invalid],
            &select_device("u280").expect("u280"),
            MAX_USAGE_LIMIT,
        )
        .expect_err("reversed control endpoints must fail");
        assert!(error.to_string().contains("Head route region"), "{error}");
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
    fn realized_usage_honors_distinct_logic_and_block_caps() {
        let graph = one_stream_graph(2, Area::default(), Area::default());
        let device = select_device("u280").expect("u280");
        let region = Coor::slot(0, 0).region_name();
        let capacity = device.slot(0, 0).expect("slot").area;
        let logic_limit = 0.5;
        let block_limit = 0.6;
        let logic_capacity = scaled_area(capacity, logic_limit);
        let block_heavy = Area {
            bram_18k: logic_capacity.bram_18k + 1,
            dsp: logic_capacity.dsp + 1,
            uram: logic_capacity.uram + 1,
            ..Area::default()
        };

        let realized = realize_slot_usage_with_resource_caps(
            &graph,
            &BTreeMap::new(),
            &BTreeMap::from([(region.clone(), block_heavy)]),
            &[],
            &device,
            logic_limit,
            block_limit,
        )
        .expect("block resources between the logic and block caps are legal");
        assert_eq!(realized[&region], block_heavy);

        for (resource, over_limit) in [
            (
                Resource::Lut,
                Area {
                    lut: logic_capacity.lut + 1,
                    ..block_heavy
                },
            ),
            (
                Resource::Ff,
                Area {
                    ff: logic_capacity.ff + 1,
                    ..block_heavy
                },
            ),
        ] {
            let error = realize_slot_usage_with_resource_caps(
                &graph,
                &BTreeMap::new(),
                &BTreeMap::from([(region.clone(), over_limit)]),
                &[],
                &device,
                logic_limit,
                block_limit,
            )
            .expect_err("logic resources must remain bounded by the logic cap");
            assert!(matches!(
                error,
                PipelineError::RealizedCapacity {
                    region: ref failed_region,
                    resource: failed_resource,
                    ..
                } if failed_region == &region && failed_resource == resource.name()
            ));
        }
    }

    #[test]
    fn non_crossing_shallow_fifo_keeps_registered_ready_baseline() {
        let graph = one_stream_graph(16, Area::default(), Area::default());
        let region = Coor::slot(0, 0).region_name();
        let regions = BTreeMap::from([
            ("Producer_0".to_string(), region.clone()),
            ("Consumer_0".to_string(), region.clone()),
        ]);
        let baseline = baseline_usage(&graph, &regions);
        assert_eq!(
            baseline[&region],
            fifo_area(33, floorplanned_fifo_storage_depth(16)),
        );

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
    fn crossing_shallow_fifo_replaces_registered_ready_baseline() {
        let graph = one_stream_graph(16, Area::default(), Area::default());
        let source = Coor::slot(0, 0).region_name();
        let destination = Coor::slot(1, 0).region_name();
        let regions = BTreeMap::from([
            ("Producer_0".to_string(), source.clone()),
            ("Consumer_0".to_string(), destination.clone()),
        ]);
        let baseline = baseline_usage(&graph, &regions);
        assert_eq!(
            baseline[&destination],
            fifo_area(33, floorplanned_fifo_storage_depth(16)),
        );

        let realized = realize_slot_usage(
            &graph,
            &regions,
            &baseline,
            &[stream_route(&["SLOT_X0Y0", "SLOT_X1Y0"], &[])],
            &select_device("u280").expect("u280"),
            MAX_USAGE_LIMIT,
        )
        .expect("crossing usage");
        assert_eq!(
            realized[&source],
            Area {
                lut: 1,
                ff: 35,
                ..Area::default()
            },
        );
        assert_eq!(
            realized[&destination],
            fifo_area(33, 22),
            "the crossing Tail uses logical depth plus six safety entries",
        );
    }

    #[test]
    fn single_scheme_levels_by_intermediate_slots() {
        let route = vec![(0, 0), (0, 1), (0, 2)];
        assert_eq!(
            pipeline_reg_regions(&route, PipelineScheme::Single).len(),
            1,
            "one intermediate"
        );
    }

    #[test]
    fn double_scheme_two_per_hop() {
        let route = vec![(0, 0), (0, 1), (0, 2)];
        assert_eq!(
            pipeline_reg_regions(&route, PipelineScheme::Double).len(),
            4,
            "two hops * 2"
        );
    }

    #[test]
    fn single_h_double_v_weights_vertical_hops() {
        // One vertical hop (y changes) then one horizontal hop (x changes).
        let route = vec![(0, 0), (0, 1), (1, 1)];
        assert_eq!(
            pipeline_reg_regions(&route, PipelineScheme::SingleHDoubleV).len(),
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
        assert!(pipeline_reg_regions(&route, PipelineScheme::Single).is_empty());
    }

    #[test]
    fn hybrid_horizontal_crossing_has_no_body() {
        let route = [(0, 0), (1, 0)];
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
