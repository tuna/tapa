//! Plan orchestration: prepare the device/graph inputs, drive the placement
//! solver with the retry/exact-cap policy, and finish with routing,
//! pipelining, and slot-usage realization.

pub mod options;

pub mod fingerprint;

pub use options::{PlanOptions, PlanOptionsError};

use std::path::Path;
use std::time::Duration;

use tapa_ir::{FloorplanResult, WorkState};

use crate::device::select::select_device;
use crate::error::{PlanError, RenderXdcError};
use crate::graph::{ControlInterface, FloorGraph, MemoryInterface};
use crate::partition::ilp::{
    floorplan_with_exact_resource_caps, floorplan_with_strategy, resolve_strategy, MAX_USAGE_LIMIT,
};
use crate::partition::PartitionStrategy;
use crate::pipeline::plan::{
    plan_routes, realize_slot_usage, realize_slot_usage_with_resource_caps,
};
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
}

/// Plan a floorplan for a synthesized design.
///
/// Selects the device from the work state's part number, flattens the graph,
/// places every instance with the floorplan ILP, routes and pipelines every
/// cross-slot channel, and returns the complete [`FloorplanResult`] contract.
pub fn plan(state: &WorkState, options: &PlanOptions) -> Result<FloorplanResult, PlanError> {
    plan_with_inputs(state, options, &PlanInputs::default())
}

/// Plan with transient RTL/connectivity-derived external-interface inputs.
pub fn plan_with_inputs(
    state: &WorkState,
    options: &PlanOptions,
    inputs: &PlanInputs,
) -> Result<FloorplanResult, PlanError> {
    plan_with_retry_ceiling(
        state,
        options,
        inputs,
        options.usage_limit.max(MAX_USAGE_LIMIT),
    )
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
/// instance twice — identical to the previous single-solver flow.
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
    calls: std::sync::atomic::AtomicUsize,
}

