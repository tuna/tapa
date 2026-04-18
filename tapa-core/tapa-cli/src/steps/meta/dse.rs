//! `tapa compile-with-floorplan-dse` — Stage 1 enumerates floorplan
//! solutions via `generate-floorplan`; Stage 2 re-runs `compile` against
//! each solution under its own `<work-dir>/solution_*` directory.

use std::path::{Path, PathBuf};

use clap::Parser;

use crate::context::{CliContext, FlowState};
use crate::error::{CliError, Result};
use crate::steps::{analyze, floorplan, pack, synth};

use super::{run_compile_composite, run_generate_floorplan_composite, CompileArgs, GenerateFloorplanArgs};


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
    // ---- analyze ----
    #[arg(short = 'f', long = "input", value_name = "FILE", required = true)]
    pub input_files: Vec<PathBuf>,
    #[arg(short = 't', long = "top", value_name = "TASK", required = true)]
    pub top: String,
    #[arg(short = 'c', long = "cflags", value_name = "FLAG")]
    pub cflags: Vec<String>,
    /// Click forwards `--flatten-hierarchy` from the unioned analyze
    /// flag surface; the DSE driver always overrides this to true at
    /// stage 1 (matching `meta.compile_with_floorplan_dse`'s
    /// `kwargs["flatten_hierarchy"] = True` line) so the user-visible
    /// flag is informational + parity-only.
    #[arg(long = "flatten-hierarchy", default_value_t = false)]
    pub flatten_hierarchy: bool,
    #[arg(long = "keep-hierarchy", conflicts_with = "flatten_hierarchy")]
    pub keep_hierarchy: bool,
    #[arg(long = "target", default_value = "xilinx-vitis")]
    pub target: String,
    // ---- synth (omit shared flags; provided below) ----
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
    /// `--enable-synth-util` / `--disable-synth-util` are click flags
    /// forwarded by the composite. `meta.compile_with_floorplan_dse`
    /// sets `enable_synth_util=true` for stage-1 generate-floorplan
    /// and resets to `false` for stage-2 per-solution compile.
    #[arg(long = "enable-synth-util", default_value_t = false)]
    pub enable_synth_util: bool,
    #[arg(long = "disable-synth-util", conflicts_with = "enable_synth_util")]
    pub disable_synth_util: bool,
    #[arg(long = "override-report-schema-version", default_value = "")]
    pub override_report_schema_version: String,
    #[arg(long = "nonpipeline-fifos", value_name = "FILE")]
    pub nonpipeline_fifos: Option<PathBuf>,
    /// `--gen-ab-graph` / `--no-gen-ab-graph` — composite forces
    /// `gen_ab_graph=true` for stage 1 and `false` for stage 2.
    #[arg(long = "gen-ab-graph", default_value_t = false)]
    pub gen_ab_graph: bool,
    #[arg(long = "no-gen-ab-graph", conflicts_with = "gen_ab_graph")]
    pub no_gen_ab_graph: bool,
    /// `--gen-graphir` — composite sets `gen_graphir=true` for stage 2.
    #[arg(long = "gen-graphir", default_value_t = false)]
    pub gen_graphir: bool,
    /// `--floorplan-path` — composite passes one solution's floorplan
    /// per stage-2 iteration. Setting this on the composite itself is
    /// allowed for parity but will be overridden per-solution.
    #[arg(long = "floorplan-path", value_name = "FILE")]
    pub floorplan_path: Option<PathBuf>,
    // ---- floorplan + synth + run_autobridge shared ----
    #[arg(long = "device-config", value_name = "FILE", required = true)]
    pub device_config: PathBuf,
    #[arg(long = "floorplan-config", value_name = "FILE", required = true)]
    pub floorplan_config: PathBuf,
    // ---- pack ----
    #[arg(short = 'o', long = "output", value_name = "FILE")]
    pub output: Option<PathBuf>,
    #[arg(short = 's', long = "bitstream-script", value_name = "FILE")]
    pub bitstream_script: Option<PathBuf>,
    #[arg(long = "custom-rtl", value_name = "PATH")]
    pub custom_rtl: Vec<PathBuf>,
    /// `--graphir-path` from `pack.py`; composite forwards through
    /// stage-2's pack invocation.
    #[arg(long = "graphir-path", value_name = "FILE")]
    pub graphir_path: Option<PathBuf>,
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
    // Python bridge is gone as of AC-8; always native.
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
