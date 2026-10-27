//! `run_native` orchestrator for `tapa synth`.
//!
//! The AIE-only branch returns after its graph build and optional report
//! refresh. Otherwise this threads device resolution → state persistence →
//! manifest staging → HLS runs → RTL codegen → final state persistence. Also
//! owns the unsupported-flag gating, the HLS cflag construction, and
//! the recursive Verilog-file walker that feeds the codegen step.

use tapa_ir::{ClockPeriod, Task, WorkState};
use tapa_xilinx::{CsynthReport, ToolRunner};

use crate::context::CliContext;
use crate::error::{CliError, Result};
use crate::state::work as work_io;
use crate::tapacc::cflags::{get_remote_hls_cflags, get_tapacc_cflags};
use crate::tapacc::discover::find_resource;

use super::device_resolve::resolve_device_info;
use super::hls_run::{run_hls_for_leaves, HlsRunOptions};
use super::hls_sources::stage_hls_sources;
use super::post_synth_util::emit_post_synth_util;
use super::report::write_top_report;
use super::rtl_codegen::{generate_rtl_tree, write_templates_info, TaskHdlInputs};
use super::SynthArgs;

/// Validate the flag surface, resolve the device, persist settings,
/// then drive manifest staging → HLS → codegen for the leaf tasks.
#[allow(
    clippy::too_many_lines,
    reason = "orchestrator function; refactored extract would bounce values through another builder without adding clarity"
)]
pub fn run_native(args: &SynthArgs, ctx: &CliContext, runner: &dyn ToolRunner) -> Result<()> {
    // The flow target is typed and validated by the state parse itself;
    // synthesis is Xilinx HLS for every supported flow, and the target's
    // only other say is which platform shell device resolution accepts.
    let mut state = work_io::load(&ctx.work_dir)?;
    let partition = tapa_ir::AiePartition::derive(&state.graph)?;
    if args.aie_only && partition.is_none() {
        return Err(CliError::InvalidArg(
            "`--aie-only` requires an AI Engine partition, but the design has no AI Engine task"
                .to_string(),
        ));
    }
    if partition.is_some() && state.graph.target != tapa_ir::Target::XilinxVitis {
        return Err(CliError::InvalidArg(
            "AI Engine tasks are linked by `v++`; analyze with `--target xilinx-vitis`".to_string(),
        ));
    }
    if let Some(partition) = &partition {
        crate::aie::warn_narrow_boundaries(partition);
    }

    let device = resolve_device_info(args, state.graph.target)?;
    // The vendor tools speak nanoseconds in text; this is the one place
    // that reading becomes our own typed quantity.
    let clock_period = ClockPeriod::from_nanoseconds_str(&device.clock_period)
        .map_err(|error| CliError::InvalidArg(format!("invalid synthesis target: {error}")))?;
    if args.aie_only {
        return run_aie_only(
            args,
            ctx,
            runner,
            state,
            partition.as_ref().expect("checked above"),
            &device.part_num,
            clock_period,
        );
    }
    state.flow.part_num = Some(device.part_num.clone());
    state.flow.platform.clone_from(&args.platform);
    state.flow.clock_period = Some(clock_period);
    // A synthesis rerun may change RTL, area, device, or timing inputs. Make
    // every result derived from the previous RTL inactive before the long HLS
    // runs, so an interrupted rerun cannot be packed or floorplanned as if it
    // had completed successfully.
    state.flow.synthed = false;
    state.flow.skip_aie = args.skip_aie;
    state.floorplan = None;
    work_io::store(&ctx.work_dir, &state)?;

    let graph_dir = crate::aie::dir(&ctx.work_dir, "hw");
    let aie_report = match &partition {
        Some(partition) if !args.skip_aie => Some(build_aie(
            args,
            ctx,
            runner,
            &state,
            partition,
            clock_period,
        )?),
        // A graph left from an earlier run must not be packed with this one.
        _ if graph_dir.exists() => {
            fs_err::remove_dir_all(&graph_dir)?;
            None
        }
        _ => None,
    };
    // HLS and codegen see the PL half only; the state keeps the whole graph.
    let pl_graph = crate::aie::pl_graph(&state.graph)?;
    let sources = stage_hls_sources(&ctx.work_dir, &pl_graph)?;

    let opts = HlsRunOptions {
        part_num: device.part_num.clone(),
        clock_period,
        other_configs: args.other_hls_configs.clone(),
        cflags: build_hls_cflags(ctx.remote_config.is_some()),
        skip_based_on_mtime: args.skip_hls_based_on_mtime,
        require_cache_hit: args.require_hls_cache,
        jobs: args.jobs,
        keep_work_dir: args.keep_hls_work_dir,
    };
    let hls_results = run_hls_for_leaves(runner, &ctx.work_dir, &pl_graph, &sources, &opts)?;

    let mut hdl_inputs: TaskHdlInputs = TaskHdlInputs::new();
    for (task_name, layout, out) in &hls_results {
        let mut files = out.verilog_files.clone();
        files.extend(super::hls_run::list_hdl_files(&layout.hdl_dir)?);
        files.sort();
        files.dedup();
        hdl_inputs.insert(task_name.clone(), files);

        if let Some(task) = state.graph.tasks.get_mut(task_name) {
            apply_hls_metrics(task_name, task, &out.csynth)?;
        }
    }
    let mut pl_graph = crate::aie::pl_graph(&state.graph)?;
    let projection = state
        .flow
        .flatten_hierarchy
        .then(|| tapa_ir::transforms::flatten_with_origins(&pl_graph))
        .transpose()
        .map_err(|error| CliError::Codegen(format!("cannot flatten the task graph: {error}")))?;
    generate_rtl_tree(
        &ctx.work_dir,
        &pl_graph,
        &hdl_inputs,
        None,
        projection.as_ref(),
    )?;

    // Per-task OOC Vivado synth → hierarchical utilization → `total_area`.
    // When disabled (the default), reports and topology consumers derive
    // effective totals recursively from the HLS-populated `self_area` maps.
    if args.enable_synth_util {
        emit_post_synth_util(
            &ctx.work_dir,
            &mut pl_graph,
            &device.part_num,
            args.jobs,
            runner,
            &sources,
            projection.as_ref().map(|projection| &projection.flat),
        )?;
        for (name, task) in &pl_graph.tasks {
            if let Some(original) = state.graph.tasks.get_mut(name) {
                original.total_area.clone_from(&task.total_area);
            }
        }
    }

    // Emit `report.{json,yaml}` at the work-dir root once area data is
    // final. Both pack paths (`xilinx-vitis` `.xo` and `xilinx-hls`
    // `.zip`) bundle the YAML at archive root.
    write_top_report(
        &ctx.work_dir,
        &pl_graph,
        &args.override_report_schema_version,
        aie_report.as_ref().zip(partition.as_ref()),
    )?;

    write_templates_info(&ctx.work_dir, &pl_graph)?;
    state.flow.synthed = true;
    work_io::store(&ctx.work_dir, &state)?;

    report_timing_summary(clock_period, &pl_graph);

    Ok(())
}

