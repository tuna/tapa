//! `tapa floorplan` — coarse-grained floorplanning between `synth` and `pack`.
//!
//! Loads the synthesized work state, plans a placement with `tapa-floorplan`,
//! stores the resulting [`FloorplanResult`](tapa_ir::FloorplanResult) back into
//! `tapa.json`, and writes the pblock constraints to `<work_dir>/floorplan.xdc`.
//! Its presence in the state switches codegen and `pack` onto the floorplanned
//! path.

use std::path::{Path, PathBuf};

use clap::{Parser, ValueEnum};
use tapa_codegen::rtl_state::DirectMmapInterface;
use tapa_floorplan::{
    dse::{explore, DseCandidate, DseOptions},
    plan_with_inputs, render_xdc, ControlInterface, MemoryInterface, PartitionStrategy, PlanInputs,
    PlanOptions,
};
use tapa_ir::{Design, MemoryBindings, PipelineScheme, WorkState};

use crate::context::CliContext;
use crate::error::{CliError, Result};
use crate::state::{json, work as work_io};
use crate::steps::synth::rtl_codegen::{
    collect_hdl_inputs, emit_prepared_rtl_tree, prepare_rtl_state, TaskHdlInputs,
};

mod implementation;

/// Name of the emitted pblock constraints file in the work directory.
pub const FLOORPLAN_XDC: &str = "floorplan.xdc";
/// Name of the exact Vitis connectivity input staged with a floorplan.
pub const FLOORPLAN_CONNECTIVITY: &str = "floorplan-connectivity.ini";

/// A connectivity input read once, syntax-checked, and retained verbatim for
/// both planning and the later link step.
#[derive(Debug)]
pub(crate) struct ConnectivityInput {
    pub bytes: Vec<u8>,
    pub bindings: MemoryBindings,
}

pub(crate) fn read_connectivity(path: Option<&Path>) -> Result<Option<ConnectivityInput>> {
    let Some(path) = path else {
        return Ok(None);
    };
    let bytes = fs_err::read(path).map_err(|error| {
        CliError::InvalidArg(format!(
            "cannot read connectivity file `{}`: {error}",
            path.display(),
        ))
    })?;
    let text = std::str::from_utf8(&bytes).map_err(|error| {
        CliError::InvalidArg(format!(
            "connectivity file `{}` is not UTF-8: {error}",
            path.display(),
        ))
    })?;
    let bindings = MemoryBindings::parse_vitis_config(text).map_err(|error| {
        CliError::InvalidArg(format!(
            "invalid connectivity file `{}`: {error}",
            path.display(),
        ))
    })?;
    Ok(Some(ConnectivityInput { bytes, bindings }))
}

fn memory_plan_inputs(
    top: &str,
    interfaces: &[DirectMmapInterface],
    connectivity: Option<&ConnectivityInput>,
) -> Result<PlanInputs> {
    let bindings = connectivity.map(|input| &input.bindings);

    if interfaces.is_empty() {
        if let Some(binding) = bindings.and_then(|bindings| bindings.iter().next()) {
            return Err(CliError::InvalidArg(format!(
                "connectivity binding `{}` does not name a direct M-AXI port of kernel `{top}`",
                binding.endpoint,
            )));
        }
        return Ok(PlanInputs::default());
    }

    let bindings = bindings.ok_or_else(|| {
        let entries = interfaces
            .iter()
            .map(|interface| format!("sp={top}.{}:<bank>", interface.endpoint.top_port))
            .collect::<Vec<_>>()
            .join(", ");
        CliError::InvalidArg(format!(
            "floorplanning direct M-AXI ports requires `--connectivity` with {entries}",
        ))
    })?;

    for binding in bindings.iter() {
        let known = binding.endpoint.kernel == top
            && interfaces
                .iter()
                .any(|interface| interface.endpoint.top_port == binding.endpoint.port);
        if !known {
            return Err(CliError::InvalidArg(format!(
                "connectivity binding `{}` does not name a direct M-AXI port of kernel `{top}`",
                binding.endpoint,
            )));
        }
    }

    let memory = interfaces
        .iter()
        .map(|interface| {
            let bank = bindings
                .get(top, &interface.endpoint.top_port)
                .ok_or_else(|| {
                    CliError::InvalidArg(format!(
                        "connectivity is missing `sp={top}.{}:<bank>` for direct M-AXI endpoint `{}.{}`",
                        interface.endpoint.top_port,
                        interface.endpoint.instance,
                        interface.endpoint.port,
                    ))
                })?;
            Ok(MemoryInterface {
                endpoint: interface.endpoint.clone(),
                bank,
                channel_widths: interface.channel_widths,
                bridge_instance: interface.bridge_instance.clone(),
            })
        })
        .collect::<Result<Vec<_>>>()?;

    Ok(PlanInputs {
        memory,
        ..PlanInputs::default()
    })
}

