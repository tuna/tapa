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
    // Python bridge is gone as of AC-8; always native.
    analyze::run(&args.analyze_args(), ctx)?;
    synth::run(&args.synth_args(), ctx)?;
    floorplan::run_run_autobridge(&args.run_autobridge_args(), ctx)
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::similar_names,
        reason = "args/argv pair matches the production naming"
    )]
    use super::*;

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

    // --- Regression fixture for `generate-floorplan` + --enable-synth-util ---
    //
    // Before the post-synth-util port landed, `synth_args().enable_synth_util = true`
    // aborted the composite with `CliError::InvalidArg("... requires Vivado on PATH ...")`.
    // These helpers stand up a minimal analyze-output fixture + a combined
    // HLS/Vivado mock runner so the synth step runs end-to-end with the
    // post-synth-util branch live.

    use std::sync::Mutex;

    use tapa_xilinx::{ToolInvocation, ToolOutput, ToolRunner};

    /// Dispatch by `inv.program`: `vitis_hls` stages the HLS
    /// output tree, `vivado` writes a canned hierarchical
    /// utilization `.rpt` to the `tclargs[1]` path we recorded.
    struct HlsAndVivadoStub {
        hls_q: Mutex<Vec<String>>,
    }

    impl HlsAndVivadoStub {
        fn stage_hls_output(&self, inv: &ToolInvocation) {
            let cwd = inv.cwd.clone().expect("HLS sets cwd");
            let top = self.hls_q.lock().unwrap().remove(0);
            let syn = cwd.join("project").join(&top).join("syn");
            std::fs::create_dir_all(syn.join("report")).unwrap();
            std::fs::create_dir_all(syn.join("verilog")).unwrap();
            std::fs::write(
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
            .unwrap();
            std::fs::write(
                syn.join("verilog").join(format!("{top}.v")),
                format!(
                    "module {top}(\n  input wire ap_clk,\n  \
                     input wire ap_rst_n,\n  input wire ap_start,\n  \
                     output wire ap_done,\n  output wire ap_idle,\n  \
                     output wire ap_ready\n);\nendmodule\n"
                ),
            )
            .unwrap();
        }

        fn stage_vivado_rpt(inv: &ToolInvocation) {
            let tclargs_pos = inv
                .args
                .iter()
                .position(|a| a == "-tclargs")
                .expect("vivado must receive -tclargs");
            let rpt_path = &inv.args[tclargs_pos + 2];
            let rpt = "Hierarchical Utilization Report\n\
                | Device : xcu250\n\
                +---+----+----+---+----+------+------+\n\
                | Instance | Total LUTs | FFs | DSP Blocks | URAM | RAMB36 | RAMB18 |\n\
                +---+----+----+---+----+------+------+\n\
                | Add | 11 | 22 | 3 | 4 | 5 | 6 |\n\
                +---+----+----+---+----+------+------+\n";
            if let Some(parent) = std::path::Path::new(rpt_path).parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(rpt_path, rpt).unwrap();
        }
    }

    impl ToolRunner for HlsAndVivadoStub {
        fn run(&self, inv: &ToolInvocation) -> tapa_xilinx::Result<ToolOutput> {
            match inv.program.as_str() {
                "vitis_hls" => {
                    self.stage_hls_output(inv);
                    Ok(ToolOutput::default())
                }
                "vivado" => {
                    Self::stage_vivado_rpt(inv);
                    Ok(ToolOutput::default())
                }
                other => panic!("unexpected program: {other}"),
            }
        }
    }

    fn seed_vadd_work_dir(work: &std::path::Path) {
        use indexmap::IndexMap;
        use serde_json::json;
        use tapa_task_graph::{Design, TaskTopology};
        use crate::state::{design as design_io, settings as settings_io};

        let mut tasks = IndexMap::new();
        tasks.insert(
            "Add".to_string(),
            TaskTopology {
                name: "Add".to_string(),
                level: "lower".to_string(),
                code: "void Add() {}\n".to_string(),
                ports: Vec::new(),
                tasks: IndexMap::new(),
                fifos: IndexMap::new(),
                target: Some("hls".to_string()),
                is_slot: false,
                self_area: IndexMap::new(),
                total_area: IndexMap::new(),
                clock_period: "0".to_string(),
            },
        );
        let mut child_tasks = IndexMap::new();
        child_tasks.insert("Add".to_string(), json!([{"args": {}, "step": 0}]));
        tasks.insert(
            "VecAdd".to_string(),
            TaskTopology {
                name: "VecAdd".to_string(),
                level: "upper".to_string(),
                code: "void VecAdd() {}\n".to_string(),
                ports: Vec::new(),
                tasks: child_tasks,
                fifos: IndexMap::new(),
                target: Some("hls".to_string()),
                is_slot: false,
                self_area: IndexMap::new(),
                total_area: IndexMap::new(),
                clock_period: "3.33".to_string(),
            },
        );
        let design = Design {
            top: "VecAdd".to_string(),
            target: "xilinx-hls".to_string(),
            tasks,
            slot_task_name_to_fp_region: None,
        };
        design_io::store_design(work, &design).expect("store design");
        let mut settings = settings_io::Settings::new();
        settings.insert("target".to_string(), json!("xilinx-hls"));
        settings_io::store_settings(work, &settings).expect("store settings");

        // `synth_args()` forces `gen_ab_graph=true` which reads
        // `cpp_arg_pre_assignments` from the floorplan config.
        std::fs::write(
            work.join("fp.json"),
            br#"{"cpp_arg_pre_assignments": {}}"#,
        )
        .expect("seed fp.json");
    }

    /// Regression: `generate-floorplan` forces `enable_synth_util =
    /// true`. This test drives the native synth step through a
    /// combined HLS+Vivado mock so the enable-synth-util path runs to
    /// completion and updates `design.tasks[top].total_area`.
    #[test]
    fn generate_floorplan_synth_step_with_enable_synth_util() {
        use crate::globals::GlobalArgs;
        use crate::state::design as design_io;

        let tmp = tempfile::tempdir().expect("tempdir");
        let work = tmp.path();
        seed_vadd_work_dir(work);

        let globals = GlobalArgs::try_parse_from([
            "tapa",
            "--work-dir",
            work.to_str().expect("utf-8"),
        ])
        .expect("parse globals");
        let ctx = CliContext::from_globals(&globals);

        let fp_args = GenerateFloorplanArgs::try_parse_from([
            "generate-floorplan",
            "--input",
            "a.cpp",
            "--top",
            "VecAdd",
            "--part-num",
            "xcvu37p-fsvh2892-2L-e",
            "--clock-period",
            "3.33",
            "--device-config",
            "dev.json",
            "--floorplan-config",
            work.join("fp.json").to_str().expect("utf-8"),
        ])
        .expect("parse");
        let synth_args = fp_args.synth_args();
        assert!(
            synth_args.enable_synth_util,
            "generate-floorplan must force --enable-synth-util",
        );

        let runner = HlsAndVivadoStub {
            hls_q: Mutex::new(vec!["Add".to_string(), "VecAdd".to_string()]),
        };
        synth::run_native(&synth_args, &ctx, &runner)
            .expect("synth with enable_synth_util must no longer reject");

        // The post-synth-util fold must have rewritten `Add.total_area`.
        let reloaded =
            design_io::load_design(work).expect("reload design after synth");
        let add = reloaded.tasks.get("Add").expect("Add present");
        assert_eq!(
            add.total_area.get("LUT"),
            Some(&serde_json::json!(11)),
            "post-synth-util must update LUT from the Vivado rpt",
        );
        // BRAM_18K = RAMB36*2 + RAMB18 = 5*2 + 6 = 16
        assert_eq!(
            add.total_area.get("BRAM_18K"),
            Some(&serde_json::json!(16)),
        );
    }
}
