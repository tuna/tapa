//! Plan orchestration: prepare the device/graph inputs, drive the placement
//! solver with the retry/exact-cap policy, and finish with routing,
//! pipelining, and slot-usage realization.

pub mod options;

pub mod fingerprint;

pub use options::{PlanOptions, PlanOptionsError};

use std::path::Path;
use std::time::Duration;

use tapa_ir::transforms::{flatten_with_origins, FlatProjection};
use tapa_ir::{FloorplanResult, WorkState};

use crate::device::select::select_device;
use crate::error::{PlanError, RenderXdcError};
use crate::graph::{ControlInterface, FloorGraph, MemoryInterface};
use crate::partition::ilp::{
    floorplan_with_exact_resource_caps, floorplan_with_strategy, resolve_strategy, MAX_USAGE_LIMIT,
};
use crate::partition::PartitionStrategy;
use crate::pipeline::plan::{plan_routes, realize_slot_usage};
use crate::solver::{CbcSolver, SolveOpts};
use crate::xdc;

pub const EXACT_DSE_CAP_SCALE: u32 = 1_000_000_000;
pub const MULTILEVEL_BLOCK_RESOURCE_MARGIN_UNITS: u32 = EXACT_DSE_CAP_SCALE / 10;

/// Effective resource limits for one exact DSE candidate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ExactDseResourceCaps {
    pub logic_utilization_cap: f64,
    pub effective_block_utilization_cap: f64,
    pub multilevel_block_margin_applied: bool,
}

pub struct ExactDsePlanAttempt {
    pub caps: ExactDseResourceCaps,
    pub result: Result<FloorplanResult, PlanError>,
}

impl ExactDseResourceCaps {
    pub(crate) fn for_strategy(logic_utilization_cap: f64, strategy: PartitionStrategy) -> Self {
        let multilevel_block_margin_applied = strategy == PartitionStrategy::MultiLevel;
        let effective_block_utilization_cap = if multilevel_block_margin_applied {
            let scale = f64::from(EXACT_DSE_CAP_SCALE);
            let margin = f64::from(MULTILEVEL_BLOCK_RESOURCE_MARGIN_UNITS) / scale;
            ((logic_utilization_cap + margin).min(1.0) * scale).round() / scale
        } else {
            logic_utilization_cap
        };
        Self {
            logic_utilization_cap,
            effective_block_utilization_cap,
            multilevel_block_margin_applied,
        }
    }
}

/// Transient inputs derived from generated RTL and link configuration.
///
/// They affect the ordinary placement/routing graph for this solve but are not
/// persisted as another design graph.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlanInputs {
    pub memory: Vec<MemoryInterface>,
    /// Enable the transient distributed-control topology for this solve.
    pub control: Option<ControlInterface>,
    /// Shared flat datapath and original occurrence references, when the
    /// caller already derived them from the work state's authoritative graph.
    pub projection: Option<FlatProjection>,
}

/// Plan with transient RTL/connectivity-derived external-interface inputs.
pub fn plan_with_inputs(
    state: &WorkState,
    options: &PlanOptions,
    inputs: &PlanInputs,
) -> Result<FloorplanResult, PlanError> {
    let retry_ceiling = table_retry_ceiling(state, options);
    plan_with_retry_ceiling(state, options, inputs, retry_ceiling)
}

/// The planner's retry ceiling: a table that pins a
/// [`Device::usage_ceiling`](crate::device::model::Device::usage_ceiling) overrides
/// the caller-derived cap (see that field for V80's rationale).
fn table_retry_ceiling(state: &WorkState, options: &PlanOptions) -> f64 {
    device_for_state(state)
        .ok()
        .and_then(|device| device.usage_ceiling)
        .unwrap_or_else(|| options.usage_limit.max(MAX_USAGE_LIMIT))
}

fn plan_with_retry_ceiling(
    state: &WorkState,
    options: &PlanOptions,
    inputs: &PlanInputs,
    retry_ceiling: f64,
) -> Result<FloorplanResult, PlanError> {
    let solver = CbcSolver::new();
    plan_with_retry_ceiling_and_solvers(state, options, inputs, retry_ceiling, &solver, &solver)
}