/// Replace the active floorplan marker only after all dependent outputs are
/// ready. Removing an earlier marker first prevents `pack` from consuming RTL
/// or state left partially updated by a failed regeneration.
fn publish_floorplan_after_update(
    work_dir: &Path,
    xdc: &str,
    connectivity: Option<&[u8]>,
    update: impl FnOnce() -> Result<()>,
) -> Result<()> {
    for file_name in [
        FLOORPLAN_XDC,
        FLOORPLAN_CONNECTIVITY,
        implementation::LEGACY_IMPLEMENTATION_XCLBIN,
        implementation::IMPLEMENTATION_TIMING_REPORT,
        implementation::IMPLEMENTATION_METRICS,
    ] {
        match fs_err::remove_file(work_dir.join(file_name)) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }

    update()?;
    if let Some(bytes) = connectivity {
        json::write_bytes_atomic(work_dir, FLOORPLAN_CONNECTIVITY, bytes)?;
    }
    // This is deliberately last: its presence is the publication marker that
    // all dependent RTL, state, and optional connectivity are ready.
    json::write_bytes_atomic(work_dir, FLOORPLAN_XDC, xdc.as_bytes())
}

/// CLI spelling of [`PipelineScheme`], with `snake_case` values matching the
/// contract's serde tags.
#[derive(Debug, Clone, Copy, ValueEnum)]
#[value(rename_all = "snake_case")]
pub enum PpScheme {
    Single,
    Double,
    SingleHDoubleV,
}

impl From<PpScheme> for PipelineScheme {
    fn from(scheme: PpScheme) -> Self {
        match scheme {
            PpScheme::Single => Self::Single,
            PpScheme::Double => Self::Double,
            PpScheme::SingleHDoubleV => Self::SingleHDoubleV,
        }
    }
}

/// Placement subdivision schedule exposed by the CLI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "kebab-case")]
pub enum PartitionMode {
    /// Select flat or multilevel placement with the built-in heuristic.
    Auto,
    /// Place directly into atomic slots with one ILP.
    Flat,
    /// Place into rows first, then refine jointly into atomic slots.
    MultiLevel,
}

impl From<PartitionMode> for PartitionStrategy {
    fn from(mode: PartitionMode) -> Self {
        match mode {
            PartitionMode::Auto => Self::Auto,
            PartitionMode::Flat => Self::Flat,
            PartitionMode::MultiLevel => Self::MultiLevel,
        }
    }
}

#[derive(Debug, Parser)]
#[command(
    name = "floorplan",
    about = "Coarse-grained floorplan the synthesized design."
)]
pub struct FloorplanArgs {
    /// Per-slot resource utilization target for a non-DSE plan; raised on infeasibility.
    #[arg(
        long = "usage-limit",
        default_value_t = 0.7,
        value_parser = parse_usage_limit,
        conflicts_with = "dse"
    )]
    pub usage_limit: f64,

    /// Placement subdivision schedule.
    #[arg(
        long = "partition-strategy",
        value_enum,
        default_value_t = PartitionMode::Auto
    )]
    pub partition_strategy: PartitionMode,

    /// How pipeline registers are distributed across a crossing's route.
    #[arg(long = "pp-scheme", value_enum, default_value_t = PpScheme::Double)]
    pub pp_scheme: PpScheme,

    /// ILP time limit, in seconds.
    #[arg(
        long = "max-seconds",
        default_value_t = 600,
        value_parser = parse_positive_u64
    )]
    pub max_seconds: u64,

    /// Vitis link configuration containing memory `sp=` assignments.
    #[arg(long = "connectivity", value_name = "FILE")]
    pub connectivity: Option<PathBuf>,

    /// Run hardware implementation and report the achieved kernel frequency.
    #[arg(long = "run-impl")]
    pub run_impl: bool,

    /// Explore exact logic-utilization caps and keep the highest-frequency implementation.
    #[arg(long = "dse")]
    pub dse: bool,

    /// Lowest exact logic-utilization cap explored by `--dse`.
    #[arg(
        long = "dse-min",
        default_value_t = 0.55,
        value_parser = parse_usage_limit,
        requires = "dse"
    )]
    pub dse_min: f64,

    /// Highest and first exact logic-utilization cap explored by `--dse`.
    #[arg(
        long = "dse-max",
        default_value_t = 0.90,
        value_parser = parse_usage_limit,
        requires = "dse"
    )]
    pub dse_max: f64,

    /// Nominal utilization-cap decrease between DSE attempts.
    #[arg(
        long = "dse-step",
        default_value_t = 0.03,
        value_parser = parse_usage_limit,
        requires = "dse"
    )]
    pub dse_step: f64,

    /// Maximum number of candidate package/link jobs run concurrently.
    #[arg(
        long = "dse-jobs",
        default_value_t = 1,
        value_parser = parse_positive_usize,
        requires = "dse"
    )]
    pub dse_jobs: usize,
}

