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
//!   3. Run Vitis HLS for each leaf task via `tapa_xilinx::run_hls`,
//!      harvesting Verilog into `<work_dir>/hls/<task>/verilog/`.
//!   4. Drive `tapa_codegen::generate_rtl` to instrument upper tasks
//!      and emit `<work_dir>/rtl/{<task>.v, <task>_fsm.v, ...}`.
//!   5. Persist `<work_dir>/templates_info.json` and re-store the
//!      state with the annotated graph (`synthed=true`).
//!
//! `--enable-synth-util` additionally drives a per-task Vivado
//! out-of-context synthesis pass through
//! `post_synth_util::emit_post_synth_util`.

use std::path::PathBuf;

use clap::Parser;
use tapa_xilinx::{LocalToolRunner, RemoteToolRunner, SshMuxOptions, SshSession};

use crate::context::CliContext;
use crate::error::Result;

mod cpp_extract;
mod device_resolve;
mod grouping_constraints;
mod hls_run;
mod metrics;
mod post_synth_util;
mod report;
mod rtl_codegen;
mod runner;

pub(crate) use runner::run_native;

#[allow(
    clippy::struct_excessive_bools,
    reason = "every bool is a distinct user-facing flag, so collapsing into an enum would \
              break compatibility"
)]
#[derive(Debug, Clone, Parser)]
#[command(name = "synth", about = "Synthesize the TAPA program into RTL code.")]
pub struct SynthArgs {
    #[arg(long = "part-num", value_name = "PART")]
    pub part_num: Option<String>,

    #[arg(short = 'p', long = "platform", value_name = "PLATFORM")]
    pub platform: Option<String>,

    #[arg(long = "clock-period", value_name = "NS")]
    pub clock_period: Option<f64>,

    #[arg(short = 'j', long = "jobs", value_name = "N")]
    pub jobs: Option<u32>,

    #[arg(long = "keep-hls-work-dir", default_value_t = false)]
    pub keep_hls_work_dir: bool,

    #[arg(long = "remove-hls-work-dir", conflicts_with = "keep_hls_work_dir")]
    pub remove_hls_work_dir: bool,

    #[arg(long = "skip-hls-based-on-mtime", default_value_t = false)]
    pub skip_hls_based_on_mtime: bool,

    #[arg(
        long = "no-skip-hls-based-on-mtime",
        conflicts_with = "skip_hls_based_on_mtime"
    )]
    pub no_skip_hls_based_on_mtime: bool,

    #[arg(long = "other-hls-configs", default_value = "")]
    pub other_hls_configs: String,

    #[arg(long = "enable-synth-util", default_value_t = false)]
    pub enable_synth_util: bool,

    #[arg(long = "disable-synth-util", conflicts_with = "enable_synth_util")]
    pub disable_synth_util: bool,

    #[arg(long = "override-report-schema-version", default_value = "")]
    pub override_report_schema_version: String,

    #[arg(long = "nonpipeline-fifos", value_name = "FILE")]
    pub nonpipeline_fifos: Option<PathBuf>,
}

/// Top-level dispatcher.
///
/// The HLS + codegen pipeline is the only path. When
/// `ctx.remote_config` is populated (via `~/.taparc` or
/// `--remote-host`), HLS dispatches through `RemoteToolRunner`;
/// otherwise `LocalToolRunner`.
pub fn run(args: &SynthArgs, ctx: &CliContext) -> Result<()> {
    if let Some(cfg) = ctx.remote_config.as_ref() {
        let session = std::sync::Arc::new(SshSession::new(cfg.clone(), SshMuxOptions::default()));
        let runner = RemoteToolRunner::new(session);
        run_native(args, ctx, &runner)
    } else {
        let runner = LocalToolRunner::new();
        run_native(args, ctx, &runner)
    }
}