/// [`plan_with_retry_ceiling`] with explicit solvers for the two solve
/// phases, so fingerprint instrumentation can record the placement-phase
/// and finish-phase models separately. Production callers pass one CBC
/// instance twice.
#[allow(
    clippy::too_many_arguments,
    reason = "mirrors plan_with_retry_ceiling plus the two phase solvers"
)]
pub fn plan_with_retry_ceiling_and_solvers(
    state: &WorkState,
    options: &PlanOptions,
    inputs: &PlanInputs,
    retry_ceiling: f64,
    placement_solver: &dyn crate::solver::Solver,
    finish_solver: &dyn crate::solver::Solver,
) -> Result<FloorplanResult, PlanError> {
    options.validate()?;
    let (device, graph) = prepare_plan(state, inputs)?;
    log::info!(
        "floorplanning {} vertices on {} with usage limit {:.2} (ceiling {:.2}), strategy {:?}",
        graph.vertices().len(),
        device.key,
        options.usage_limit,
        retry_ceiling,
        options.partition_strategy,
    );
    let opts = solve_options(options);
    // `max_seconds` bounds one invocation, so report how many a plan actually
    // took: the total solver time is that count times the limit, not the limit.
    let placement_solver = CountingSolver::new(placement_solver);
    let finish_solver = CountingSolver::new(finish_solver);
    let result = (|| {
        let assignment = floorplan_with_strategy(
            &graph,
            &device,
            options.usage_limit,
            retry_ceiling,
            options.partition_strategy,
            &placement_solver,
            &opts,
        )?;
        finish_plan(
            &graph,
            &device,
            options,
            &finish_solver,
            &opts,
            assignment,
            ExactDseResourceCaps {
                logic_utilization_cap: retry_ceiling,
                effective_block_utilization_cap: retry_ceiling,
                multilevel_block_margin_applied: false,
            },
        )
    })();
    log::info!(
        "floorplan issued {} placement and {} routing solves, each capped at {}s",
        placement_solver.count(),
        finish_solver.count(),
        options.max_seconds,
    );
    result
}

/// A pass-through [`Solver`](crate::solver::Solver) that counts invocations, so
/// a plan can report how many times it paid `max_seconds`.
struct CountingSolver<'a> {
    inner: &'a dyn crate::solver::Solver,
    calls: std::cell::Cell<usize>,
}

impl<'a> CountingSolver<'a> {
    fn new(inner: &'a dyn crate::solver::Solver) -> Self {
        Self {
            inner,
            calls: std::cell::Cell::new(0),
        }
    }

    fn count(&self) -> usize {
        self.calls.get()
    }
}

impl crate::solver::Solver for CountingSolver<'_> {
    fn solve(
        &self,
        model: &crate::solver::LpModel,
        opts: &SolveOpts,
    ) -> Result<crate::solver::LpSolution, crate::solver::SolverError> {
        self.calls.set(self.calls.get() + 1);
        self.inner.solve(model, opts)
    }
}

pub fn plan_with_inputs_at_usage_limit_and_caps(
    state: &WorkState,
    options: &PlanOptions,
    inputs: &PlanInputs,
) -> Result<ExactDsePlanAttempt, PlanError> {
    options.validate()?;
    let (device, graph) = prepare_plan(state, inputs)?;
    let strategy = resolve_strategy(&graph, &device, options.partition_strategy);
    let caps = ExactDseResourceCaps::for_strategy(options.usage_limit, strategy);
    log::info!(
        "floorplanning {} vertices on {} at exact usage limit {:.2} (block cap {:.2}), strategy {:?}",
        graph.vertices().len(),
        device.key,
        caps.logic_utilization_cap,
        caps.effective_block_utilization_cap,
        strategy,
    );
    let solver = CbcSolver::new();
    let opts = solve_options(options);
    let result = (|| {
        let assignment = floorplan_with_exact_resource_caps(
            &graph,
            &device,
            caps.logic_utilization_cap,
            caps.effective_block_utilization_cap,
            strategy,
            &solver,
            &opts,
        )?;
        finish_plan(&graph, &device, options, &solver, &opts, assignment, caps)
    })();
    Ok(ExactDsePlanAttempt { caps, result })
}

fn solve_options(options: &PlanOptions) -> SolveOpts {
    SolveOpts {
        time_limit: Some(Duration::from_secs(options.max_seconds)),
        threads: Some(options.threads),
        mip_gap_abs: None,
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "the completed plan keeps its graph, device, solver, and exact resource limits explicit"
)]
fn finish_plan(
    graph: &FloorGraph,
    device: &crate::device::model::Device,
    options: &PlanOptions,
    solver: &dyn crate::solver::Solver,
    opts: &SolveOpts,
    assignment: crate::partition::Assignment,
    realized_caps: ExactDseResourceCaps,
) -> Result<FloorplanResult, PlanError> {
    let routes = plan_routes(
        graph,
        &assignment.regions,
        &assignment.slot_usage,
        device,
        options.scheme,
        realized_caps.logic_utilization_cap,
        solver,
        opts,
    )?;
    // Deliberate headroom: the relaxed `plan` path validates the *realized*
    // usage — placement plus pipeline registers — against the retry ceiling
    // (`MAX_USAGE_LIMIT`), not the cap the placement was found at, so a
    // placement at 0.6 may absorb registers up to the ceiling. The exact
    // DSE path validates against its per-candidate logic/block caps instead.
    let slot_usage = realize_slot_usage(
        graph,
        &assignment.regions,
        &assignment.slot_usage,
        &routes,
        device,
        realized_caps.logic_utilization_cap,
        realized_caps.effective_block_utilization_cap,
    )?;
    let mut regions = graph
        .vertices()
        .iter()
        .zip(assignment.regions)
        .map(|(vertex, (x, y))| (vertex.name.clone(), tapa_ir::Coor::slot(x, y).region_name()))
        .collect();
    graph.materialize_co_locations(&mut regions)?;
    graph.remove_transient_regions(&mut regions);

    Ok(FloorplanResult {
        device: device.key.clone(),
        grid: (device.cols, device.rows),
        regions,
        routes: routes.into_iter().map(Into::into).collect(),
        slot_usage: slot_usage
            .into_iter()
            .map(|((x, y), area)| (tapa_ir::Coor::slot(x, y).region_name(), area))
            .collect(),
    })
}

