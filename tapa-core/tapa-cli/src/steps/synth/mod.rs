//! `tapa synth` orchestration.
//!
//! For the vadd-style happy path (`--platform <p>`, leaf children +
//! one upper top), this module drives the full Vitis HLS + RTL codegen
//! pipeline natively:
//!
//!   1. Resolve the device (part / clock / platform) via
//!      `tapa_xilinx::parse_device_info` and persist into
//!      `<work_dir>/tapa.json`.
//!   2. Extract per-task C++ from the task graph to `<work_dir>/cpp/`.
//!   3. Run Vitis HLS for each leaf task, harvesting Verilog into
//!      `<work_dir>/hls/<task>/verilog/`.
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

mod cpp_extract;
mod device_resolve;
mod hls_run;
mod metrics;
mod post_synth_util;
mod report;
pub(crate) mod rtl_codegen;
mod runner;

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

    /// Reuse a task's existing Verilog when it is newer than its
    /// extracted C++, skipping that task's HLS run.
    #[arg(long = "skip-hls-based-on-mtime", default_value_t = false)]
    pub skip_hls_based_on_mtime: bool,

    /// Extra Tcl appended verbatim to every generated HLS script.
    #[arg(long = "other-hls-configs", default_value = "")]
    pub other_hls_configs: String,

    /// Run out-of-context Vivado synthesis per task for accurate area
    /// numbers, instead of relying on the coarser HLS estimates.
    #[arg(long = "enable-synth-util", default_value_t = false)]
    pub enable_synth_util: bool,

    /// Stamp `report.json` / `report.yaml` with this schema version
    /// instead of the built-in one. For report-consumer testing.
    #[arg(long = "override-report-schema-version", default_value = "")]
    pub override_report_schema_version: String,
}

/// Top-level dispatcher.
///
/// The HLS + codegen pipeline is the only path. When
/// `ctx.remote_config` is populated (via `~/.taparc` or
/// `--remote-host`), HLS dispatches through `RemoteToolRunner`;
/// otherwise `LocalToolRunner`.
pub fn run(args: &SynthArgs, ctx: &CliContext) -> Result<()> {
    ctx.with_tool_runner(|runner| run_native(args, ctx, runner))
}