fn parse_usage_limit(value: &str) -> std::result::Result<f64, String> {
    let parsed = value
        .parse::<f64>()
        .map_err(|_| format!("`{value}` is not a number"))?;
    if parsed.is_finite() && parsed > 0.0 && parsed <= 1.0 {
        Ok(parsed)
    } else {
        Err(format!(
            "usage limit must be finite and in the range (0, 1], got {value}",
        ))
    }
}

fn parse_positive_u64(value: &str) -> std::result::Result<u64, String> {
    let parsed = value
        .parse::<u64>()
        .map_err(|_| format!("`{value}` is not a non-negative integer"))?;
    if parsed == 0 {
        Err("max seconds must be greater than zero".to_string())
    } else {
        Ok(parsed)
    }
}

fn parse_positive_usize(value: &str) -> std::result::Result<usize, String> {
    let parsed = value
        .parse::<usize>()
        .map_err(|_| format!("`{value}` is not a non-negative integer"))?;
    if parsed == 0 {
        Err("DSE jobs must be greater than zero".to_string())
    } else {
        Ok(parsed)
    }
}

fn plan_implementation_candidates(
    args: &FloorplanArgs,
    state: &WorkState,
    options: &PlanOptions,
    inputs: &PlanInputs,
    dse_options: &DseOptions,
) -> Result<(
    Vec<implementation::PlannedCandidate>,
    Vec<implementation::InfeasibleCandidate>,
)> {
    if !args.dse {
        let floorplan = plan_with_inputs(state, options, inputs)
            .map_err(|error| CliError::Floorplan(error.to_string()))?;
        let realized_max_utilization = implementation::realized_max_utilization(&floorplan)?;
        return Ok((
            vec![implementation::PlannedCandidate {
                index: 0,
                requested_utilization_cap: args.usage_limit,
                effective_block_utilization_cap: None,
                multilevel_block_margin_applied: false,
                utilization_cap_policy: implementation::UtilizationCapPolicy::Relaxing,
                realized_max_utilization,
                floorplan,
            }],
            Vec::new(),
        ));
    }

    let explored = explore(state, options, inputs, dse_options)
        .map_err(|error| CliError::Floorplan(error.to_string()))?;
    let mut candidates = Vec::new();
    let mut infeasible = Vec::new();
    for (index, candidate) in explored.into_iter().enumerate() {
        match candidate {
            DseCandidate::Feasible {
                logic_utilization_cap,
                effective_block_utilization_cap,
                multilevel_block_margin_applied,
                max_utilization,
                floorplan,
            } => candidates.push(implementation::PlannedCandidate {
                index,
                requested_utilization_cap: logic_utilization_cap,
                effective_block_utilization_cap: Some(effective_block_utilization_cap),
                multilevel_block_margin_applied,
                utilization_cap_policy: implementation::UtilizationCapPolicy::Exact,
                realized_max_utilization: max_utilization,
                floorplan,
            }),
            DseCandidate::Infeasible {
                logic_utilization_cap,
                effective_block_utilization_cap,
                multilevel_block_margin_applied,
            } => {
                infeasible.push(implementation::InfeasibleCandidate {
                    index,
                    requested_utilization_cap: logic_utilization_cap,
                    effective_block_utilization_cap,
                    multilevel_block_margin_applied,
                });
            }
        }
    }
    Ok((candidates, infeasible))
}

fn publish_implementation_winner(
    work_dir: &Path,
    state: &mut WorkState,
    flat: &Design,
    hdl_inputs: &TaskHdlInputs,
    connectivity: Option<&ConnectivityInput>,
    target_mhz: u32,
    winner: &implementation::ImplementationWinner,
) -> Result<()> {
    let result = winner.floorplan().clone();
    let xdc = render_xdc(&result).map_err(|error| CliError::Floorplan(error.to_string()))?;
    publish_floorplan_after_update(
        work_dir,
        &xdc,
        connectivity.map(|input| input.bytes.as_slice()),
        || {
            let mut canonical_rtl = prepare_rtl_state(flat, hdl_inputs)?;
            canonical_rtl.floorplan = Some(result.clone());
            emit_prepared_rtl_tree(work_dir, &mut canonical_rtl, hdl_inputs)?;

            state.floorplan = Some(result);
            work_io::store(work_dir, state)?;
            winner.publish_artifacts(work_dir, target_mhz)
        },
    )?;
    winner.log_selection(work_dir);
    Ok(())
}