/// Rebuild only the AI Engine graph. PL results remain reusable only while
/// all inputs that produced them are unchanged; this path never creates PL
/// results or promotes an analyzed/failed state to synthesized.
fn run_aie_only(
    args: &SynthArgs,
    ctx: &CliContext,
    runner: &dyn ToolRunner,
    mut state: WorkState,
    partition: &tapa_ir::AiePartition,
    part_num: &str,
    clock_period: ClockPeriod,
) -> Result<()> {
    let pl_inputs_unchanged = state.flow.part_num.as_deref() == Some(part_num)
        && state.flow.platform == args.platform
        && state.flow.clock_period == Some(clock_period);
    if !pl_inputs_unchanged {
        state.flow.synthed = false;
        state.floorplan = None;
    }
    state.flow.part_num = Some(part_num.to_string());
    state.flow.platform.clone_from(&args.platform);
    state.flow.clock_period = Some(clock_period);
    // Persist invalidation before the build, but keep the previous skip
    // policy until a replacement AI Engine graph has actually been built.
    work_io::store(&ctx.work_dir, &state)?;

    let aie_report = build_aie(args, ctx, runner, &state, partition, clock_period)?;
    state.flow.skip_aie = false;
    work_io::store(&ctx.work_dir, &state)?;

    let report_exists =
        ctx.work_dir.join("report.json").exists() || ctx.work_dir.join("report.yaml").exists();
    if state.flow.synthed && report_exists {
        let pl_graph = crate::aie::pl_graph(&state.graph)?;
        write_top_report(
            &ctx.work_dir,
            &pl_graph,
            &args.override_report_schema_version,
            Some((&aie_report, partition)),
        )?;
        log::info!(
            "refreshed the root report with the new AI Engine mapping; rerun \
             `tapa aiesim --cycle-accurate` to measure its timing"
        );
    }
    Ok(())
}

