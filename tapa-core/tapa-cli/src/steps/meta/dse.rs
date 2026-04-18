//! `tapa compile-with-floorplan-dse` — Stage 1 enumerates floorplan
//! solutions via `generate-floorplan`; Stage 2 re-runs `compile` against
//! each solution under its own `<work-dir>/solution_*` directory.

use std::path::{Path, PathBuf};

use clap::Parser;

use crate::context::{CliContext, FlowState};
use crate::error::{CliError, Result};
use crate::steps::{analyze, floorplan, pack, python_bridge, synth};

use super::{run_compile_composite, run_generate_floorplan_composite, CompileArgs, GenerateFloorplanArgs};

const STEP: &str = "compile-with-floorplan-dse";

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
    if python_bridge::is_enabled(STEP) {
        return python_bridge::run(STEP, &python_argv(args), ctx);
    }

    let original_work_dir = ctx.work_dir.clone();

    // Stage 1: drive generate-floorplan to enumerate floorplans.
    run_generate_floorplan_composite(&build_generate_floorplan_stage1(args), ctx)?;

    // Stage 2: per-solution `compile`, each into its own `solution_*` dir.
    let solutions = floorplan::enumerate_solution_floorplans(&original_work_dir)?;
    let mut succeeded = Vec::<PathBuf>::new();
    for floorplan_file in solutions {
        let Some(solution_name) = floorplan_file
            .parent()
            .and_then(Path::file_name)
            .map(|s| s.to_string_lossy().into_owned())
        else {
            continue;
        };
        let solution_work_dir = original_work_dir.join(&solution_name);
        log::info!("Using floorplan file: {}", floorplan_file.display());

        let output = solution_work_dir.join(format!("{solution_name}.xo"));
        let compile = build_compile_stage2(args, &floorplan_file, &output);

        // Reset the in-process flow state between solutions so each
        // sub-compile starts from a clean slate (matches the Python
        // `clean_obj` per-iteration context). Switch the work dir so
        // every step writes under `<orig_work_dir>/<solution_name>/`.
        ctx.flow.replace(FlowState::default());
        if let Err(e) = ctx.switch_work_dir(solution_work_dir.clone()) {
            log::error!(
                "skipping floorplan {}: failed to switch work dir to {}: {e}",
                floorplan_file.display(),
                solution_work_dir.display(),
            );
            continue;
        }

        match run_compile_composite(&compile, ctx) {
            Ok(()) => succeeded.push(floorplan_file.clone()),
            Err(e) => log::error!(
                "Error during compilation with floorplan {}: {e}",
                floorplan_file.display(),
            ),
        }
    }

    // Restore the original work dir; downstream chained steps (if any)
    // expect to see the top-level state, not a per-solution directory.
    if let Err(e) = ctx.switch_work_dir(original_work_dir.clone()) {
        log::warn!(
            "failed to restore work dir to {}: {e}",
            original_work_dir.display(),
        );
    }

    log::info!(
        "Found {} successful compilations with floorplan.",
        succeeded.len(),
    );
    for floorplan_file in &succeeded {
        log::info!(
            "Successful compilation with floorplan: {}",
            floorplan_file.display(),
        );
    }
    Ok(())
}

fn build_generate_floorplan_stage1(
    args: &CompileWithFloorplanDseArgs,
) -> GenerateFloorplanArgs {
    GenerateFloorplanArgs {
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
    }
}

fn build_compile_stage2(
    args: &CompileWithFloorplanDseArgs,
    floorplan_path: &Path,
    output: &Path,
) -> CompileArgs {
    // Stage 2 mirrors Python's `kwargs[...] = ...` overrides: enable
    // graphir generation, disable utilization estimates and ab-graph
    // generation, and apply the per-solution floorplan_path / output.
    let analyze_args = analyze::AnalyzeArgs {
        input_files: args.input_files.clone(),
        top: args.top.clone(),
        cflags: args.cflags.clone(),
        flatten_hierarchy: true,
        keep_hierarchy: false,
        target: args.target.clone(),
    };
    let synth_args = synth::SynthArgs {
        part_num: args.part_num.clone(),
        platform: args.platform.clone(),
        clock_period: args.clock_period,
        jobs: args.jobs,
        keep_hls_work_dir: args.keep_hls_work_dir,
        remove_hls_work_dir: args.remove_hls_work_dir,
        skip_hls_based_on_mtime: args.skip_hls_based_on_mtime,
        no_skip_hls_based_on_mtime: args.no_skip_hls_based_on_mtime,
        other_hls_configs: args.other_hls_configs.clone(),
        enable_synth_util: false,
        disable_synth_util: true,
        override_report_schema_version: args.override_report_schema_version.clone(),
        nonpipeline_fifos: args.nonpipeline_fifos.clone(),
        gen_ab_graph: false,
        no_gen_ab_graph: true,
        gen_graphir: true,
        floorplan_config: Some(args.floorplan_config.clone()),
        device_config: Some(args.device_config.clone()),
        floorplan_path: Some(floorplan_path.to_path_buf()),
    };
    let pack_args = pack::PackArgs {
        output: Some(output.to_path_buf()),
        bitstream_script: args.bitstream_script.clone(),
        custom_rtl: args.custom_rtl.clone(),
        graphir_path: None,
    };
    CompileArgs {
        analyze: analyze_args,
        synth: synth_args,
        pack: pack_args,
    }
}

/// Render the DSE composite as the click argv shape Python expects.
///
/// We compose by reusing each per-step's `to_python_argv` over a Stage-1
/// projection of the merged flag surface — the shape matches what
/// `tapa compile-with-floorplan-dse` accepts via `_extend_params`.
fn python_argv(args: &CompileWithFloorplanDseArgs) -> Vec<String> {
    let stage1 = build_generate_floorplan_stage1(args);
    let mut out = analyze::to_python_argv(&stage1.analyze_args());
    out.extend(synth::to_python_argv(&stage1.synth_args()));
    out.extend(floorplan::to_python_argv_run_autobridge(
        &stage1.run_autobridge_args(),
    ));
    let pack_args = pack::PackArgs {
        output: None,
        bitstream_script: args.bitstream_script.clone(),
        custom_rtl: args.custom_rtl.clone(),
        graphir_path: None,
    };
    out.extend(pack::to_python_argv(&pack_args));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::globals::GlobalArgs;

    #[test]
    fn rejects_output_flag() {
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
