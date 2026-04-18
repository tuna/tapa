//! `tapa synth` — clap parity with `tapa/steps/synth.py`. Body delegates
//! to the Python bridge for now.

use std::path::PathBuf;

use clap::Parser;

use crate::context::CliContext;
use crate::error::Result;
use crate::steps::python_bridge;

#[allow(
    clippy::struct_excessive_bools,
    reason = "mirrors the click flag surface in tapa/steps/synth.py — every bool \
              is a distinct user-facing flag, so collapsing into an enum would \
              break parity"
)]
#[derive(Debug, Parser)]
#[command(name = "synth", about = "Synthesize the TAPA program into RTL code.")]
pub struct Args {
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

    #[arg(long = "floorplan-config", value_name = "FILE")]
    pub floorplan_config: Option<PathBuf>,

    #[arg(long = "device-config", value_name = "FILE")]
    pub device_config: Option<PathBuf>,

    #[arg(long = "floorplan-path", value_name = "FILE")]
    pub floorplan_path: Option<PathBuf>,
}

fn opt_str(out: &mut Vec<String>, flag: &str, value: Option<&str>) {
    if let Some(v) = value {
        out.push(flag.to_string());
        out.push(v.to_string());
    }
}

fn opt_path(out: &mut Vec<String>, flag: &str, value: Option<&PathBuf>) {
    if let Some(v) = value {
        out.push(flag.to_string());
        out.push(v.display().to_string());
    }
}

pub fn to_python_argv(args: &Args) -> Vec<String> {
    let mut out = Vec::<String>::new();
    opt_str(&mut out, "--part-num", args.part_num.as_deref());
    opt_str(&mut out, "--platform", args.platform.as_deref());
    if let Some(c) = args.clock_period {
        out.push("--clock-period".to_string());
        out.push(c.to_string());
    }
    if let Some(j) = args.jobs {
        out.push("--jobs".to_string());
        out.push(j.to_string());
    }
    out.push(if args.keep_hls_work_dir {
        "--keep-hls-work-dir"
    } else {
        "--remove-hls-work-dir"
    }
    .to_string());
    out.push(if args.skip_hls_based_on_mtime {
        "--skip-hls-based-on-mtime"
    } else {
        "--no-skip-hls-based-on-mtime"
    }
    .to_string());
    out.push("--other-hls-configs".to_string());
    out.push(args.other_hls_configs.clone());
    out.push(if args.enable_synth_util {
        "--enable-synth-util"
    } else {
        "--disable-synth-util"
    }
    .to_string());
    out.push("--override-report-schema-version".to_string());
    out.push(args.override_report_schema_version.clone());
    opt_path(&mut out, "--nonpipeline-fifos", args.nonpipeline_fifos.as_ref());
    out.push(if args.gen_ab_graph {
        "--gen-ab-graph"
    } else {
        "--no-gen-ab-graph"
    }
    .to_string());
    if args.gen_graphir {
        out.push("--gen-graphir".to_string());
    }
    opt_path(&mut out, "--floorplan-config", args.floorplan_config.as_ref());
    opt_path(&mut out, "--device-config", args.device_config.as_ref());
    opt_path(&mut out, "--floorplan-path", args.floorplan_path.as_ref());
    out
}

pub fn run(args: &Args, ctx: &mut CliContext) -> Result<()> {
    python_bridge::require_enabled("synth")?;
    python_bridge::run("synth", &to_python_argv(args), ctx)
}