/// Compile the AI Engine partition for hardware into
/// `<work_dir>/aie/hw/libadf.a`, with its PLIOs at the PL clock.
fn build_aie(
    args: &SynthArgs,
    ctx: &CliContext,
    runner: &dyn ToolRunner,
    state: &WorkState,
    partition: &tapa_ir::AiePartition,
    clock_period: ClockPeriod,
) -> Result<tapa_xilinx::tools::aie::report::AieReport> {
    use crate::aie::{device, dir, prepare, GraphBuild};
    let job = prepare(
        &ctx.work_dir,
        &state.graph,
        partition,
        GraphBuild {
            dir: dir(&ctx.work_dir, "hw"),
            target: tapa_xilinx::tools::aie::AieTarget::Hw,
            device: device(args.platform.as_deref(), args.part_num.as_deref())?,
            pl_freq_mhz: Some(1000.0 / clock_period.nanoseconds()),
            sim: None,
        },
    )?;
    log::info!(
        "compiling {} AI Engine kernel(s) into {}",
        partition.kernels.len(),
        job.graph_dir.join("libadf.a")
    );
    tapa_xilinx::tools::aie::compile(runner, &job)?;
    Ok(tapa_xilinx::tools::aie::report::read_report(
        &job.graph_dir,
    )?)
}

/// Say whether HLS met the requested clock period, and if not, which task
/// was worst.
///
/// Frequency is the headline property of a TAPA design, but until now the
/// only place the achieved estimate appeared was `report.json` — a design
/// could miss its target by 2× and `tapa synth` would print nothing at all.
fn report_timing_summary(target: ClockPeriod, graph: &tapa_ir::TaskGraph) {
    let Some((worst_task, worst)) = graph
        .tasks
        .iter()
        .filter_map(|(name, task)| task.clock_period.map(|period| (name, period)))
        .max_by_key(|(_, period)| period.picoseconds())
    else {
        return;
    };
    let target_ns = target.nanoseconds();
    let worst_ns = worst.nanoseconds();
    if worst > target {
        log::warn!(
            "HLS estimates {worst_ns:.3} ns ({:.1} MHz) for the critical task `{worst_task}`, \
             over the {target_ns:.3} ns ({:.1} MHz) target; see report.json for the per-task breakdown",
            1000.0 / worst_ns,
            1000.0 / target_ns,
        );
    } else {
        log::info!(
            "HLS estimates {worst_ns:.3} ns ({:.1} MHz) for the critical task `{worst_task}`, \
             within the {target_ns:.3} ns target",
            1000.0 / worst_ns,
        );
    }
}

/// Apply the raw metrics reported by HLS to a task.
///
/// `clock_period` stores the achieved estimate, not the requested target.
/// `total_area` is cleared because it is either re-populated by optional
/// out-of-context synthesis or derived recursively from `self_area` by report
/// and topology consumers.
fn apply_hls_metrics(task_name: &str, task: &mut Task, report: &CsynthReport) -> Result<()> {
    task.clock_period = Some(
        ClockPeriod::from_nanoseconds_str(&report.estimated_clock_period_ns).map_err(|error| {
            CliError::Report(format!(
                "HLS reported an unusable clock period for `{task_name}`: {error}"
            ))
        })?,
    );
    task.self_area = Some(report.area);
    // Cleared because it is either re-populated by optional out-of-context
    // synthesis or derived recursively from `self_area`.
    task.total_area = None;
    Ok(())
}