impl<'a> CountingSolver<'a> {
    fn new(inner: &'a dyn crate::solver::Solver) -> Self {
        Self {
            inner,
            calls: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    fn count(&self) -> usize {
        self.calls.load(std::sync::atomic::Ordering::Relaxed)
    }
}

impl crate::solver::Solver for CountingSolver<'_> {
    fn solve(
        &self,
        model: &crate::solver::LpModel,
        opts: &SolveOpts,
    ) -> Result<crate::solver::LpSolution, crate::solver::SolverError> {
        self.calls
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
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
        let (assignment, solved_strategy) = floorplan_with_exact_resource_caps(
            &graph,
            &device,
            caps.logic_utilization_cap,
            caps.effective_block_utilization_cap,
            strategy,
            &solver,
            &opts,
        )?;
        debug_assert_eq!(
            solved_strategy, strategy,
            "an explicitly resolved strategy must remain stable during placement",
        );
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
    mut assignment: crate::partition::Assignment,
    realized_caps: ExactDseResourceCaps,
) -> Result<FloorplanResult, PlanError> {
    let routes = plan_routes(
        graph,
        &assignment.regions,
        device,
        options.scheme,
        solver,
        opts,
    )?;
    // Deliberate headroom: the relaxed `plan` path validates the *realized*
    // usage — placement plus pipeline registers — against the retry ceiling
    // (`MAX_USAGE_LIMIT`), not the cap the placement was found at, so a
    // placement at 0.6 may absorb registers up to the ceiling. The exact
    // DSE path validates against its per-candidate logic/block caps instead.
    let slot_usage = if realized_caps.multilevel_block_margin_applied {
        realize_slot_usage_with_resource_caps(
            graph,
            &assignment.regions,
            &assignment.slot_usage,
            &routes,
            device,
            realized_caps.logic_utilization_cap,
            realized_caps.effective_block_utilization_cap,
        )?
    } else {
        realize_slot_usage(
            graph,
            &assignment.regions,
            &assignment.slot_usage,
            &routes,
            device,
            realized_caps.logic_utilization_cap,
        )?
    };
    graph.materialize_co_locations(&mut assignment.regions)?;
    graph.remove_transient_regions(&mut assignment.regions);

    Ok(FloorplanResult {
        device: device.key.clone(),
        grid: (device.cols, device.rows),
        regions: assignment.regions,
        routes,
        slot_usage,
    })
}

fn prepare_plan(
    state: &WorkState,
    inputs: &PlanInputs,
) -> Result<(crate::device::model::Device, FloorGraph), PlanError> {
    let part_num = state.flow.part_num.as_deref().ok_or(PlanError::NoPartNum)?;
    let device = select_device(part_num)?;
    validate_memory_platform(state, &device, !inputs.memory.is_empty())?;
    let mut validated_banks = std::collections::BTreeSet::new();
    for bank in inputs.memory.iter().map(|interface| interface.bank) {
        if !validated_banks.insert(bank) {
            continue;
        }
        let matches = device.slots_with_tag(&bank.to_string()).count();
        if matches != 1 {
            return Err(PlanError::BankTag { bank, matches });
        }
    }
    let flat = tapa_ir::flatten(&state.graph)?;
    let active_control = inputs.control.filter(|_| {
        flat.tasks
            .get(&flat.top)
            .is_some_and(|top| !top.tasks.is_empty())
    });
    let control_anchor = active_control
        .map(|control| select_control_anchor(&device, control))
        .transpose()?
        .flatten();
    let graph =
        FloorGraph::build_with_interfaces(&flat, &inputs.memory, active_control, control_anchor)?;
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

fn validate_memory_platform(
    state: &WorkState,
    device: &crate::device::model::Device,
    has_memory: bool,
) -> Result<(), PlanError> {
    let Some(expected) = device.platform_name.as_deref().filter(|_| has_memory) else {
        return Ok(());
    };
    let platform = state
        .flow
        .platform
        .as_deref()
        .ok_or_else(|| PlanError::PlatformRequired {
            expected: expected.to_string(),
        })?;
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
            "cflags": [], "top": "VecAdd", "target": "xilinx-hls",
            "tasks": {
                "VecAdd": {
                    "readable_name": "VecAdd", "code": "void VecAdd() {}", "level": "upper", "synth": "hls",
                    "ports": [],
                    "tasks": {
                        "A": [{"args": {"out": {"arg": "fifo", "cat": "ostream"}}, "step": 0}],
                        "B": [{"args": {"in": {"arg": "fifo", "cat": "istream"}}, "step": 0}]
                    },
                    "fifos": {"fifo": {"depth": 2, "consumed_by": ["B", 0], "produced_by": ["A", 0]}}
                },
                "A": {"readable_name": "A", "code": "void A() {}", "level": "lower", "synth": "hls",
                    "ports": [{"cat": "ostream", "name": "out", "type": "float", "width": 32}],
                    "self_area": {"lut": 100, "ff": 200}},
                "B": {"readable_name": "B", "code": "void B() {}", "level": "lower", "synth": "hls",
                    "ports": [{"cat": "istream", "name": "in", "type": "float", "width": 32}],
                    "self_area": {"lut": 50, "ff": 60}}
            }
        }"#;
        let graph = tapa_ir::TaskGraph::from_json(json).expect("parse");
        let mut state = WorkState::new(graph);
        state.flow.part_num = Some("xcu280-fsvh2892-2L-e".to_string());

        match plan(&state, &PlanOptions::default()) {
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
            "cflags": [], "top": "VecAdd", "target": "xilinx-vitis",
            "tasks": {
                "VecAdd": {
                    "readable_name": "VecAdd", "code": "void VecAdd() {}", "level": "upper", "synth": "hls",
                    "ports": [],
                    "tasks": {
                        "A": [{"args": {"out": {"arg": "fifo", "cat": "ostream"}}, "step": 0}],
                        "B": [{"args": {"in": {"arg": "fifo", "cat": "istream"}}, "step": 0}]
                    },
                    "fifos": {"fifo": {"depth": 2, "consumed_by": ["B", 0], "produced_by": ["A", 0]}}
                },
                "A": {"readable_name": "A", "code": "void A() {}", "level": "lower", "synth": "hls",
                    "ports": [{"cat": "ostream", "name": "out", "type": "float", "width": 32}],
                    "self_area": {"lut": 120000}},
                "B": {"readable_name": "B", "code": "void B() {}", "level": "lower", "synth": "hls",
                    "ports": [{"cat": "istream", "name": "in", "type": "float", "width": 32}],
                    "self_area": {"lut": 120000}}
            }
        }"#;
        let graph = tapa_ir::TaskGraph::from_json(json).expect("parse");
        let mut state = WorkState::new(graph);
        state.flow.part_num = Some("xcu280-fsvh2892-2L-e".to_string());

        match plan(&state, &PlanOptions::default()) {
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

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "the end-to-end assertion covers controller placement and all route inventories"
    )]
    fn distributed_control_plan_materializes_and_routes_exact_inventory() {
        let json = r#"{
            "cflags": [], "top": "Top", "target": "xilinx-vitis",
            "tasks": {
                "Top": {
                    "readable_name":"Top","code":"","level":"upper","synth":"hls",
                    "ports":[],"tasks":{
                        "Normal":[{"name":"normal#0","args":{},"step":0}],
                        "Ticker":[{"name":"ticker[1]","args":{},"step":-1}]
                    },"fifos":{}
                },
                "Normal": {"readable_name":"Normal","code":"","level":"lower","synth":"hls",
                    "ports":[],"self_area":{"lut":120000}},
                "Ticker": {"readable_name":"Ticker","code":"","level":"lower","synth":"hls",
                    "ports":[],"self_area":{"lut":120000}}
            }
        }"#;
        let graph = tapa_ir::TaskGraph::from_json(json).expect("parse");
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
        for instance in ["normal#0", "ticker[1]"] {
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
        assert!(xdc.contains(&tapa_ir::local_controller_instance_name("normal#0")));
        assert!(xdc.contains(&tapa_ir::local_controller_instance_name("ticker[1]")));
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
            r#"{"cflags": [], "top": "T", "target": "xilinx-hls",
                "tasks": {"T": {"readable_name": "T", "code": "void T(){}", "level": "upper",
                    "synth": "hls", "ports": [], "tasks": {}, "fifos": {}}}}"#,
        )
        .expect("parse");
        let state = WorkState::new(graph);
        assert!(matches!(
            plan(&state, &PlanOptions::default()),
            Err(PlanError::NoPartNum)
        ));
    }

    #[test]
    fn exact_memory_map_requires_its_recorded_platform() {
        let graph = tapa_ir::TaskGraph::from_json(
            r#"{"cflags": [], "top": "T", "target": "xilinx-vitis",
                "tasks": {"T": {"readable_name": "T", "code": "void T(){}", "level": "upper",
                    "synth": "hls", "ports": [], "tasks": {}, "fifos": {}}}}"#,
        )
        .expect("parse");
        let mut state = WorkState::new(graph);
        let device = select_device("u280").expect("u280");

        assert!(matches!(
            validate_memory_platform(&state, &device, true),
            Err(PlanError::PlatformRequired { .. })
        ));

        state.flow.platform = Some("/opt/xilinx/platforms/wrong_shell".to_string());
        assert!(matches!(
            validate_memory_platform(&state, &device, true),
            Err(PlanError::PlatformMismatch { .. })
        ));

        state.flow.platform = device
            .platform_name
            .as_ref()
            .map(|name| format!("/opt/xilinx/platforms/{name}"));
        validate_memory_platform(&state, &device, true).expect("matching platform");
        validate_memory_platform(&state, &device, false).expect("memory-free plans are part-only");
    }

    #[test]
    fn devices_without_exact_bank_tags_reject_memory_inputs_before_solving() {
        let graph = tapa_ir::TaskGraph::from_json(
            r#"{"cflags": [], "top": "T", "target": "xilinx-vitis",
                "tasks": {"T": {"readable_name": "T", "code": "void T(){}", "level": "upper",
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
        };

        for part in ["xcu250-figd2104-2L-e", "xcvc1902-vsva2197-2MP-e-S"] {
            let mut state = WorkState::new(graph.clone());
            state.flow.part_num = Some(part.to_string());
            let error = plan_with_inputs(
                &state,
                &PlanOptions::default(),
                &PlanInputs {
                    memory: vec![interface.clone()],
                    control: None,
                },
            )
            .expect_err("an unmodeled bank must fail before CBC");
            assert!(matches!(error, PlanError::BankTag { matches: 0, .. }));
        }
    }

    #[test]
    fn plan_options_fail_before_solver_or_device_lookup() {
        let graph = tapa_ir::TaskGraph::from_json(
            r#"{"cflags": [], "top": "T", "target": "xilinx-hls",
                "tasks": {"T": {"readable_name": "T", "code": "void T(){}", "level": "upper",
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
            assert!(matches!(plan(&state, &options), Err(PlanError::Options(_))));
        }
    }
}
