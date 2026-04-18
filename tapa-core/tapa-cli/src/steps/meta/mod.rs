//! Composite click commands.
//!
//! `tapa compile`, `tapa generate-floorplan`, and
//! `tapa compile-with-floorplan-dse` mirror the click commands of the
//! same names in `tapa/steps/meta.py`. Each composite materializes the
//! union of its constituent click commands' flag surfaces — exactly
//! what `_extend_params` does in Python — by flattening the underlying
//! `Args` structs (or, where flag conflicts would arise, by
//! hand-rolling the merged flag set).

mod dse;

use std::path::PathBuf;

use clap::Parser;

pub use self::dse::{run_compile_with_floorplan_dse_composite, CompileWithFloorplanDseArgs};

use crate::context::CliContext;
use crate::error::Result;
use crate::steps::{analyze, floorplan, pack, synth};

// ---------------------------------------------------------------------
// `compile` = analyze + synth + pack
// ---------------------------------------------------------------------
//
// No flag conflicts among analyze / synth / pack so we flatten directly.

#[derive(Debug, Clone, Parser)]
#[command(
    name = "compile",
    about = "Compile a TAPA program to a hardware design (analyze + synth + pack)."
)]
pub struct CompileArgs {
    #[command(flatten)]
    pub analyze: analyze::AnalyzeArgs,
    #[command(flatten)]
    pub synth: synth::SynthArgs,
    #[command(flatten)]
    pub pack: pack::PackArgs,
}

pub fn run_compile_composite(args: &CompileArgs, ctx: &mut CliContext) -> Result<()> {
    // Python bridge is gone as of AC-8 (`tapa/__main__.py` deleted); the
    // composite is always native.
    analyze::run(&args.analyze, ctx)?;
    synth::run(&args.synth, ctx)?;
    pack::run(&args.pack, ctx)
}

// ---------------------------------------------------------------------
// `generate-floorplan` = analyze + synth + run_autobridge
// ---------------------------------------------------------------------
//
// `synth` and `run_autobridge` both expose `--device-config` and
// `--floorplan-config`, so we hand-roll the merged flag set and
// project it back into the per-step `Args` structures at run time.

#[allow(
    clippy::struct_excessive_bools,
    reason = "merged click flag surface — collapsing into an enum would break parity"
)]
#[derive(Debug, Clone, Parser)]
#[command(
    name = "generate-floorplan",
    about = "Generate floorplan solution(s) for a TAPA program via AutoBridge."
)]
pub struct GenerateFloorplanArgs {
    // ---- analyze ----
    #[arg(short = 'f', long = "input", value_name = "FILE", required = true)]
    pub input_files: Vec<PathBuf>,
    #[arg(short = 't', long = "top", value_name = "TASK", required = true)]
    pub top: String,
    #[arg(short = 'c', long = "cflags", value_name = "FLAG")]
    pub cflags: Vec<String>,
    #[arg(long = "flatten-hierarchy", default_value_t = false)]
    pub flatten_hierarchy: bool,
    #[arg(long = "keep-hierarchy", conflicts_with = "flatten_hierarchy")]
    pub keep_hierarchy: bool,
    #[arg(long = "target", value_enum, default_value_t = analyze::AnalyzeTarget::XilinxVitis)]
    pub target: analyze::AnalyzeTarget,
    // ---- synth ----
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
    #[arg(long = "no-skip-hls-based-on-mtime", conflicts_with = "skip_hls_based_on_mtime")]
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
    #[arg(long = "gen-ab-graph", default_value_t = false)]
    pub gen_ab_graph: bool,
    #[arg(long = "no-gen-ab-graph", conflicts_with = "gen_ab_graph")]
    pub no_gen_ab_graph: bool,
    #[arg(long = "gen-graphir", default_value_t = false)]
    pub gen_graphir: bool,
    #[arg(long = "floorplan-path", value_name = "FILE")]
    pub floorplan_path: Option<PathBuf>,
    // ---- shared between synth + run_autobridge ----
    #[arg(long = "device-config", value_name = "FILE", required = true)]
    pub device_config: PathBuf,
    #[arg(long = "floorplan-config", value_name = "FILE", required = true)]
    pub floorplan_config: PathBuf,
}

impl GenerateFloorplanArgs {
    pub fn analyze_args(&self) -> analyze::AnalyzeArgs {
        analyze::AnalyzeArgs {
            input_files: self.input_files.clone(),
            top: self.top.clone(),
            cflags: self.cflags.clone(),
            flatten_hierarchy: true,
            keep_hierarchy: false,
            target: self.target,
            tapacc: None,
            tapa_cpp: None,
        }
    }

    pub fn synth_args(&self) -> synth::SynthArgs {
        synth::SynthArgs {
            part_num: self.part_num.clone(),
            platform: self.platform.clone(),
            clock_period: self.clock_period,
            jobs: self.jobs,
            keep_hls_work_dir: self.keep_hls_work_dir,
            remove_hls_work_dir: self.remove_hls_work_dir,
            skip_hls_based_on_mtime: self.skip_hls_based_on_mtime,
            no_skip_hls_based_on_mtime: self.no_skip_hls_based_on_mtime,
            other_hls_configs: self.other_hls_configs.clone(),
            enable_synth_util: true,
            disable_synth_util: false,
            override_report_schema_version: self.override_report_schema_version.clone(),
            nonpipeline_fifos: self.nonpipeline_fifos.clone(),
            gen_ab_graph: true,
            no_gen_ab_graph: false,
            gen_graphir: self.gen_graphir,
            floorplan_config: Some(self.floorplan_config.clone()),
            device_config: Some(self.device_config.clone()),
            floorplan_path: self.floorplan_path.clone(),
        }
    }

    pub fn run_autobridge_args(&self) -> floorplan::RunAutobridgeArgs {
        floorplan::RunAutobridgeArgs {
            device_config: self.device_config.clone(),
            floorplan_config: self.floorplan_config.clone(),
        }
    }
}

pub fn run_generate_floorplan_composite(
    args: &GenerateFloorplanArgs,
    ctx: &mut CliContext,
) -> Result<()> {
    // Python bridge is gone as of AC-8; always native.
    analyze::run(&args.analyze_args(), ctx)?;
    synth::run(&args.synth_args(), ctx)?;
    floorplan::run_run_autobridge(&args.run_autobridge_args(), ctx)
}


#[cfg(test)]
mod tests;