pub fn run(args: &FloorplanArgs, ctx: &CliContext) -> Result<()> {
    let options = PlanOptions {
        usage_limit: args.usage_limit,
        max_seconds: args.max_seconds,
        threads: 1,
        partition_strategy: args.partition_strategy.into(),
        scheme: args.pp_scheme.into(),
    };
    options
        .validate()
        .map_err(|error| CliError::InvalidArg(error.to_string()))?;
    let dse_options = DseOptions {
        min: args.dse_min,
        max: args.dse_max,
        step: args.dse_step,
    };
    if args.dse {
        dse_options
            .validate()
            .map_err(|error| CliError::InvalidArg(error.to_string()))?;
    }

    let mut state = work_io::load(&ctx.work_dir)?;
    if !state.flow.synthed {
        return Err(CliError::Floorplan(
            "run `synth` before `floorplan`: the placement needs per-task areas".to_string(),
        ));
    }
    let implementation_target = (args.run_impl || args.dse)
        .then(|| implementation::validate_target(&state))
        .transpose()?;
    let connectivity = read_connectivity(args.connectivity.as_deref())?;
    if let Some(input) = &connectivity {
        log::debug!(
            "parsed {} memory-bank binding(s) from connectivity input",
            input.bindings.len(),
        );
    }

    // Prepare the exact flattened RTL once. The parsed state is available to
    // derive interface-aware planner inputs before solving and is then reused
    // directly for code generation below.
    let flat = tapa_ir::flatten(&state.graph)
        .map_err(|e| CliError::Floorplan(format!("flatten failed: {e}")))?;
    let hdl_inputs = collect_hdl_inputs(&ctx.work_dir, &flat)?;
    let mut rtl_state = prepare_rtl_state(&flat, &hdl_inputs)?;

    let interfaces = rtl_state
        .direct_mmap_interfaces(&flat.top)
        .map_err(|error| CliError::Floorplan(error.to_string()))?;
    let mut inputs = memory_plan_inputs(&flat.top, &interfaces, connectivity.as_ref())?;
    inputs.control = rtl_state
        .supports_distributed_control()
        .then(|| ControlInterface {
            has_s_axi_control: rtl_state.top_instantiates_control_s_axi(),
        });

    if let Some(target) = implementation_target.as_ref() {
        let (candidates, infeasible) =
            plan_implementation_candidates(args, &state, &options, &inputs, &dse_options)?;
        let jobs = if args.dse { args.dse_jobs } else { 1 };
        let implementation_state = state.clone();
        return ctx.with_tool_runner(|runner| {
            implementation::implement_and_publish(
                runner,
                &ctx.work_dir,
                &implementation_state,
                &flat,
                &hdl_inputs,
                connectivity.as_ref().map(|input| input.bytes.as_slice()),
                target,
                candidates,
                &infeasible,
                jobs,
                |winner| {
                    publish_implementation_winner(
                        &ctx.work_dir,
                        &mut state,
                        &flat,
                        &hdl_inputs,
                        connectivity.as_ref(),
                        target.target_mhz(),
                        winner,
                    )
                },
            )
        });
    }

    let result = plan_with_inputs(&state, &options, &inputs)
        .map_err(|e| CliError::Floorplan(e.to_string()))?;
    let xdc = render_xdc(&result).map_err(|e| CliError::Floorplan(e.to_string()))?;

    publish_floorplan_after_update(
        &ctx.work_dir,
        &xdc,
        connectivity.as_ref().map(|input| input.bytes.as_slice()),
        || {
            rtl_state.floorplan = Some(result.clone());
            emit_prepared_rtl_tree(&ctx.work_dir, &mut rtl_state, &hdl_inputs)?;

            state.floorplan = Some(result);
            work_io::store(&ctx.work_dir, &state)
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tapa_ir::{AxiChannelWidths, AxiEndpoint, MemoryBank, MemoryKind, TaskGraph, WorkState};

    fn ctx_at(work_dir: &std::path::Path) -> CliContext {
        CliContext {
            work_dir: work_dir.to_path_buf(),
            temp_dir: None,
            clang_format_quota_in_bytes: 0,
            remote_config: None,
            verbose: 0,
            quiet: 0,
        }
    }

    /// A minimal synthesized vadd state: top `VecAdd` with A -> fifo -> B.
    fn synthed_vadd_state() -> WorkState {
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
                    "self_area": {"LUT": 100, "FF": 200}},
                "B": {"readable_name": "B", "code": "void B() {}", "level": "lower", "synth": "hls",
                    "ports": [{"cat": "istream", "name": "in", "type": "float", "width": 32}],
                    "self_area": {"LUT": 50, "FF": 60}}
            }
        }"#;
        let graph = TaskGraph::from_json(json).expect("parse vadd graph");
        let mut state = WorkState::new(graph);
        state.flow.part_num = Some("xcu280-fsvh2892-2L-e".to_string());
        state.flow.synthed = true;
        state
    }

    fn synthed_direct_mmap_state() -> WorkState {
        let graph = TaskGraph::from_json(
            r#"{
                "cflags": [], "top": "Top", "target": "xilinx-vitis",
                "tasks": {
                    "Top": {
                        "readable_name": "Top", "code": "", "level": "upper", "synth": "hls",
                        "ports": [{"cat":"mmap","name":"mem","type":"int*","width":32}],
                        "tasks": {"Reader": [{"args":{"data":{"arg":"mem","cat":"mmap"}}}]},
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
        .expect("parse mmap graph");
        let mut state = WorkState::new(graph);
        state.flow.part_num = Some("xcu280-fsvh2892-2L-e".to_string());
        state.flow.platform = Some("xilinx_u280_gen3x16_xdma_1_202211_1".to_string());
        state.flow.synthed = true;
        state
    }

    fn direct_mmap_interface(
        instance: &str,
        child_port: &str,
        top_port: &str,
    ) -> DirectMmapInterface {
        DirectMmapInterface {
            endpoint: AxiEndpoint {
                instance: instance.to_string(),
                port: child_port.to_string(),
                top_port: top_port.to_string(),
            },
            data_width: 32,
            addr_width: 64,
            id_width: 1,
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

    fn connectivity_input(text: &str) -> ConnectivityInput {
        ConnectivityInput {
            bytes: text.as_bytes().to_vec(),
            bindings: MemoryBindings::parse_vitis_config(text).expect("parse connectivity"),
        }
    }

    #[test]
    fn exact_connectivity_becomes_transient_memory_input() {
        let interfaces = [direct_mmap_interface("Reader_0", "data", "input")];
        let connectivity = connectivity_input("[connectivity]\nsp=Top.input:HBM[7]\n");

        let inputs = memory_plan_inputs("Top", &interfaces, Some(&connectivity)).expect("inputs");

        assert_eq!(
            inputs.memory,
            vec![MemoryInterface {
                endpoint: interfaces[0].endpoint.clone(),
                bank: MemoryBank {
                    kind: MemoryKind::Hbm,
                    index: 7,
                },
                channel_widths: interfaces[0].channel_widths,
                bridge_instance: None,
            }]
        );
    }

    #[test]
    fn direct_mmap_requires_connectivity() {
        let interfaces = [direct_mmap_interface("Reader_0", "data", "input")];

        let error = memory_plan_inputs("Top", &interfaces, None).expect_err("missing input");

        assert!(matches!(error, CliError::InvalidArg(ref message)
            if message.contains("--connectivity") && message.contains("sp=Top.input:<bank>")));
    }

    #[test]
    fn connectivity_must_cover_every_direct_mmap() {
        let interfaces = [
            direct_mmap_interface("Reader_0", "data", "input"),
            direct_mmap_interface("Writer_0", "data", "output"),
        ];
        let connectivity = connectivity_input("[connectivity]\nsp=Top.input:HBM[0]\n");

        let error = memory_plan_inputs("Top", &interfaces, Some(&connectivity))
            .expect_err("missing output binding");

        assert!(matches!(error, CliError::InvalidArg(ref message)
            if message.contains("sp=Top.output:<bank>") && message.contains("Writer_0.data")));
    }

    #[test]
    fn connectivity_rejects_unknown_kernel_or_port() {
        let interfaces = [direct_mmap_interface("Reader_0", "data", "input")];
        for endpoint in ["Other.input", "Top.unknown"] {
            let connectivity =
                connectivity_input(&format!("[connectivity]\nsp={endpoint}:HBM[0]\n"));

            let error = memory_plan_inputs("Top", &interfaces, Some(&connectivity))
                .expect_err("unknown binding");

            assert!(matches!(error, CliError::InvalidArg(ref message)
                if message.contains(endpoint) && message.contains("kernel `Top`")));
        }
    }

    #[test]
    fn no_memory_design_rejects_stray_sp_binding() {
        let connectivity = connectivity_input("[connectivity]\nsp=Top.input:HBM[0]\n");

        let error = memory_plan_inputs("Top", &[], Some(&connectivity)).expect_err("stray binding");

        assert!(matches!(error, CliError::InvalidArg(ref message)
            if message.contains("Top.input") && message.contains("direct M-AXI")));
    }

    #[test]
    fn floorplan_args_reject_unsafe_solver_limits() {
        for value in ["0", "-0.1", "1.01", "NaN", "inf"] {
            FloorplanArgs::try_parse_from(["floorplan", "--usage-limit", value])
                .expect_err("invalid usage limit");
        }
        FloorplanArgs::try_parse_from(["floorplan", "--max-seconds", "0"])
            .expect_err("zero-second solve");
    }

    #[test]
    fn floorplan_args_accept_partition_strategies() {
        for (value, expected) in [
            ("auto", PartitionMode::Auto),
            ("flat", PartitionMode::Flat),
            ("multi-level", PartitionMode::MultiLevel),
        ] {
            let args = FloorplanArgs::try_parse_from(["floorplan", "--partition-strategy", value])
                .expect("valid partition strategy");
            assert_eq!(args.partition_strategy, expected);
        }
    }

    #[test]
    fn floorplan_args_accept_connectivity_file() {
        let args =
            FloorplanArgs::try_parse_from(["floorplan", "--connectivity", "configs/link.ini"])
                .expect("connectivity argument");
        assert_eq!(args.connectivity, Some(PathBuf::from("configs/link.ini")));
    }

    #[test]
    fn floorplan_args_validate_minimal_dse_surface() {
        let args =
            FloorplanArgs::try_parse_from(["floorplan", "--dse"]).expect("DSE defaults must parse");
        assert!(args.dse);
        assert!((args.dse_min - 0.55).abs() < f64::EPSILON);
        assert!((args.dse_max - 0.90).abs() < f64::EPSILON);
        assert!((args.dse_step - 0.03).abs() < f64::EPSILON);
        assert_eq!(args.dse_jobs, 1);

        for argv in [
            vec!["floorplan", "--dse", "--dse-jobs", "0"],
            vec!["floorplan", "--dse-min", "0.5"],
            vec!["floorplan", "--dse", "--usage-limit", "0.7"],
        ] {
            FloorplanArgs::try_parse_from(argv).expect_err("invalid DSE arguments");
        }

        let dir = tempfile::tempdir().expect("tempdir");
        let invalid_range = FloorplanArgs::try_parse_from([
            "floorplan",
            "--dse",
            "--dse-min",
            "0.9",
            "--dse-max",
            "0.5",
        ])
        .expect("individual limits parse");
        let error = run(&invalid_range, &ctx_at(dir.path()))
            .expect_err("DSE range must be validated before work-dir I/O");
        assert!(matches!(error, CliError::InvalidArg(_)));
    }

    #[test]
    fn direct_floorplan_run_validates_options_before_io() {
        let dir = tempfile::tempdir().expect("tempdir");
        let err = run(
            &FloorplanArgs {
                usage_limit: 0.0,
                partition_strategy: PartitionMode::Auto,
                pp_scheme: PpScheme::Double,
                max_seconds: 60,
                connectivity: None,
                run_impl: false,
                dse: false,
                dse_min: 0.55,
                dse_max: 0.90,
                dse_step: 0.03,
                dse_jobs: 1,
            },
            &ctx_at(dir.path()),
        )
        .expect_err("invalid usage limit");
        assert!(matches!(err, CliError::InvalidArg(_)));
    }

    #[test]
    fn failed_floorplan_update_leaves_outputs_inactive() {
        let dir = tempfile::tempdir().expect("tempdir");
        let active_path = dir.path().join(FLOORPLAN_XDC);
        let connectivity_path = dir.path().join(FLOORPLAN_CONNECTIVITY);
        let legacy_xclbin_path = dir
            .path()
            .join(implementation::LEGACY_IMPLEMENTATION_XCLBIN);
        fs_err::write(&active_path, "old constraints").expect("old xdc");
        fs_err::write(&connectivity_path, "old connectivity").expect("old connectivity");
        fs_err::write(&legacy_xclbin_path, "old xclbin").expect("old xclbin");

        let err = publish_floorplan_after_update(
            dir.path(),
            "new constraints",
            Some(b"new connectivity"),
            || {
                Err(CliError::Codegen(
                    "injected regeneration failure".to_string(),
                ))
            },
        )
        .expect_err("update must fail");

        assert!(
            matches!(err, CliError::Codegen(ref message) if message == "injected regeneration failure"),
            "the original failure must be preserved: {err}",
        );
        assert!(
            !active_path.exists(),
            "a failed update must not leave an XDC marker for pack",
        );
        assert!(
            !connectivity_path.exists(),
            "a failed update must not leave staged connectivity active",
        );
        assert!(
            !legacy_xclbin_path.exists(),
            "a new floorplan must remove obsolete implementation artifacts",
        );
    }

    #[test]
    fn connectivity_is_syntax_checked_and_kept_byte_for_byte() {
        let dir = tempfile::tempdir().expect("tempdir");
        let source_path = dir.path().join("input.ini");
        let bytes = b"[connectivity]\r\n# keep this spelling\r\nsp=Top.mem:HBM[3]\r\n";
        fs_err::write(&source_path, bytes).expect("write connectivity");

        let input = read_connectivity(Some(&source_path))
            .expect("read connectivity")
            .expect("present");
        assert_eq!(input.bindings.len(), 1);
        publish_floorplan_after_update(dir.path(), "constraints", Some(&input.bytes), || Ok(()))
            .expect("publish");

        assert_eq!(
            fs_err::read(dir.path().join(FLOORPLAN_CONNECTIVITY)).expect("read staged"),
            bytes,
            "the link step must receive the exact configuration that was parsed",
        );
        assert!(dir.path().join(FLOORPLAN_XDC).is_file());
    }

    #[test]
    fn malformed_connectivity_is_rejected_before_publication() {
        let dir = tempfile::tempdir().expect("tempdir");
        let source_path = dir.path().join("bad.ini");
        fs_err::write(&source_path, "[connectivity]\nsp=Top.mem:HBM[x]\n")
            .expect("write connectivity");

        let error = read_connectivity(Some(&source_path)).expect_err("syntax must fail");
        assert!(matches!(error, CliError::InvalidArg(_)));
        assert!(!dir.path().join(FLOORPLAN_CONNECTIVITY).exists());
        assert!(!dir.path().join(FLOORPLAN_XDC).exists());
    }

    #[test]
    fn direct_m_axi_errors_are_reported_before_solver_launch() {
        for (connectivity, expected) in [
            (None, "--connectivity"),
            (Some("sp=Other.mem:HBM[0]"), "Other.mem"),
            (Some("sp=Top.mem:HBM[32]"), "maps to 0 device slots"),
        ] {
            let dir = tempfile::tempdir().expect("tempdir");
            work_io::store(dir.path(), &synthed_direct_mmap_state()).expect("store");
            write_hls_module(
                dir.path(),
                "Top",
                "module Top(input wire ap_clk, input wire ap_rst_n); endmodule",
            );
            write_direct_mmap_hls_module(dir.path());
            let connectivity_path = connectivity.map(|binding| {
                let path = dir.path().join("connectivity.ini");
                fs_err::write(&path, format!("[connectivity]\n{binding}\n"))
                    .expect("write connectivity");
                path
            });
            let error = run(
                &FloorplanArgs {
                    usage_limit: 0.7,
                    partition_strategy: PartitionMode::Auto,
                    pp_scheme: PpScheme::Double,
                    max_seconds: 60,
                    connectivity: connectivity_path,
                    run_impl: false,
                    dse: false,
                    dse_min: 0.55,
                    dse_max: 0.90,
                    dse_step: 0.03,
                    dse_jobs: 1,
                },
                &ctx_at(dir.path()),
            )
            .expect_err("invalid memory input must fail before CBC");

            assert!(error.to_string().contains(expected), "got {error}");
            assert!(!dir.path().join(FLOORPLAN_XDC).exists());
        }
    }

    #[test]
    fn floorplan_step_writes_xdc_and_result() {
        let dir = tempfile::tempdir().expect("tempdir");
        work_io::store(dir.path(), &synthed_vadd_state()).expect("store state");
        write_hls_module(
            dir.path(),
            "VecAdd",
            "module VecAdd(input wire ap_clk, input wire ap_rst_n); endmodule",
        );
        write_hls_module(
            dir.path(),
            "A",
            "module A(input wire ap_clk, input wire ap_rst_n, output wire [31:0] out_din, output wire out_write, input wire out_full_n); endmodule",
        );
        write_hls_module(
            dir.path(),
            "B",
            "module B(input wire ap_clk, input wire ap_rst_n, input wire [31:0] in_dout, input wire in_empty_n, output wire in_read); endmodule",
        );
        let ctx = ctx_at(dir.path());
        let args = FloorplanArgs {
            usage_limit: 0.7,
            partition_strategy: PartitionMode::Auto,
            pp_scheme: PpScheme::Double,
            max_seconds: 60,
            connectivity: None,
            run_impl: false,
            dse: false,
            dse_min: 0.55,
            dse_max: 0.90,
            dse_step: 0.03,
            dse_jobs: 1,
        };

        match run(&args, &ctx) {
            Ok(()) => {
                let xdc = fs_err::read_to_string(dir.path().join(FLOORPLAN_XDC)).expect("xdc");
                assert!(xdc.contains("create_pblock SLOT_X"), "xdc has pblocks");
                let reloaded = work_io::load(dir.path()).expect("reload");
                let floorplan = reloaded.floorplan.expect("floorplan stored");
                assert_eq!(floorplan.device, "u280");
                for controller in [
                    tapa_ir::global_controller_instance_name().to_string(),
                    tapa_ir::local_controller_instance_name("A_0"),
                    tapa_ir::local_controller_instance_name("B_0"),
                ] {
                    assert!(
                        floorplan.regions.contains_key(&controller),
                        "missing planned controller {controller}",
                    );
                }
                assert_eq!(
                    floorplan.regions.len(),
                    6,
                    "A_0, B_0, the FIFO, and three controller instances",
                );
            }
            Err(CliError::Floorplan(msg)) if msg.contains("cbc") || msg.contains("solver") => {
                eprintln!("skipping floorplan_step: cbc not available ({msg})");
            }
            Err(other) => panic!("floorplan step failed: {other}"),
        }
    }

    /// Write a fake HLS Verilog module under `hls/<task>/verilog/<task>.v`.
    fn write_hls_module(work_dir: &std::path::Path, task: &str, src: &str) {
        let dir = work_dir.join("hls").join(task).join("verilog");
        fs_err::create_dir_all(&dir).expect("hls dir");
        fs_err::write(dir.join(format!("{task}.v")), src).expect("hls verilog");
    }

    fn write_direct_mmap_hls_module(work_dir: &std::path::Path) {
        let parsed = tapa_rtl::VerilogModule::parse(
            "module Reader(input wire ap_clk, input wire ap_rst_n); endmodule",
        )
        .expect("parse Reader");
        let mut module = tapa_rtl::mutation::MutableModule::from_parsed(parsed);
        tapa_codegen::m_axi::add_m_axi_ports_with_id_width(&mut module, "data", 32, 64, 1);
        write_hls_module(work_dir, "Reader", &module.emit());
    }

    fn assert_pipelined_floorplan_outputs(work_dir: &std::path::Path) {
        let top_v = fs_err::read_to_string(work_dir.join("rtl").join("VecAdd.v")).expect("top rtl");
        assert!(top_v.contains("tapa_hs_pipeline"), "got:\n{top_v}");
        assert!(
            top_v.contains("__tapa_global_controller")
                && top_v.contains("__tapa_local_controller_A_0")
                && top_v.contains("__tapa_local_controller_B_0"),
            "distributed controller hierarchy missing:\n{top_v}",
        );
        assert!(
            top_v.contains("__tapa_control_A_0_launch")
                || top_v.contains("__tapa_control_B_0_launch"),
            "cross-slot Launch pipeline missing:\n{top_v}",
        );
        assert!(work_dir.join("rtl").join("tapa_control.v").is_file());

        let xdc = fs_err::read_to_string(work_dir.join(FLOORPLAN_XDC)).expect("xdc");
        for hierarchy in [
            "TAPA_HS_HEAD",
            "TAPA_HS_BODY",
            "TAPA_HS_TAIL",
            "__tapa_global_controller",
            "__tapa_local_controller_A_0",
            "__tapa_local_controller_B_0",
        ] {
            assert!(xdc.contains(hierarchy), "missing {hierarchy}:\n{xdc}");
        }

        let floorplan = work_io::load(work_dir)
            .expect("reload floorplan")
            .floorplan
            .expect("stored floorplan");
        assert!(floorplan
            .regions
            .contains_key(tapa_ir::global_controller_instance_name()));
        assert!(
            floorplan
                .routes
                .iter()
                .any(|route| matches!(route.channel, tapa_ir::RoutedChannel::Control { .. })),
            "the forced crossing must publish typed control routes",
        );
    }

    #[test]
    fn floorplan_step_regenerates_head_body_tail_rtl() {
        // A and B are too large to share a u280 slot, so the stream between
        // them crosses a boundary and must use the floorplanned handshake
        // pipeline in the regenerated top RTL.
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
                    "self_area": {"LUT": 120000}},
                "B": {"readable_name": "B", "code": "void B() {}", "level": "lower", "synth": "hls",
                    "ports": [{"cat": "istream", "name": "in", "type": "float", "width": 32}],
                    "self_area": {"LUT": 120000}}
            }
        }"#;
        let graph = TaskGraph::from_json(json).expect("parse");
        let mut state = WorkState::new(graph);
        state.flow.part_num = Some("xcu280-fsvh2892-2L-e".to_string());
        state.flow.synthed = true;

        let dir = tempfile::tempdir().expect("tempdir");
        work_io::store(dir.path(), &state).expect("store");
        write_hls_module(
            dir.path(),
            "VecAdd",
            "module VecAdd(\n input wire ap_clk,\n input wire ap_rst_n\n);\nendmodule",
        );
        write_hls_module(
            dir.path(),
            "A",
            "module A(\n input wire ap_clk,\n input wire ap_rst_n,\n \
             output wire [31:0] out_din,\n output wire out_write,\n input wire out_full_n\n);\nendmodule",
        );
        write_hls_module(
            dir.path(),
            "B",
            "module B(\n input wire ap_clk,\n input wire ap_rst_n,\n \
             input wire [31:0] in_dout,\n input wire in_empty_n,\n output wire in_read\n);\nendmodule",
        );

        let args = FloorplanArgs {
            usage_limit: 0.7,
            partition_strategy: PartitionMode::Auto,
            pp_scheme: PpScheme::Double,
            max_seconds: 60,
            connectivity: None,
            run_impl: false,
            dse: false,
            dse_min: 0.55,
            dse_max: 0.90,
            dse_step: 0.03,
            dse_jobs: 1,
        };
        match run(&args, &ctx_at(dir.path())) {
            Ok(()) => assert_pipelined_floorplan_outputs(dir.path()),
            Err(CliError::Floorplan(msg)) if msg.contains("cbc") || msg.contains("solver") => {
                eprintln!(
                    "skipping floorplan_step_regenerates_head_body_tail_rtl: cbc not available ({msg})"
                );
            }
            Err(other) => panic!("floorplan step failed: {other}"),
        }
    }

    #[test]
    fn floorplan_before_synth_is_rejected() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut state = synthed_vadd_state();
        state.flow.synthed = false;
        work_io::store(dir.path(), &state).expect("store");
        let err = run(
            &FloorplanArgs {
                usage_limit: 0.7,
                partition_strategy: PartitionMode::Auto,
                pp_scheme: PpScheme::Double,
                max_seconds: 60,
                connectivity: None,
                run_impl: false,
                dse: false,
                dse_min: 0.55,
                dse_max: 0.90,
                dse_step: 0.03,
                dse_jobs: 1,
            },
            &ctx_at(dir.path()),
        )
        .expect_err("must require synth");
        assert!(matches!(err, CliError::Floorplan(_)));
    }
}
