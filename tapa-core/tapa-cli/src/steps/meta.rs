//! Composite click commands.
//!
//! `tapa compile`, `tapa generate-floorplan`, and
//! `tapa compile-with-floorplan-dse` mirror the click commands of the
//! same names in `tapa/steps/meta.py`. Each composite materializes the
//! union of its constituent click commands' flag surfaces — exactly
//! what `_extend_params` does in Python — by flattening the underlying
//! `Args` structs (or, where flag conflicts would arise, by
//! hand-rolling the merged flag set).

use std::path::PathBuf;

use clap::Parser;

use crate::context::CliContext;
use crate::error::{CliError, Result};
use crate::steps::{analyze, floorplan, pack, python_bridge, synth};

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
    #[arg(long = "target", default_value = "xilinx-vitis")]
    pub target: String,
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
            target: self.target.clone(),
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
    analyze::run(&args.analyze_args(), ctx)?;
    synth::run(&args.synth_args(), ctx)?;
    floorplan::run_run_autobridge(&args.run_autobridge_args(), ctx)
}

// ---------------------------------------------------------------------
// `compile-with-floorplan-dse` — analyze + floorplan + synth +
// run_autobridge + pack
// ---------------------------------------------------------------------

#[allow(
    clippy::struct_excessive_bools,
    reason = "merged click flag surface — collapsing into an enum would break parity"
)]
#[derive(Debug, Clone, Parser)]
#[command(
    name = "compile-with-floorplan-dse",
    about = "Compile a TAPA program with floorplan design space exploration."
)]
pub struct CompileWithFloorplanDseArgs {
    // analyze
    #[arg(short = 'f', long = "input", value_name = "FILE", required = true)]
    pub input_files: Vec<PathBuf>,
    #[arg(short = 't', long = "top", value_name = "TASK", required = true)]
    pub top: String,
    #[arg(short = 'c', long = "cflags", value_name = "FLAG")]
    pub cflags: Vec<String>,
    #[arg(long = "target", default_value = "xilinx-vitis")]
    pub target: String,
    // synth (omit conflicting flags; provided below)
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
    #[arg(long = "override-report-schema-version", default_value = "")]
    pub override_report_schema_version: String,
    #[arg(long = "nonpipeline-fifos", value_name = "FILE")]
    pub nonpipeline_fifos: Option<PathBuf>,
    // floorplan + synth + run_autobridge shared
    #[arg(long = "device-config", value_name = "FILE", required = true)]
    pub device_config: PathBuf,
    #[arg(long = "floorplan-config", value_name = "FILE", required = true)]
    pub floorplan_config: PathBuf,
    // pack
    #[arg(short = 'o', long = "output", value_name = "FILE")]
    pub output: Option<PathBuf>,
    #[arg(short = 's', long = "bitstream-script", value_name = "FILE")]
    pub bitstream_script: Option<PathBuf>,
    #[arg(long = "custom-rtl", value_name = "PATH")]
    pub custom_rtl: Vec<PathBuf>,
}

pub fn run_compile_with_floorplan_dse_composite(
    args: &CompileWithFloorplanDseArgs,
    ctx: &mut CliContext,
) -> Result<()> {
    if args.output.is_some() {
        return Err(CliError::InvalidArg(
            "compile-with-floorplan-dse: --output must not be specified \
             (each floorplan solution writes its own output)"
                .to_string(),
        ));
    }
    // Stage 1: full DSE drives generate-floorplan to enumerate floorplans.
    let gf = GenerateFloorplanArgs {
        input_files: args.input_files.clone(),
        top: args.top.clone(),
        cflags: args.cflags.clone(),
        flatten_hierarchy: true,
        keep_hierarchy: false,
        target: args.target.clone(),
        part_num: args.part_num.clone(),
        platform: args.platform.clone(),
        clock_period: args.clock_period,
        jobs: args.jobs,
        keep_hls_work_dir: args.keep_hls_work_dir,
        remove_hls_work_dir: args.remove_hls_work_dir,
        skip_hls_based_on_mtime: args.skip_hls_based_on_mtime,
        no_skip_hls_based_on_mtime: args.no_skip_hls_based_on_mtime,
        other_hls_configs: args.other_hls_configs.clone(),
        enable_synth_util: true,
        disable_synth_util: false,
        override_report_schema_version: args.override_report_schema_version.clone(),
        nonpipeline_fifos: args.nonpipeline_fifos.clone(),
        gen_ab_graph: true,
        no_gen_ab_graph: false,
        gen_graphir: false,
        floorplan_path: None,
        device_config: args.device_config.clone(),
        floorplan_config: args.floorplan_config.clone(),
    };
    run_generate_floorplan_composite(&gf, ctx)?;

    // Stage 2: re-run compile per floorplan solution. The Python
    // implementation iterates `<work-dir>/autobridge/solution_*/floorplan.json`
    // and re-invokes `compile`. While the Rust per-solution loop matures
    // alongside the native ports, route through the Python composite so
    // nothing breaks on existing flows.
    python_bridge::require_enabled("compile-with-floorplan-dse")?;
    python_bridge::run(
        "compile-with-floorplan-dse",
        &compile_with_floorplan_dse_python_argv(args),
        ctx,
    )
}