/// The table for the synthesized part, refined to a SLASH shell when the
/// platform names one (`slash_v80_service` → `service`). Other platforms
/// leave the part-only default: the service shell on V80.
fn device_for_state(state: &WorkState) -> Result<crate::device::model::Device, PlanError> {
    let part = state.flow.part_num.as_deref().ok_or(PlanError::NoPartNum)?;
    let shell = state
        .flow
        .platform
        .as_deref()
        .filter(|platform| platform.starts_with("slash_"))
        .map(|platform| platform.strip_prefix("slash_v80_").unwrap_or(platform));
    Ok(crate::device::select_device_with_shell(part, shell)?)
}

fn prepare_plan(
    state: &WorkState,
    inputs: &PlanInputs,
) -> Result<(crate::device::model::Device, FloorGraph), PlanError> {
    let device = device_for_state(state)?;
    validate_platform(state, &device, !inputs.memory.is_empty())?;
    // A bank the platform reaches through the NoC from every slot anchors
    // nothing; any other bank anchors to the one slot that carries it.
    let mut noc_banks = std::collections::BTreeSet::new();
    let mut validated_banks = std::collections::BTreeSet::new();
    for bank in inputs.memory.iter().map(|interface| interface.bank) {
        if !validated_banks.insert(bank) {
            continue;
        }
        if device.reaches_through_noc(bank) {
            noc_banks.insert(bank);
            continue;
        }
        let matches = device.slots_with_tag(&bank.to_string()).count();
        if matches != 1 {
            return Err(PlanError::BankTag { bank, matches });
        }
    }
    let derived_projection;
    let projection = if let Some(projection) = &inputs.projection {
        projection
    } else {
        derived_projection = flatten_with_origins(&state.graph)?;
        &derived_projection
    };
    let active_control = inputs.control.filter(|_| {
        state
            .graph
            .tasks
            .get(&state.graph.top)
            .is_some_and(|top| top.level == tapa_ir::TaskLevel::Upper && !top.tasks.is_empty())
    });
    let control_anchor = active_control
        .map(|control| select_control_anchor(&device, control))
        .transpose()?
        .flatten();
    let graph = FloorGraph::build_with_projection(
        projection,
        &inputs.memory,
        &noc_banks,
        active_control,
        control_anchor,
    )?;
    Ok((device, graph))
}

fn select_control_anchor(
    device: &crate::device::model::Device,
    control: ControlInterface,
) -> Result<Option<&'static str>, PlanError> {
    let tag = if control.has_s_axi_control {
        "S_AXI_CONTROL"
    } else {
        "CLK_RST"
    };
    let matches = device.slots_with_tag(tag).count();
    match matches {
        0 => Ok(None),
        1 => Ok(Some(tag)),
        _ => Err(PlanError::ControlTag { tag, matches }),
    }
}

/// Check the recorded platform against the shell the device table was built
/// from. Another shell is rejected whatever the design binds, since the
/// table's slots and pblocks describe its own shell only. Without a recorded
/// platform the plan stays part-only, unless the design binds memory, whose
/// bank locations belong to the table's shell.
fn validate_platform(
    state: &WorkState,
    device: &crate::device::model::Device,
    has_memory: bool,
) -> Result<(), PlanError> {
    let Some(expected) = device.platform_name.as_deref() else {
        return Ok(());
    };
    let Some(platform) = state.flow.platform.as_deref() else {
        if has_memory {
            return Err(PlanError::PlatformRequired {
                expected: expected.to_string(),
            });
        }
        return Ok(());
    };
    let basename = Path::new(platform)
        .file_name()
        .map_or(platform, |name| name.to_str().unwrap_or(platform));
    let normalized = basename.replace([':', '.'], "_");
    if normalized != expected {
        return Err(PlanError::PlatformMismatch {
            platform: platform.to_string(),
            expected: expected.to_string(),
        });
    }
    Ok(())
}

/// Render a floorplan's pblock XDC, re-selecting the device from the result.
pub fn render_xdc(result: &FloorplanResult) -> Result<String, RenderXdcError> {
    let device = select_device(&result.device)?;
    Ok(xdc::emit_xdc(result, &device)?)
}

