//! `tapa synth` orchestration.
//!
//! For the vadd-style happy path (`--platform <p>`, leaf children +
//! one upper top), this module drives the full Vitis HLS + RTL codegen
//! pipeline natively:
//!
//!   1. Resolve the device (part / clock / platform) via
//!      `tapa_xilinx::parse_device_info` and persist into
//!      `<work_dir>/tapa.json`.
//!   2. Stage the per-task source manifests against
//!      `<work_dir>/rewritten/` (written by `tapa analyze`).
//!   3. Run Vitis HLS for every task that targets HLS — all tasks,
//!      not just leaves, since codegen needs the upper-task shells —
//!      harvesting Verilog into `<work_dir>/hls/<task>/verilog/`.
//!   4. Drive `tapa_codegen::generate_rtl` to instrument upper tasks
//!      and emit `<work_dir>/rtl/{<task>.v, <task>_fsm.v, ...}`.
//!   5. Persist `<work_dir>/templates_info.json` and re-store the
//!      state with the annotated graph (`synthed=true`).
//!
//! `--enable-synth-util` additionally drives a per-task Vivado
//! out-of-context synthesis pass through
//! `post_synth_util::emit_post_synth_util`.

use clap::Parser;

use crate::context::CliContext;
use crate::error::Result;

mod device_resolve;
mod hls_run;
mod hls_sources;
mod metrics;
mod post_synth_util;
pub(crate) mod report;
pub(crate) mod rtl_codegen;
mod runner;

pub(crate) use device_resolve::resolve_platform_dir;
pub(crate) use runner::run_native;

/// Resolve the parallel worker count for a synth sub-pass.
///
/// A positive `--jobs N` wins; zero/absent falls back to the host's
/// available parallelism. The result is capped by `work_count` so we
/// never spawn more workers than there are units of work.
fn resolve_worker_count(jobs: Option<u32>, work_count: usize) -> usize {
    let desired = match jobs {
        None | Some(0) => {
            std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get)
        }
        Some(jobs) => jobs as usize,
    };
    desired.min(work_count.max(1))
}

#[derive(Debug, Clone, Parser)]
#[command(name = "synth", about = "Synthesize the TAPA program into RTL code.")]
#[allow(
    clippy::struct_excessive_bools,
    reason = "each bool is an independent command-line switch, not a state"
)]
pub struct SynthArgs {
    /// Target FPGA part number. Required unless `--platform` is given.
    #[arg(long = "part-num", value_name = "PART")]
    pub part_num: Option<String>,

    /// Vitis platform name. Required unless `--part-num` is given; also
    /// supplies a default clock period.
    #[arg(short = 'p', long = "platform", value_name = "PLATFORM")]
    pub platform: Option<String>,

    /// Target clock period in nanoseconds. Defaults to the platform's.
    #[arg(
        long = "clock-period",
        value_name = "NS",
        value_parser = crate::util::parse_clock_period
    )]
    pub clock_period: Option<tapa_ir::ClockPeriod>,

    /// Parallel HLS and post-synthesis jobs. Defaults to the host's
    /// available parallelism.
    #[arg(short = 'j', long = "jobs", value_name = "N")]
    pub jobs: Option<u32>,

    /// Keep each task's Vitis HLS project under `hls/<task>/project`
    /// for post-mortem inspection instead of discarding it.
    #[arg(long = "keep-hls-work-dir", default_value_t = false)]
    pub keep_hls_work_dir: bool,

    /// Reuse a task's existing Verilog when it is newer than every
    /// TAPA-owned file in the rewritten source tree and its settings match.
    #[arg(long = "skip-hls-based-on-mtime", default_value_t = false)]
    pub skip_hls_based_on_mtime: bool,

    /// Require every task's cached HLS output and csynth report to be valid;
    /// fail instead of running HLS.
    #[arg(long = "require-hls-cache", requires = "skip_hls_based_on_mtime")]
    pub require_hls_cache: bool,

    /// Use hand-authored test fixtures staged under hls/<task> instead of HLS.
    /// Reports must match the requested part and clock; no cache is recorded.
    #[arg(long, conflicts_with = "skip_hls_based_on_mtime")]
    pub hls_fixtures: bool,

    /// Extra Tcl appended verbatim to every generated HLS script.
    #[arg(long = "other-hls-configs", default_value = "")]
    pub other_hls_configs: String,

    /// Run out-of-context Vivado synthesis per task for accurate area
    /// numbers, instead of relying on the coarser HLS estimates.
    #[arg(long = "enable-synth-util", default_value_t = false)]
    pub enable_synth_util: bool,

    /// Build only the PL half of a design with AI Engine tasks, without
    /// compiling the AI Engine graph: enough for hybrid cosimulation, which
    /// runs the AI Engine tasks in software.
    #[arg(long = "skip-aie", default_value_t = false)]
    pub skip_aie: bool,

    /// Build only the AI Engine graph, reusing PL synthesis results only
    /// when their device, platform, and requested clock are unchanged.
    #[arg(
        long = "aie-only",
        default_value_t = false,
        conflicts_with = "skip_aie"
    )]
    pub aie_only: bool,

    /// Stamp `report.json` / `report.yaml` with this schema version
    /// instead of the built-in one. For report-consumer testing.
    #[arg(long = "override-report-schema-version", default_value = "")]
    pub override_report_schema_version: String,
}

/// Top-level dispatcher.
///
/// AIE-only returns before the HLS + codegen pipeline. When
/// `ctx.remote_config` is populated (via `~/.taparc` or
/// `--remote-host`), HLS dispatches through `RemoteToolRunner`;
/// otherwise `LocalToolRunner`.
pub fn run(args: &SynthArgs, ctx: &CliContext) -> Result<()> {
    ctx.with_tool_runner(|runner| run_native(args, ctx, runner))
}