fn compile_with_floorplan_dse_python_argv(
    args: &CompileWithFloorplanDseArgs,
) -> Vec<String> {
    let mut out = Vec::<String>::new();
    for f in &args.input_files {
        out.push("--input".to_string());
        out.push(f.display().to_string());
    }
    out.push("--top".to_string());
    out.push(args.top.clone());
    for c in &args.cflags {
        out.push("--cflags".to_string());
        out.push(c.clone());
    }
    out.push("--target".to_string());
    out.push(args.target.clone());
    if let Some(p) = &args.platform {
        out.push("--platform".to_string());
        out.push(p.clone());
    }
    if let Some(p) = &args.part_num {
        out.push("--part-num".to_string());
        out.push(p.clone());
    }
    if let Some(c) = args.clock_period {
        out.push("--clock-period".to_string());
        out.push(c.to_string());
    }
    if let Some(j) = args.jobs {
        out.push("--jobs".to_string());
        out.push(j.to_string());
    }
    out.push("--device-config".to_string());
    out.push(args.device_config.display().to_string());
    out.push("--floorplan-config".to_string());
    out.push(args.floorplan_config.display().to_string());
    out
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::similar_names,
        reason = "args/argv pair matches the production naming"
    )]
    use super::*;
    use crate::globals::GlobalArgs;

    #[test]
    fn compile_args_round_trip_via_clap() {
        let args = CompileArgs::try_parse_from([
            "compile",
            "--input",
            "vadd.cpp",
            "--top",
            "VecAdd",
            "--platform",
            "xilinx_u250",
            "--output",
            "vadd.xo",
        ])
        .expect("compile args parse");
        assert_eq!(args.analyze.input_files.len(), 1);
        assert_eq!(args.analyze.top, "VecAdd");
        assert_eq!(args.synth.platform.as_deref(), Some("xilinx_u250"));
        assert_eq!(
            args.pack.output.as_ref().map(|p| p.display().to_string()),
            Some("vadd.xo".to_string()),
        );
    }

    #[test]
    fn generate_floorplan_args_parse() {
        let args = GenerateFloorplanArgs::try_parse_from([
            "generate-floorplan",
            "--input",
            "a.cpp",
            "--top",
            "T",
            "--platform",
            "xilinx_u250",
            "--device-config",
            "dev.json",
            "--floorplan-config",
            "fp.json",
        ])
        .expect("parse");
        assert_eq!(args.top, "T");
        let synth_args = args.synth_args();
        // Composites force flatten_hierarchy / enable_synth_util / gen_ab_graph
        // per Python `kwargs[...] = True` assignments in `meta.py`.
        assert!(synth_args.gen_ab_graph);
        assert!(synth_args.enable_synth_util);
        assert!(args.analyze_args().flatten_hierarchy);
    }

    #[test]
    fn compile_with_floorplan_dse_rejects_output_flag() {
        let args = CompileWithFloorplanDseArgs::try_parse_from([
            "compile-with-floorplan-dse",
            "--input",
            "a.cpp",
            "--top",
            "T",
            "--device-config",
            "dev.json",
            "--floorplan-config",
            "fp.json",
            "--output",
            "out.xo",
        ])
        .expect("clap accepts --output (the runtime check rejects it)");
        // Build a dummy ctx to drive the runtime check.
        let globals = GlobalArgs {
            verbose: 0,
            quiet: 0,
            work_dir: std::env::temp_dir(),
            temp_dir: None,
            clang_format_quota_in_bytes: 0,
            remote_host: None,
            remote_key_file: None,
            remote_xilinx_settings: None,
            remote_ssh_control_dir: None,
            remote_ssh_control_persist: None,
            remote_disable_ssh_mux: false,
        };
        let mut ctx = CliContext::from_globals(&globals);
        let err = run_compile_with_floorplan_dse_composite(&args, &mut ctx)
            .expect_err("--output should be rejected");
        assert!(err.to_string().contains("--output"));
    }
}