#[cfg(test)]
use crate::partition::ilp::IlpError;
#[cfg(test)]
use crate::pipeline::plan::PipelineError;
#[cfg(test)]
use tapa_ir::MemoryBank;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::solver::SolverError;

    #[test]
    fn plan_end_to_end_on_vadd() {
        let json = r#"{
            "schema_version": 4,
            "cflags": [], "top": "VecAdd", "target": "xilinx-hls",
            "tasks": {
                "VecAdd": {
                    "readable_name": "VecAdd", "srcs": ["VecAdd.cpp"], "include_dirs": [], "defines": [], "level": "upper", "synth": "hls",
                    "ports": [],
                    "tasks": {
                        "A": [{"args": {"out": {"arg": "fifo", "cat": "ostream"}}, "step": 0}],
                        "B": [{"args": {"in": {"arg": "fifo", "cat": "istream"}}, "step": 0}]
                    },
                    "fifos": {"fifo": {"depth": 2, "consumed_by": ["B", 0], "produced_by": ["A", 0]}}
                },
                "A": {"readable_name": "A", "srcs": ["A.cpp"], "include_dirs": [], "defines": [], "level": "lower", "synth": "hls",
                    "ports": [{"cat": "ostream", "name": "out", "type": "float", "width": 32}],
                    "self_area": {"lut": 100, "ff": 200}},
                "B": {"readable_name": "B", "srcs": ["B.cpp"], "include_dirs": [], "defines": [], "level": "lower", "synth": "hls",
                    "ports": [{"cat": "istream", "name": "in", "type": "float", "width": 32}],
                    "self_area": {"lut": 50, "ff": 60}}
            }
        }"#;
        let graph = tapa_ir::TaskGraph::from_json(json).expect("parse");
        let mut state = WorkState::new(graph);
        state.flow.part_num = Some("xcu280-fsvh2892-2L-e".to_string());

        match plan_with_inputs(&state, &PlanOptions::default(), &PlanInputs::default()) {
            Ok(result) => {
                assert_eq!(result.device, "u280");
                assert_eq!(result.grid, (2, 3));
                assert!(result.routes.is_empty(), "the tiny design co-locates");
                assert_eq!(result.regions.len(), 3, "A_0, B_0, and the FIFO");
                assert_eq!(
                    result.regions["fifo_VecAdd"], result.regions["B_0"],
                    "the physical FIFO is co-located with its consumer Tail"
                );
                // The rendered XDC references the assigned pblock.
                let xdc = render_xdc(&result).expect("render xdc");
                assert!(xdc.contains("create_pblock SLOT_X"));
            }
            Err(PlanError::Ilp(IlpError::Solver(SolverError::Spawn { .. }))) => {
                crate::solver::missing_cbc()
            }
            Err(other) => panic!("plan failed: {other}"),
        }
    }

    #[test]
    fn plan_pipelines_a_forced_crossing() {
        use tapa_ir::RoutedChannel;
        // Two tasks whose LUTs together exceed one u280 slot's derated
        // capacity (2 * 120000 > 220800 * 0.7) must split into adjacent slots,
        // so their connecting stream crosses one boundary.
        let json = r#"{
            "schema_version": 4,
            "cflags": [], "top": "VecAdd", "target": "xilinx-vitis",
            "tasks": {
                "VecAdd": {
                    "readable_name": "VecAdd", "srcs": ["VecAdd.cpp"], "include_dirs": [], "defines": [], "level": "upper", "synth": "hls",
                    "ports": [],
                    "tasks": {
                        "A": [{"args": {"out": {"arg": "fifo", "cat": "ostream"}}, "step": 0}],
                        "B": [{"args": {"in": {"arg": "fifo", "cat": "istream"}}, "step": 0}]
                    },
                    "fifos": {"fifo": {"depth": 2, "consumed_by": ["B", 0], "produced_by": ["A", 0]}}
                },
                "A": {"readable_name": "A", "srcs": ["A.cpp"], "include_dirs": [], "defines": [], "level": "lower", "synth": "hls",
                    "ports": [{"cat": "ostream", "name": "out", "type": "float", "width": 32}],
                    "self_area": {"lut": 120000}},
                "B": {"readable_name": "B", "srcs": ["B.cpp"], "include_dirs": [], "defines": [], "level": "lower", "synth": "hls",
                    "ports": [{"cat": "istream", "name": "in", "type": "float", "width": 32}],
                    "self_area": {"lut": 120000}}
            }
        }"#;
        let graph = tapa_ir::TaskGraph::from_json(json).expect("parse");
        let mut state = WorkState::new(graph);
        state.flow.part_num = Some("xcu280-fsvh2892-2L-e".to_string());

        match plan_with_inputs(&state, &PlanOptions::default(), &PlanInputs::default()) {
            Ok(result) => {
                assert!(
                    result
                        .regions
                        .values()
                        .collect::<std::collections::BTreeSet<_>>()
                        .len()
                        >= 2,
                    "the two large tasks cannot share a slot",
                );
                assert_eq!(result.routes.len(), 1, "the one stream crosses");
                let route = &result.routes[0];
                assert_eq!(
                    route.channel,
                    RoutedChannel::Stream {
                        fifo: "fifo_VecAdd".to_string()
                    }
                );
                assert_eq!(
                    result.regions["fifo_VecAdd"], result.regions["B_0"],
                    "the routed stream must end at the FIFO's physical region"
                );
                assert_ne!(
                    result.regions["fifo_VecAdd"], result.regions["A_0"],
                    "the forced split keeps the destination-side FIFO off the producer"
                );
                assert_eq!(route.route.len(), 2, "adjacent slots, one hop");
                assert_eq!(
                    route.reg_regions.len(),
                    2,
                    "double scheme, one hop -> 2 stages"
                );
            }
            Err(PlanError::Ilp(IlpError::Solver(SolverError::Spawn { .. }))) => {
                crate::solver::missing_cbc()
            }
            Err(other) => panic!("plan failed: {other}"),
        }
    }

    fn nested_control_graph() -> tapa_ir::TaskGraph {
        let json = r#"{
            "schema_version": 4,
            "cflags": [], "top": "Top", "target": "xilinx-vitis",
            "tasks": {
                "Top": {
                    "readable_name":"Top","srcs": ["Top.cpp"], "include_dirs": [], "defines": [],"level":"upper","synth":"hls",
                    "ports":[],"tasks":{
                        "Group":[{"name":"background","args":{},"step":-1}],
                        "Ticker":[{"name":"ticker[1]","args":{},"step":-1}]
                    },"fifos":{}
                },
                "Group": {"readable_name":"Group","srcs": ["Group.cpp"], "include_dirs": [], "defines": [],"level":"upper","synth":"hls",
                    "ports":[],"tasks":{"Normal":[{"name":"normal#0","args":{},"step":0}]},"fifos":{}},
                "Normal": {"readable_name":"Normal","srcs": ["Normal.cpp"], "include_dirs": [], "defines": [],"level":"lower","synth":"hls",
                    "ports":[],"self_area":{"lut":120000}},
                "Ticker": {"readable_name":"Ticker","srcs": ["Ticker.cpp"], "include_dirs": [], "defines": [],"level":"lower","synth":"hls",
                    "ports":[],"self_area":{"lut":120000}}
            }
        }"#;
        tapa_ir::TaskGraph::from_json(json).expect("parse")
    }

    #[test]
    fn distributed_control_plan_materializes_and_routes_exact_inventory() {
        let graph = nested_control_graph();
        let mut state = WorkState::new(graph);
        state.flow.part_num = Some("xcu280-fsvh2892-2L-e".to_string());

        let result = match plan_with_inputs(
            &state,
            &PlanOptions::default(),
            &PlanInputs {
                control: Some(ControlInterface {
                    has_s_axi_control: true,
                }),
                ..PlanInputs::default()
            },
        ) {
            Ok(result) => result,
            Err(
                PlanError::Ilp(IlpError::Solver(SolverError::Spawn { .. }))
                | PlanError::Pipeline(PipelineError::Route(crate::route::ilp::RouteError::Solver(
                    SolverError::Spawn { .. },
                ))),
            ) => crate::solver::missing_cbc(),
            Err(error) => panic!("control plan failed: {error}"),
        };
        let global = tapa_ir::global_controller_instance_name();
        assert_eq!(
            result.regions[global],
            crate::device::model::Coor::slot(1, 1).region_name(),
            "the exact S-AXI controller tag anchors the global controller"
        );
        assert_eq!(result.regions["control_s_axi_U"], result.regions[global]);
        for controller in [
            tapa_ir::scope_controller_instance_name("background_Top"),
            tapa_ir::local_controller_instance_name("background_Top"),
        ] {
            assert_eq!(
                result.regions[&controller], result.regions[global],
                "upper scope controllers share the global controller placement"
            );
        }
        for instance in ["normal#0_background_Top", "ticker[1]"] {
            assert_eq!(
                result.regions[&tapa_ir::local_controller_instance_name(instance)],
                result.regions[instance]
            );
            let crossing = result.regions[instance] != result.regions[global];
            let routes = result
                .routes
                .iter()
                .filter(|route| {
                    matches!(
                        &route.channel,
                        tapa_ir::RoutedChannel::Control {
                            instance: routed_instance,
                            ..
                        } if routed_instance == instance
                    )
                })
                .collect::<Vec<_>>();
            let expected = if !crossing {
                0
            } else if instance == "ticker[1]" {
                2
            } else {
                3
            };
            assert_eq!(routes.len(), expected, "route inventory for {instance}");
            if crossing {
                let launch = routes
                    .iter()
                    .find(|route| {
                        matches!(
                            route.channel,
                            tapa_ir::RoutedChannel::Control {
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
                            tapa_ir::RoutedChannel::Control {
                                channel: tapa_ir::ControlChannel::Reset,
                                ..
                            }
                        )
                    })
                    .expect("reset route");
                assert_eq!(launch.route, reset.route);
                assert_eq!(launch.reg_regions, reset.reg_regions);
            }
        }
        let xdc = render_xdc(&result).expect("render control XDC");
        assert!(xdc.contains(global));
        assert!(xdc.contains(&tapa_ir::local_controller_instance_name(
            "normal#0_background_Top"
        )));
        assert!(xdc.contains(&tapa_ir::local_controller_instance_name("ticker[1]")));
    }

    #[test]
    fn scoped_control_preserves_empty_upper_occurrences() {
        let mut source = nested_control_graph();
        let mut empty = source.tasks[&source.top].clone();
        empty.tasks.clear();
        empty.fifos.clear();
        empty.ports.clear();
        source.tasks.insert("Empty".into(), empty);
        let top = source.tasks.get_mut(&source.top).expect("top");
        top.fifos.clear();
        top.tasks = std::collections::BTreeMap::from([(
            "Empty".into(),
            [-1, 0]
                .map(|step| tapa_ir::TaskInstance {
                    name: None,
                    args: std::collections::BTreeMap::new(),
                    step,
                    position: 0,
                })
                .to_vec(),
        )]);
        let mut state = WorkState::new(source);
        state.flow.part_num = Some("xcu280-fsvh2892-2L-e".into());
        let (_, graph) = prepare_plan(
            &state,
            &PlanInputs {
                control: Some(ControlInterface::default()),
                projection: Some(flatten_with_origins(&state.graph).expect("shared projection")),
                ..PlanInputs::default()
            },
        )
        .expect("empty scoped control graph");
        assert_eq!(
            graph.vertices().len(),
            1,
            "scope controllers add no placement vertices"
        );
        assert!(graph.control_nets().is_empty(), "there are no leaf routes");
        let global = tapa_ir::global_controller_instance_name();
        let region = "SLOT_X0Y0";
        let mut regions = std::collections::BTreeMap::from([(global.into(), region.into())]);
        graph
            .materialize_co_locations(&mut regions)
            .expect("scope placements");
        for index in 0..2 {
            let scope = format!("Empty_{index}_Top");
            assert_eq!(
                regions[&tapa_ir::scope_controller_instance_name(&scope)],
                region
            );
            assert_eq!(
                regions[&tapa_ir::local_controller_instance_name(&scope)],
                region
            );
        }
    }

    #[test]
    fn control_anchor_prefers_exact_shell_location_and_rejects_ambiguity() {
        let device = select_device("u280").expect("u280");
        assert_eq!(
            select_control_anchor(
                &device,
                ControlInterface {
                    has_s_axi_control: true,
                }
            )
            .expect("S-AXI anchor"),
            Some("S_AXI_CONTROL")
        );
        assert_eq!(
            select_control_anchor(&device, ControlInterface::default()).expect("clock anchor"),
            Some("CLK_RST")
        );

        let mut absent = device.clone();
        for slot in &mut absent.slots {
            slot.tags.retain(|tag| tag != "CLK_RST");
        }
        assert_eq!(
            select_control_anchor(&absent, ControlInterface::default()).expect("optional anchor"),
            None
        );

        let mut ambiguous = device;
        ambiguous.slots[0].tags.push("S_AXI_CONTROL".to_string());
        let error = select_control_anchor(
            &ambiguous,
            ControlInterface {
                has_s_axi_control: true,
            },
        )
        .expect_err("ambiguous anchor must fail");
        assert!(matches!(
            error,
            PlanError::ControlTag {
                tag: "S_AXI_CONTROL",
                matches: 2
            }
        ));
    }

    #[test]
    fn plan_without_part_number_errors() {
        let graph = tapa_ir::TaskGraph::from_json(
            r#"{"schema_version": 4, "cflags": [], "top": "T", "target": "xilinx-hls",
                "tasks": {"T": {"readable_name": "T", "srcs": ["T.cpp"], "include_dirs": [], "defines": [], "level": "upper",
                    "synth": "hls", "ports": [], "tasks": {}, "fifos": {}}}}"#,
        )
        .expect("parse");
        let state = WorkState::new(graph);
        assert!(matches!(
            plan_with_inputs(&state, &PlanOptions::default(), &PlanInputs::default()),
            Err(PlanError::NoPartNum)
        ));
    }

    #[test]
    fn a_plan_requires_the_table_platform_it_records() {
        let graph = tapa_ir::TaskGraph::from_json(
            r#"{"schema_version": 4, "cflags": [], "top": "T", "target": "xilinx-vitis",
                "tasks": {"T": {"readable_name": "T", "srcs": ["T.cpp"], "include_dirs": [], "defines": [], "level": "upper",
                    "synth": "hls", "ports": [], "tasks": {}, "fifos": {}}}}"#,
        )
        .expect("parse");
        let mut state = WorkState::new(graph);
        let device = select_device("u280").expect("u280");

        assert!(matches!(
            validate_platform(&state, &device, true),
            Err(PlanError::PlatformRequired { .. })
        ));
        validate_platform(&state, &device, false).expect("without a platform, part-only");

        // The table's slots belong to its shell, so another shell is
        // rejected even for a design that binds no memory.
        state.flow.platform = Some("/opt/xilinx/platforms/xilinx_u280_xdma_201920_3".to_string());
        for has_memory in [true, false] {
            assert!(matches!(
                validate_platform(&state, &device, has_memory),
                Err(PlanError::PlatformMismatch { .. })
            ));
        }

        state.flow.platform = device
            .platform_name
            .as_ref()
            .map(|name| format!("/opt/xilinx/platforms/{name}"));
        validate_platform(&state, &device, true).expect("matching platform");
        validate_platform(&state, &device, false).expect("matching platform");
    }

    #[test]
    fn devices_without_exact_bank_tags_reject_memory_inputs_before_solving() {
        let graph = tapa_ir::TaskGraph::from_json(
            r#"{"schema_version": 4, "cflags": [], "top": "T", "target": "xilinx-vitis",
                "tasks": {"T": {"readable_name": "T", "srcs": ["T.cpp"], "include_dirs": [], "defines": [], "level": "upper",
                    "synth": "hls", "ports": [], "tasks": {}, "fifos": {}}}}"#,
        )
        .expect("parse");
        let interface = MemoryInterface {
            endpoint: tapa_ir::AxiEndpoint {
                instance: "Reader_0".to_string(),
                port: "mem".to_string(),
                top_port: "mem".to_string(),
            },
            bank: MemoryBank {
                kind: tapa_ir::MemoryKind::Ddr,
                index: Some(0),
            },
            channel_widths: tapa_ir::AxiChannelWidths {
                read_address: 80,
                read_data: 38,
                write_address: 80,
                write_data: 39,
                write_response: 5,
            },
            bridge_instance: None,
        };

        for (part, modeled) in [
            ("xcu250-figd2104-2L-e", "DDR[0]"),
            ("xcu280-fsvh2892-2L-e", "DDR[0]"),
            ("xcvc1902-vsva2197-2MP-e-S", "DDR"),
        ] {
            let mut state = WorkState::new(graph.clone());
            state.flow.part_num = Some(part.to_string());
            // Exact bank locations are only valid for the platform the table
            // was measured on, so the platform check runs first.
            state.flow.platform = crate::device::select_device(part)
                .expect("known part")
                .platform_name
                .clone();
            // An index no device models — a typo in the connectivity file —
            // must be caught before CBC is handed an unsolvable problem.
            let mut unmodeled = interface.clone();
            unmodeled.bank.index = Some(99);
            let error = plan_with_inputs(
                &state,
                &PlanOptions::default(),
                &PlanInputs {
                    memory: vec![unmodeled],
                    control: None,
                    projection: None,
                },
            )
            .expect_err("an unmodeled bank must fail before CBC");
            assert!(
                matches!(error, PlanError::BankTag { matches: 0, .. }),
                "{part}: expected a bank-tag error, got {error:?}"
            );

            // A bank the table models must get past the bank check (the call
            // still fails later on this stub graph).
            let mut modeled_interface = interface.clone();
            modeled_interface.bank = modeled.parse().expect("memory tag");
            let error = plan_with_inputs(
                &state,
                &PlanOptions::default(),
                &PlanInputs {
                    memory: vec![modeled_interface],
                    control: None,
                    projection: None,
                },
            )
            .expect_err("the stub graph cannot be planned");
            assert!(
                !matches!(error, PlanError::BankTag { .. }),
                "{part}: {modeled} must resolve, got {error:?}"
            );
        }
    }

    /// VCK190's `DDR` and `LPDDR` enter the NoC beside each task, so a plan
    /// gives their ports no terminal and no fabric route: the readers place
    /// for their own area alone, and nothing is pipelined.
    #[test]
    fn vck190_noc_memory_needs_no_terminal_or_route() {
        let reader = |name: &str| {
            serde_json::json!({
                "readable_name": name, "srcs": [format!("{name}.cpp")], "include_dirs": [],
                "defines": [], "level": "lower", "synth": "hls",
                "ports": [{"cat": "mmap", "name": "data", "type": "int*", "width": 32}],
                "self_area": {"lut": 150_000, "ff": 150_000}
            })
        };
        let graph = tapa_ir::TaskGraph::from_json(
            &serde_json::json!({
                "schema_version": 4, "cflags": [], "top": "Top", "target": "xilinx-vitis",
                "tasks": {
                    "Top": {
                        "readable_name": "Top", "srcs": ["Top.cpp"], "include_dirs": [],
                        "defines": [], "level": "upper", "synth": "hls",
                        "ports": [
                            {"cat": "mmap", "name": "a", "type": "int*", "width": 32},
                            {"cat": "mmap", "name": "b", "type": "int*", "width": 32}
                        ],
                        "tasks": {
                            "A": [{"args": {"data": {"arg": "a", "cat": "mmap"}}, "step": 0}],
                            "B": [{"args": {"data": {"arg": "b", "cat": "mmap"}}, "step": 0}]
                        },
                        "fifos": {}
                    },
                    "A": reader("A"),
                    "B": reader("B")
                }
            })
            .to_string(),
        )
        .expect("parse");
        let interface = |instance: &str, top_port: &str, tag: &str| MemoryInterface {
            endpoint: tapa_ir::AxiEndpoint {
                instance: instance.to_string(),
                port: "data".to_string(),
                top_port: top_port.to_string(),
            },
            bank: tag.parse().expect("memory tag"),
            channel_widths: tapa_ir::AxiChannelWidths {
                read_address: 80,
                read_data: 38,
                write_address: 80,
                write_data: 39,
                write_response: 5,
            },
            bridge_instance: None,
        };
        let mut state = WorkState::new(graph);
        state.flow.part_num = Some("xcvc1902-vsva2197-2MP-e-S".to_string());
        state.flow.platform = Some("xilinx_vck190_base_202410_1".to_string());

        let result = match plan_with_inputs(
            &state,
            &PlanOptions::default(),
            &PlanInputs {
                memory: vec![interface("A_0", "a", "DDR"), interface("B_0", "b", "LPDDR")],
                control: None,
                projection: None,
            },
        ) {
            Ok(result) => result,
            Err(PlanError::Ilp(IlpError::Solver(SolverError::Spawn { .. }))) => {
                crate::solver::missing_cbc()
            }
            Err(other) => panic!("plan failed: {other}"),
        };
        assert!(result.routes.is_empty(), "{:?}", result.routes);
        assert_eq!(
            result.regions.keys().collect::<Vec<_>>(),
            ["A_0", "B_0"],
            "no memory terminal is published"
        );
        assert_ne!(
            result.regions["A_0"], result.regions["B_0"],
            "the readers do not fit one slot"
        );
    }

    /// Every VCK190 port bound to `DDR` or `LPDDR` takes one NMU of its
    /// task's slot, from one pool for both memories. Five small readers with
    /// two such ports each fit slot (0,0)'s fabric, which the stable ranking
    /// prefers, but not its eight NMUs: without the cap all ten ports land
    /// there and the NoC compiler takes two NMUs from another slot.
    #[test]
    fn vck190_places_no_more_noc_ports_in_a_slot_than_it_has_nmus() {
        const READERS: usize = 5;
        let port = |name: &str| serde_json::json!({"cat": "mmap", "name": name, "type": "int*", "width": 32});
        let top_ports: Vec<_> = (0..2 * READERS).map(|i| port(&format!("p{i}"))).collect();
        let instances: Vec<_> = (0..READERS)
            .map(|i| {
                serde_json::json!({"args": {
                    "a": {"arg": format!("p{}", 2 * i), "cat": "mmap"},
                    "b": {"arg": format!("p{}", 2 * i + 1), "cat": "mmap"}
                }, "step": 0})
            })
            .collect();
        let graph = tapa_ir::TaskGraph::from_json(
            &serde_json::json!({
                "schema_version": 4, "cflags": [], "top": "Top", "target": "xilinx-vitis",
                "tasks": {
                    "Top": {
                        "readable_name": "Top", "srcs": ["Top.cpp"], "include_dirs": [],
                        "defines": [], "level": "upper", "synth": "hls",
                        "ports": top_ports, "tasks": {"Reader": instances}, "fifos": {}
                    },
                    "Reader": {
                        "readable_name": "Reader", "srcs": ["Reader.cpp"], "include_dirs": [],
                        "defines": [], "level": "lower", "synth": "hls",
                        "ports": [port("a"), port("b")],
                        "self_area": {"lut": 1000, "ff": 1000}
                    }
                }
            })
            .to_string(),
        )
        .expect("parse");
        let memory = (0..READERS)
            .flat_map(|i| {
                [("a", 2 * i, "DDR"), ("b", 2 * i + 1, "LPDDR")].map(|(child, top, tag)| {
                    MemoryInterface {
                        endpoint: tapa_ir::AxiEndpoint {
                            instance: format!("Reader_{i}"),
                            port: child.to_string(),
                            top_port: format!("p{top}"),
                        },
                        bank: tag.parse().expect("memory tag"),
                        channel_widths: tapa_ir::AxiChannelWidths {
                            read_address: 80,
                            read_data: 38,
                            write_address: 80,
                            write_data: 39,
                            write_response: 5,
                        },
                        bridge_instance: None,
                    }
                })
            })
            .collect();
        let mut state = WorkState::new(graph);
        state.flow.part_num = Some("xcvc1902-vsva2197-2MP-e-S".to_string());
        state.flow.platform = Some("xilinx_vck190_base_202410_1".to_string());

        let result = match plan_with_inputs(
            &state,
            &PlanOptions::default(),
            &PlanInputs {
                memory,
                control: None,
                projection: None,
            },
        ) {
            Ok(result) => result,
            Err(PlanError::Ilp(IlpError::Solver(SolverError::Spawn { .. }))) => {
                crate::solver::missing_cbc()
            }
            Err(other) => panic!("plan failed: {other}"),
        };
        let device = select_device("vck190").expect("vck190");
        for slot in &device.slots {
            let name = tapa_ir::Coor::slot(slot.x, slot.y).region_name();
            let ports = 2 * result
                .regions
                .values()
                .filter(|region| **region == name)
                .count();
            assert!(
                ports <= usize::try_from(slot.nmus).expect("fits"),
                "{name} hosts {ports} NoC ports but has {} NMUs: {:?}",
                slot.nmus,
                result.regions,
            );
        }
    }

    #[test]
    fn plan_options_fail_before_solver_or_device_lookup() {
        let graph = tapa_ir::TaskGraph::from_json(
            r#"{"schema_version": 4, "cflags": [], "top": "T", "target": "xilinx-hls",
                "tasks": {"T": {"readable_name": "T", "srcs": ["T.cpp"], "include_dirs": [], "defines": [], "level": "upper",
                    "synth": "hls", "ports": [], "tasks": {}, "fifos": {}}}}"#,
        )
        .expect("parse");
        let state = WorkState::new(graph);

        for options in [
            PlanOptions {
                usage_limit: 0.0,
                ..PlanOptions::default()
            },
            PlanOptions {
                usage_limit: 1.01,
                ..PlanOptions::default()
            },
            PlanOptions {
                usage_limit: f64::NAN,
                ..PlanOptions::default()
            },
            PlanOptions {
                max_seconds: 0,
                ..PlanOptions::default()
            },
            PlanOptions {
                threads: 0,
                ..PlanOptions::default()
            },
        ] {
            assert!(matches!(
                plan_with_inputs(&state, &options, &PlanInputs::default()),
                Err(PlanError::Options(_))
            ));
        }
    }
}