/// Build the shared HLS cflags TAIL: vendor/tapa include resolution
/// (`get_tapacc_cflags()`, so `-isystem <tapa-lib>` etc. are forwarded
/// into HLS) followed by the `-DTAPA_TARGET_*` defines and a
/// `-I <tapa-extra-runtime-include>` entry when the resource can be
/// resolved ("WORKAROUND: Vitis HLS requires -I or gflags cannot be
/// found..." branch).
///
/// The user's graph cflags and the per-task manifest flags are
/// prepended by [`task_cflags`](super::hls_run) — which also means the
/// `-I <tapa-extra-runtime-include>` workaround flag must stay the
/// LAST element this returns: Vitis 2025.2's unified (vitis-run) flow
/// glues any `-cflags` token ending in the exact string `include` to
/// the token after it (clang `-include <file>` argument-merging
/// semantics), destroying both. The directory name ends in `include`
/// by construction, so nothing load-bearing may follow it.
fn build_hls_cflags(remote: bool) -> Vec<String> {
    let mut flags: Vec<String> = Vec::new();
    // Remote HLS substitutes `get_tapacc_cflags()` (local
    // vendor/stdlib paths) with `get_remote_hls_cflags()` when
    // `~/.taparc` is active — the
    // remote host ships its own vendor headers via `settings64.sh`,
    // and a host-specific macOS `__assert_rtn` remap is added so
    // macOS → Linux remote synth does not mis-expand assert() into
    // `__assert_rtn(func,file,line,expr)`.
    if remote {
        flags.extend(get_remote_hls_cflags());
    } else {
        flags.extend(get_tapacc_cflags(false));
    }
    flags.push("-DTAPA_TARGET_DEVICE_".to_string());
    flags.push("-DTAPA_TARGET_XILINX_HLS_".to_string());
    // Vitis HLS requires `-I` (not `-isystem`) to locate gflags during build.
    if let Ok(p) = find_resource("tapa-extra-runtime-include") {
        flags.push(format!("-I{}", p.display()));
    }
    flags
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::similar_names,
        reason = "the `args`/`argv` pair appears throughout the dispatcher; \
                  matching the production names keeps tests legible"
    )]

    use super::*;

    use std::path::Path;
    use std::sync::Mutex;

    use clap::Parser;

    use std::collections::BTreeMap;

    use tapa_ir::{
        Area, ArgCategory, FloorplanResult, Port, SynthTarget, Target, TaskGraph, TaskLevel,
    };
    use tapa_xilinx::tools::aie::report::{AieKernelReport, AieReport, AieTile};
    use tapa_xilinx::{MockToolRunner, ToolInvocation, ToolOutput};

    use crate::globals::GlobalArgs;
    use crate::state::work::WorkState;

    fn parse_synth(extra: &[&str]) -> SynthArgs {
        let mut argv = vec!["synth"];
        argv.extend_from_slice(extra);
        SynthArgs::try_parse_from(argv).expect("parse synth args")
    }

    fn ctx_with_work_dir(work_dir: &Path) -> CliContext {
        let globals = GlobalArgs::try_parse_from([
            "tapa",
            "--work-dir",
            work_dir.to_str().expect("utf-8 work dir"),
        ])
        .expect("parse globals");
        CliContext::from_globals(&globals)
    }

    const AIE_PART: &str = "xcvc1902-vsva2197-2MP-e-S";

    fn task(name: &str, level: TaskLevel, synth: SynthTarget) -> Task {
        Task {
            level,
            srcs: vec![format!("{name}.cpp")],
            include_dirs: Vec::new(),
            defines: Vec::new(),
            ports: Vec::new(),
            tasks: BTreeMap::new(),
            fifos: BTreeMap::new(),
            readable_name: name.to_string(),
            aie_location: Vec::new(),
            aie_stack_bytes: None,
            synth,
            self_area: None,
            total_area: None,
            clock_period: None,
        }
    }

    /// Smallest valid hybrid graph: one AIE leaf and a scalar PL control
    /// argument. It needs no FIFO fixture to exercise the AIE-only boundary.
    fn aie_work_state() -> WorkState {
        let mut top = task("Top", TaskLevel::Upper, SynthTarget::Hls);
        top.ports.push(Port {
            cat: ArgCategory::Scalar,
            name: "n".to_string(),
            ctype: "int".to_string(),
            width: 32,
            chan_count: None,
            chan_size: None,
            aie_frame: None,
            aie_plio: None,
            stream_depth: None,
            mmap_addr_width: None,
        });
        top.tasks.insert(
            "Add".to_string(),
            vec![tapa_ir::TaskInstance {
                name: None,
                args: BTreeMap::new(),
                step: 0,
            }],
        );
        WorkState::new(TaskGraph {
            schema_version: tapa_ir::graph::SCHEMA_VERSION,
            top: "Top".to_string(),
            target: Target::XilinxVitis,
            tasks: BTreeMap::from([
                (
                    "Add".to_string(),
                    task("Add", TaskLevel::Lower, SynthTarget::Aie),
                ),
                ("Top".to_string(), top),
            ]),
            cflags: Vec::new(),
        })
    }

    fn prepare_aie_work(work: &Path, state: &WorkState) {
        work_io::store(work, state).expect("store state");
        let rewritten = work.join(crate::tapacc::REWRITTEN_DIR);
        fs_err::create_dir_all(&rewritten).expect("mkdir rewritten");
        fs_err::write(rewritten.join("Add.cpp"), "void Add() {}\n").expect("write AIE source");
    }

    fn aie_only_args(part: &str, clock: &str) -> SynthArgs {
        parse_synth(&["--aie-only", "--part-num", part, "--clock-period", clock])
    }

    fn mock_aie_compile(work: &Path, program_bytes: u64) -> MockToolRunner {
        let runner = MockToolRunner::new();
        runner.push_ok(
            "aiecompiler",
            ToolOutput {
                exit_code: 0,
                stdout: String::new(),
                stderr: String::new(),
            },
        );
        let reports = work.join("aie/hw/Work/reports");
        runner.attach_download(
            crate::util::utf8(reports.join("compiler_report.json")),
            br#"{
              "blockTypes": {
                "kernel": {"type": "function", "function": "tapa_aie_Add_0"}
              },
              "blockInstances": {
                "k": {"blockType": "kernel", "fabric": "me"}
              },
              "mapping": {"blockInstanceMapping": {
                "k": {"coreInfo": {"column": 1, "row": 0}}
              }}
            }"#
            .to_vec(),
        );
        runner.attach_download(
            crate::util::utf8(reports.join("report_pm.txt")),
            format!("Note: All sizes are in bytes\nCore 1_0\nPM Size Used = {program_bytes}\n")
                .into_bytes(),
        );
        runner
    }

    fn floorplan_fixture() -> FloorplanResult {
        FloorplanResult {
            device: "vck190".to_string(),
            grid: (1, 1),
            regions: BTreeMap::new(),
            routes: Vec::new(),
            slot_usage: BTreeMap::new(),
        }
    }

    fn synthesized_aie_state() -> WorkState {
        let mut state = aie_work_state();
        state.flow.part_num = Some(AIE_PART.to_string());
        state.flow.clock_period = Some(ClockPeriod::from_picoseconds(3330));
        state.flow.synthed = true;
        state.floorplan = Some(floorplan_fixture());
        let top = state.graph.tasks.get_mut("Top").expect("top");
        top.clock_period = Some(ClockPeriod::from_picoseconds(2000));
        top.self_area = Some(Area {
            lut: 7,
            ..Area::default()
        });
        state
    }

    fn write_current_report(work: &Path, state: &WorkState, program_bytes: u64) {
        let partition = tapa_ir::AiePartition::derive(&state.graph)
            .expect("derive partition")
            .expect("AIE partition");
        let pl_graph = crate::aie::pl_graph(&state.graph).expect("PL graph");
        let aie = AieReport {
            tiles_used: 1,
            plio_map: BTreeMap::new(),
            per_kernel: BTreeMap::from([(
                "tapa_aie_Add_0".to_string(),
                AieKernelReport {
                    tile: AieTile { column: 1, row: 0 },
                    program_bytes,
                    data_bytes: None,
                },
            )]),
        };
        write_top_report(work, &pl_graph, "", Some((&aie, &partition))).expect("write report");
    }

    #[test]
    fn aie_only_conflicts_with_skip_aie() {
        let error = SynthArgs::try_parse_from(["synth", "--aie-only", "--skip-aie"])
            .expect_err("conflicting one-sided builds");
        assert!(error.to_string().contains("--skip-aie"), "{error}");
    }

    #[test]
    fn aie_only_rejects_a_design_without_an_aie_partition() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut state = aie_work_state();
        state.graph.tasks.get_mut("Add").expect("Add").synth = SynthTarget::Hls;
        work_io::store(dir.path(), &state).expect("store state");
        let runner = MockToolRunner::new();

        let error = run_native(
            &aie_only_args(AIE_PART, "3.33"),
            &ctx_with_work_dir(dir.path()),
            &runner,
        )
        .expect_err("no AIE partition");

        assert!(error.to_string().contains("no AI Engine task"), "{error}");
        assert!(runner.calls().is_empty(), "no tool may run");
    }

    #[test]
    fn aie_only_runs_no_pl_stage_or_fabricates_a_report() {
        let dir = tempfile::tempdir().expect("tempdir");
        let work = dir.path();
        let state = aie_work_state();
        prepare_aie_work(work, &state);
        let runner = mock_aie_compile(work, 4096);
        let mut args = aie_only_args(AIE_PART, "3.33");
        args.enable_synth_util = true;

        run_native(&args, &ctx_with_work_dir(work), &runner).expect("AIE-only synth");

        assert_eq!(
            runner
                .calls()
                .iter()
                .map(|call| call.program.as_str())
                .collect::<Vec<_>>(),
            ["aiecompiler"]
        );
        for artifact in [
            "hls",
            "rtl",
            "templates_info.json",
            "report.json",
            "report.yaml",
        ] {
            assert!(!work.join(artifact).exists(), "unexpected {artifact}");
        }
        let persisted = work_io::load(work).expect("load state");
        assert!(!persisted.flow.synthed, "analyze state is never promoted");
        assert!(!persisted.flow.skip_aie);
        assert_eq!(persisted.flow.part_num.as_deref(), Some(AIE_PART));
        assert_eq!(persisted.graph, state.graph, "PL metrics are untouched");
    }

    #[test]
    fn aie_only_reuses_matching_pl_results_and_refreshes_current_reports() {
        let dir = tempfile::tempdir().expect("tempdir");
        let work = dir.path();
        let mut state = synthesized_aie_state();
        state.flow.skip_aie = true;
        prepare_aie_work(work, &state);
        write_current_report(work, &state, 1);
        super::super::report::fold_aiesim_timing(
            work,
            &super::super::report::AiesimSection::default(),
        )
        .expect("add stale aiesim timing");
        let runner = mock_aie_compile(work, 4096);

        run_native(
            &aie_only_args(AIE_PART, "3.33"),
            &ctx_with_work_dir(work),
            &runner,
        )
        .expect("AIE-only synth");

        let persisted = work_io::load(work).expect("load state");
        assert!(persisted.flow.synthed);
        assert!(!persisted.flow.skip_aie, "pack expects the new AIE sidecar");
        assert_eq!(persisted.floorplan, state.floorplan);
        assert_eq!(
            persisted.graph, state.graph,
            "typed PL metrics are preserved"
        );
        let report: serde_json::Value = serde_json::from_str(
            &fs_err::read_to_string(work.join("report.json")).expect("read report"),
        )
        .expect("parse report");
        assert_eq!(
            report["aie"]["per_kernel"]["tapa_aie_Add_0"]["program_bytes"],
            4096
        );
        assert!(
            report["aie"].get("aiesim").is_none(),
            "old timing is invalidated"
        );
        assert_eq!(report["performance"]["clock_period"], "2");
        assert_eq!(report["area"]["total"]["LUT"], 7);
    }

    #[test]
    fn aie_only_changed_part_or_clock_invalidates_pl_results() {
        for (part, clock) in [("xcvc1902-changed", "3.33"), (AIE_PART, "4.0")] {
            let dir = tempfile::tempdir().expect("tempdir");
            let work = dir.path();
            let state = synthesized_aie_state();
            prepare_aie_work(work, &state);
            write_current_report(work, &state, 1);
            let runner = mock_aie_compile(work, 4096);

            run_native(
                &aie_only_args(part, clock),
                &ctx_with_work_dir(work),
                &runner,
            )
            .expect("AIE-only synth");

            let persisted = work_io::load(work).expect("load state");
            assert!(!persisted.flow.synthed, "changed inputs: {part} at {clock}");
            assert!(persisted.floorplan.is_none());
            assert_eq!(
                persisted.graph, state.graph,
                "PL metrics remain typed state"
            );
            let report: serde_json::Value = serde_json::from_str(
                &fs_err::read_to_string(work.join("report.json")).expect("read stale report"),
            )
            .expect("parse stale report");
            assert_eq!(
                report["aie"]["per_kernel"]["tapa_aie_Add_0"]["program_bytes"], 1,
                "invalidated reports are not refreshed as current"
            );
        }
    }

    #[test]
    fn failed_aie_only_build_keeps_the_previous_skip_policy() {
        for skip_aie in [false, true] {
            let dir = tempfile::tempdir().expect("tempdir");
            let work = dir.path();
            let mut state = synthesized_aie_state();
            state.flow.skip_aie = skip_aie;
            prepare_aie_work(work, &state);
            let runner = MockToolRunner::new();
            runner.push_failure("aiecompiler", 1, "synthetic failure");

            run_native(
                &aie_only_args(AIE_PART, "3.33"),
                &ctx_with_work_dir(work),
                &runner,
            )
            .expect_err("compiler failure");

            let persisted = work_io::load(work).expect("load state");
            assert_eq!(persisted.flow.skip_aie, skip_aie);
            assert!(persisted.flow.synthed, "unchanged PL results stay valid");
        }
    }

    /// `ToolRunner` stub that pre-stages a well-formed
    /// `project/<top>/syn/{report,verilog}` tree under `cwd` so
    /// `tapa_xilinx::run_hls`'s harvester succeeds.
    struct StubHls {
        responses: Mutex<Vec<(String, String)>>,
    }

    impl StubHls {
        fn new(responses: Vec<(String, String)>) -> Self {
            Self {
                responses: Mutex::new(responses),
            }
        }
    }

    impl ToolRunner for StubHls {
        fn run(&self, inv: &ToolInvocation) -> tapa_xilinx::Result<ToolOutput> {
            let cwd = inv.cwd.clone().expect("HLS sets cwd");
            // Route by TAPA_KERNEL_PATH_0's cpp basename so parallel
            // HLS dispatch (new default) stages each task under the
            // correct `project/<task>/syn/` tree. Queue is still
            // consulted for the verilog body content, keyed on top.
            let inferred_top = inv.env.get("TAPA_KERNEL_PATH_0").and_then(|p| {
                std::path::Path::new(&p.to_string())
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .map(str::to_string)
            });
            let mut q = self.responses.lock().expect("poisoned");
            let (top, body) = if let Some(name) = inferred_top {
                let idx = q.iter().position(|(t, _)| t == &name).unwrap_or_else(|| {
                    let queued_tops = q.iter().map(|(top, _)| top.as_str()).collect::<Vec<_>>();
                    panic!(
                        "StubHls: no response queued for inferred top `{name}`; queued \
                         tops: {queued_tops:?}"
                    );
                });
                let (_, body) = q.remove(idx);
                (name, body)
            } else {
                let (top, body) = q.first().cloned().expect("StubHls: no response queued");
                q.remove(0);
                (top, body)
            };
            let syn = cwd.join("project").join(&top).join("syn");
            fs_err::create_dir_all(syn.join("report")).expect("mkdir report");
            fs_err::create_dir_all(syn.join("verilog")).expect("mkdir verilog");
            fs_err::write(
                syn.join("report").join(format!("{top}_csynth.xml")),
                br#"<?xml version="1.0"?>
<profile>
  <UserAssignments>
    <TopModelName>X</TopModelName>
    <Part>xcvu37p</Part>
    <TargetClockPeriod>3.33</TargetClockPeriod>
  </UserAssignments>
  <PerformanceEstimates>
    <SummaryOfTimingAnalysis>
      <EstimatedClockPeriod>1.0</EstimatedClockPeriod>
    </SummaryOfTimingAnalysis>
  </PerformanceEstimates>
</profile>"#,
            )
            .expect("csynth.xml");
            fs_err::write(syn.join("verilog").join(format!("{top}.v")), body).expect("write v");
            Ok(ToolOutput {
                exit_code: 0,
                stdout: String::new(),
                stderr: String::new(),
            })
        }
    }

    /// Parallel HLS dispatch routes each invocation by the cpp basename in
    /// `TAPA_KERNEL_PATH_0`; an inferred top with no queued response is a
    /// stub-authoring bug and must panic loudly, naming both the inferred
    /// top and the queued tops.
    #[test]
    #[should_panic(
        expected = "StubHls: no response queued for inferred top `Mul`; queued tops: [\"Add\"]"
    )]
    fn stub_hls_panics_when_inferred_top_has_no_queued_response() {
        let dir = tempfile::tempdir().expect("tempdir");
        let runner = StubHls::new(vec![("Add".into(), "module Add(); endmodule\n".into())]);
        let mut inv = ToolInvocation::new("vitis_hls");
        inv.cwd = Some(
            camino::Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).expect("utf-8 work dir"),
        );
        inv.env
            .insert("TAPA_KERNEL_PATH_0".into(), "/kernels/Mul.cpp".into());
        drop(runner.run(&inv));
    }

    #[test]
    fn hls_metrics_use_estimated_clock_and_clear_stale_total() {
        let mut task = Task {
            level: TaskLevel::Lower,
            srcs: vec!["Add.cpp".to_string()],
            include_dirs: Vec::new(),
            defines: Vec::new(),
            ports: Vec::new(),
            tasks: BTreeMap::new(),
            fifos: BTreeMap::new(),
            readable_name: String::new(),
            aie_location: Vec::new(),
            aie_stack_bytes: None,
            synth: SynthTarget::Hls,
            self_area: Some(Area {
                lut: 999,
                ..Area::default()
            }),
            total_area: Some(Area {
                lut: 999,
                ..Area::default()
            }),
            clock_period: Some(ClockPeriod::from_picoseconds(9990)),
        };
        let report = CsynthReport {
            top: "Add".to_string(),
            part: "xcvu37p".to_string(),
            target_clock_period_ns: "3.33".to_string(),
            estimated_clock_period_ns: "1.25".to_string(),
            area: Area {
                lut: 42,
                ff: 21,
                ..Area::default()
            },
        };

        apply_hls_metrics("Add", &mut task, &report).expect("apply metrics");

        assert_eq!(task.clock_period, Some(ClockPeriod::from_picoseconds(1250)));
        let area = task.self_area.expect("self area");
        assert_eq!(area.lut, 42);
        assert_eq!(area.ff, 21);
        assert!(
            task.total_area.is_none(),
            "stale post-synthesis totals must not survive a new HLS run"
        );
    }

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "integration test with many assertions"
    )]
    fn synth_writes_full_pipeline_artifacts() {
        let dir = tempfile::tempdir().expect("tempdir");
        let work = dir.path();
        let mut tasks = BTreeMap::new();
        tasks.insert(
            "Add".to_string(),
            Task {
                level: TaskLevel::Lower,
                srcs: vec!["Add.cpp".to_string()],
                include_dirs: Vec::new(),
                defines: Vec::new(),
                ports: Vec::new(),
                tasks: BTreeMap::new(),
                fifos: BTreeMap::new(),
                readable_name: String::new(),
                aie_location: Vec::new(),
                aie_stack_bytes: None,
                synth: SynthTarget::Hls,
                self_area: None,
                total_area: None,
                clock_period: None,
            },
        );
        let mut child_tasks = BTreeMap::new();
        child_tasks.insert(
            "Add".to_string(),
            vec![tapa_ir::TaskInstance {
                name: None,
                args: BTreeMap::new(),
                step: 0,
            }],
        );
        tasks.insert(
            "VecAdd".to_string(),
            Task {
                level: TaskLevel::Upper,
                srcs: vec!["VecAdd.cpp".to_string()],
                include_dirs: Vec::new(),
                defines: Vec::new(),
                ports: Vec::new(),
                tasks: child_tasks,
                fifos: BTreeMap::new(),
                readable_name: String::new(),
                aie_location: Vec::new(),
                aie_stack_bytes: None,
                synth: SynthTarget::Hls,
                self_area: None,
                total_area: None,
                clock_period: Some(ClockPeriod::from_picoseconds(3330)),
            },
        );
        let mut state = WorkState::new(TaskGraph {
            schema_version: tapa_ir::graph::SCHEMA_VERSION,
            top: "VecAdd".to_string(),
            target: Target::XilinxHls,
            tasks,
            cflags: Vec::new(),
        });
        state.flow.synthed = true;
        state.floorplan = Some(FloorplanResult {
            device: "u280".to_string(),
            grid: (2, 3),
            regions: BTreeMap::from([("Add_0".to_string(), "SLOT_X0Y0_TO_SLOT_X0Y0".to_string())]),
            routes: Vec::new(),
            slot_usage: BTreeMap::from([("SLOT_X0Y0_TO_SLOT_X0Y0".to_string(), Area::default())]),
        });
        work_io::store(work, &state).expect("store state");

        // The rewritten sources `tapa analyze` would have written; the
        // manifest in the stored state points at them.
        let rewritten = work.join(crate::tapacc::REWRITTEN_DIR);
        fs_err::create_dir_all(&rewritten).expect("mkdir rewritten");
        fs_err::write(rewritten.join("Add.cpp"), "void Add() {}\n").expect("write Add.cpp");
        fs_err::write(rewritten.join("VecAdd.cpp"), "void VecAdd() {}\n")
            .expect("write VecAdd.cpp");
        fs_err::write(
            rewritten.join(".tapa-tree-manifest"),
            "tapa-tree-ownership-v1\nAdd.cpp\nVecAdd.cpp\n",
        )
        .expect("write rewritten-tree ownership manifest");

        // Two HLS invocations: the leaf `Add` and the upper-task shell
        // `VecAdd`. Iteration order is `BTreeMap` alphabetical order,
        // which matches the sorted order `tapa analyze` writes.
        let stub_module = |name: &str| -> String {
            format!(
                "module {name}(\n  input wire ap_clk,\n  input wire ap_rst_n,\n  \
                 input wire ap_start,\n  output wire ap_done,\n  output wire ap_idle,\n  \
                 output wire ap_ready\n);\nendmodule\n"
            )
        };
        let runner = StubHls::new(vec![
            ("Add".into(), stub_module("Add")),
            ("VecAdd".into(), stub_module("VecAdd")),
        ]);
        let ctx = ctx_with_work_dir(work);
        let args = parse_synth(&[
            "--part-num",
            "xcvu37p-fsvh2892-2L-e",
            "--clock-period",
            "3.33",
        ]);
        run_native(&args, &ctx, &runner).expect("native synth must succeed end-to-end");

        assert!(work.join("tapa.json").is_file(), "tapa.json must persist");
        assert!(
            work.join("templates_info.json").is_file(),
            "templates_info.json must persist"
        );
        assert!(
            work.join("hls/Add/verilog").is_dir(),
            "hls/Add/verilog must exist"
        );
        assert!(work.join("rtl").is_dir(), "rtl directory must exist");
        assert!(
            work.join("rtl/VecAdd.v").is_file(),
            "rtl/VecAdd.v must be emitted"
        );
        assert!(
            work.join("rtl/VecAdd_fsm.v").is_file(),
            "rtl/VecAdd_fsm.v must be emitted (upper task FSM)",
        );

        let templates = std::fs::read_to_string(work.join("templates_info.json")).expect("read");
        assert_eq!(templates, "{}");

        // The one state file carries both the resolved flow settings and the
        // graph that synth annotated in place.
        let persisted = work_io::load(work).expect("load persisted state");
        assert!(persisted.flow.synthed, "synthed flag must persist");
        assert!(
            persisted.floorplan.is_none(),
            "synth must invalidate a floorplan derived from the previous RTL",
        );
        assert_eq!(
            persisted.flow.part_num.as_deref(),
            Some("xcvu37p-fsvh2892-2L-e"),
        );
        assert_eq!(
            persisted.flow.clock_period,
            Some(ClockPeriod::from_picoseconds(3330))
        );
        assert_eq!(
            persisted.flow.platform, None,
            "no --platform was passed, so none must be recorded",
        );
        assert_eq!(
            persisted.graph.target,
            Target::XilinxHls,
            "the flow target must survive synth's rewrite",
        );
        let one_ns = Some(ClockPeriod::from_picoseconds(1000));
        assert_eq!(persisted.graph.tasks["Add"].clock_period, one_ns);
        assert_eq!(persisted.graph.tasks["VecAdd"].clock_period, one_ns);
        let report: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(work.join("report.json")).expect("read report"),
        )
        .expect("parse report");
        assert_eq!(report["performance"]["clock_period"], "1");
    }
}
