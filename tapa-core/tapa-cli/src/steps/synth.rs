//! `tapa synth` — native Rust port of `tapa/steps/synth.py`.
//!
//! Drives device-info parsing through `tapa_xilinx::parse_device_info`
//! and persists the resolved part / clock / platform into
//! `<work_dir>/settings.json` so chained pack invocations can wire the
//! kernel without re-resolving the platform. The actual HLS + RTL
//! codegen pipeline (`Program.run_hls` + `generate_task_rtl` +
//! `generate_top_rtl`) is **not** yet ported natively — callers that
//! need it must opt in to the Python fallback via
//! `TAPA_STEP_SYNTH_PYTHON=1`. The native dispatcher surfaces that
//! requirement as a typed [`CliError::InvalidArg`] so the failure mode
//! is clear instead of silently producing an empty `rtl/` tree.

use std::path::{Path, PathBuf};

use clap::Parser;
use serde_json::{json, Value};
use tapa_xilinx::{parse_device_info as xilinx_parse_device_info, DeviceInfo};

use crate::context::CliContext;
use crate::error::{CliError, Result};
use crate::state::{design as design_io, settings as settings_io};
use crate::steps::python_bridge;

#[allow(
    clippy::struct_excessive_bools,
    reason = "mirrors the click flag surface in tapa/steps/synth.py — every bool \
              is a distinct user-facing flag, so collapsing into an enum would \
              break parity"
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

pub fn to_python_argv(args: &SynthArgs) -> Vec<String> {
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

/// Top-level dispatcher: route to the Python bridge when the user has
/// opted in via `TAPA_STEP_SYNTH_PYTHON=1`, otherwise execute the
/// native preflight + settings-update path.
pub fn run(args: &SynthArgs, ctx: &mut CliContext) -> Result<()> {
    if python_bridge::is_enabled("synth") {
        return python_bridge::run("synth", &to_python_argv(args), ctx);
    }
    run_native(args, ctx)
}

/// Native synth preflight: validate the flag surface, resolve the
/// device, and persist the resolved settings under `work_dir`. The
/// actual HLS + RTL codegen is deferred to the Python bridge — see
/// the module-level docstring for the rationale.
fn run_native(args: &SynthArgs, ctx: &CliContext) -> Result<()> {
    reject_unsupported_flags(args)?;

    // The Python `Program` is loaded from `<work_dir>/design.json`;
    // mirror the load so we surface a typed `MissingState` if the
    // user invoked synth before analyze.
    let design = design_io::load_design(&ctx.work_dir)?;
    let mut settings = settings_io::load_settings(&ctx.work_dir)?;
    let target = settings
        .get("target")
        .and_then(Value::as_str)
        .unwrap_or(&design.target)
        .to_string();

    let device = resolve_device_info(args)?;

    settings.insert("part_num".to_string(), json!(&device.part_num));
    settings.insert(
        "platform".to_string(),
        args.platform
            .as_ref()
            .map_or(Value::Null, |p| Value::String(p.clone())),
    );
    settings.insert(
        "clock_period".to_string(),
        json!(&device.clock_period),
    );
    settings_io::store_settings(&ctx.work_dir, &settings)?;

    // Cache the resolved settings + design for chained steps in this
    // process so a follow-on `pack` invocation does not reload from
    // disk twice.
    let mut flow = ctx.flow.borrow_mut();
    flow.settings = Some(settings);
    flow.design = Some(design);
    drop(flow);

    // The HLS + codegen pipeline is the un-ported scope. Surface a
    // typed error rather than silently dropping the user's request
    // for synthesis.
    Err(CliError::InvalidArg(format!(
        "native synth resolved target=`{target}`, part_num=`{}`, clock_period=`{}` and \
         persisted them to `settings.json`, but the HLS + RTL codegen pipeline \
         (`Program.run_hls` + `generate_task_rtl` + `generate_top_rtl`) is not yet \
         ported. Rerun with `TAPA_STEP_SYNTH_PYTHON=1` to drive the Python fallback.",
        device.part_num, device.clock_period,
    )))
}

/// Reject the synth feature surface that has no native equivalent yet.
fn reject_unsupported_flags(args: &SynthArgs) -> Result<()> {
    if args.nonpipeline_fifos.is_some() {
        return Err(CliError::InvalidArg(
            "`--nonpipeline-fifos` requires the Python `grouping_constraints.json` \
             generator; rerun with `TAPA_STEP_SYNTH_PYTHON=1`."
                .to_string(),
        ));
    }
    if args.gen_ab_graph {
        return Err(CliError::InvalidArg(
            "`--gen-ab-graph` requires the Python AutoBridge graph generator; \
             rerun with `TAPA_STEP_SYNTH_PYTHON=1`."
                .to_string(),
        ));
    }
    if args.gen_graphir {
        return Err(CliError::InvalidArg(
            "`--gen-graphir` requires the Python GraphIR project conversion; \
             rerun with `TAPA_STEP_SYNTH_PYTHON=1`."
                .to_string(),
        ));
    }
    if args.floorplan_path.is_some() {
        return Err(CliError::InvalidArg(
            "`--floorplan-path` requires the Python floorplan-aware codegen path; \
             rerun with `TAPA_STEP_SYNTH_PYTHON=1`."
                .to_string(),
        ));
    }
    Ok(())
}

/// Resolve the target part and clock period using the same rules
/// `tapa.backend.device_config.parse_device_info` enforces:
///
///   - When `--platform` is provided, look it up under standard
///     `/opt/xilinx/platforms` (or `$XILINX_VITIS/platforms`,
///     `$XILINX_SDX/platforms`) and parse the `.xpfm` archive.
///   - Otherwise, both `--part-num` and `--clock-period` are required.
///   - In either branch, explicit `--part-num` / `--clock-period` flags
///     override the platform-derived values.
fn resolve_device_info(args: &SynthArgs) -> Result<DeviceInfo> {
    let part_override = args.part_num.as_deref();
    // Python coerces `clock_period` to a string before storing.
    let clock_override_owned = args.clock_period.map(|c| format!("{c}"));
    let clock_override = clock_override_owned.as_deref();

    if let Some(platform) = args.platform.as_deref() {
        let resolved = resolve_platform_dir(platform).ok_or_else(|| {
            CliError::InvalidArg(format!(
                "cannot find the specified platform `{platform}`; are you sure it has \
                 been installed, e.g., in `/opt/xilinx/platforms`?",
            ))
        })?;
        return Ok(xilinx_parse_device_info(
            &resolved,
            part_override,
            clock_override,
        )?);
    }

    let Some(part_num) = part_override else {
        return Err(CliError::InvalidArg(
            "cannot determine the target part number; please either specify \
             `--platform` so the target part number can be extracted from it, or \
             specify `--part-num` directly."
                .to_string(),
        ));
    };
    let Some(clock_period) = clock_override else {
        return Err(CliError::InvalidArg(
            "cannot determine the target clock period; please either specify \
             `--platform` so the target clock period can be extracted from it, or \
             specify `--clock-period` directly."
                .to_string(),
        ));
    };
    Ok(DeviceInfo {
        part_num: part_num.to_string(),
        clock_period: clock_period.to_string(),
    })
}

/// Resolve a `--platform` argument to an on-disk directory. Mirrors
/// Python's lookup chain (`/opt/xilinx`, `$XILINX_VITIS`,
/// `$XILINX_SDX`) and the `:`/`.` → `_` basename normalization.
fn resolve_platform_dir(platform: &str) -> Option<PathBuf> {
    let raw = Path::new(platform);
    let parent = raw.parent().map(Path::to_path_buf).unwrap_or_default();
    let basename = raw.file_name().map_or_else(
        || platform.to_string(),
        |s| s.to_string_lossy().into_owned(),
    );
    let normalized = basename.replace([':', '.'], "_");
    let direct = if parent.as_os_str().is_empty() {
        PathBuf::from(&normalized)
    } else {
        parent.join(&normalized)
    };
    if direct.is_dir() {
        return Some(direct);
    }
    for root in platform_roots() {
        let candidate = root.join("platforms").join(&normalized);
        if candidate.is_dir() {
            return Some(candidate);
        }
    }
    None
}

fn platform_roots() -> Vec<PathBuf> {
    let mut out = vec![PathBuf::from("/opt/xilinx")];
    if let Ok(p) = std::env::var("XILINX_VITIS") {
        out.push(PathBuf::from(p));
    }
    if let Ok(p) = std::env::var("XILINX_SDX") {
        out.push(PathBuf::from(p));
    }
    out
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::similar_names,
        reason = "the `args`/`argv` pair appears throughout the dispatcher; \
                  matching the production names keeps tests legible"
    )]

    use super::*;

    use indexmap::IndexMap;
    use tapa_task_graph::{Design, TaskTopology};

    use crate::globals::GlobalArgs;

    fn parse_synth(extra: &[&str]) -> SynthArgs {
        let mut argv = vec!["synth"];
        argv.extend_from_slice(extra);
        SynthArgs::try_parse_from(argv).expect("parse synth args")
    }

    fn write_design(work_dir: &Path) {
        std::fs::create_dir_all(work_dir).expect("mkdir work");
        let mut tasks = IndexMap::new();
        tasks.insert(
            "Top".to_string(),
            TaskTopology {
                name: "Top".to_string(),
                level: "lower".to_string(),
                code: "void Top() {}".to_string(),
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
        let design = Design {
            top: "Top".to_string(),
            target: "xilinx-hls".to_string(),
            tasks,
            slot_task_name_to_fp_region: None,
        };
        design_io::store_design(work_dir, &design).expect("store design");
        let mut settings = settings_io::Settings::new();
        settings.insert("target".to_string(), json!("xilinx-hls"));
        settings_io::store_settings(work_dir, &settings).expect("store settings");
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

    #[test]
    fn argv_round_trips_python_shape() {
        let args = parse_synth(&["--platform", "xilinx_u250", "--clock-period", "3.33"]);
        let argv = to_python_argv(&args);
        assert!(argv.contains(&"--platform".to_string()));
        assert!(argv.contains(&"xilinx_u250".to_string()));
        assert!(argv.contains(&"--clock-period".to_string()));
    }

    #[test]
    fn unsupported_flag_surfaces_invalid_arg() {
        let args = parse_synth(&[
            "--platform",
            "xilinx_u250",
            "--gen-graphir",
        ]);
        let dir = tempfile::tempdir().expect("tempdir");
        let ctx = ctx_with_work_dir(dir.path());
        let err = run_native(&args, &ctx).expect_err("must reject gen-graphir");
        assert!(matches!(err, CliError::InvalidArg(ref m) if m.contains("--gen-graphir")));
    }

    #[test]
    fn part_num_without_clock_errors() {
        let args = parse_synth(&["--part-num", "xcvu37p"]);
        let err = resolve_device_info(&args).expect_err("missing clock");
        assert!(matches!(err, CliError::InvalidArg(ref m) if m.contains("clock period")));
    }

    #[test]
    fn part_num_and_clock_resolve_without_platform() {
        let args = parse_synth(&["--part-num", "xcvu37p-fsvh2892-2L-e", "--clock-period", "3.33"]);
        let info = resolve_device_info(&args).expect("must resolve");
        assert_eq!(info.part_num, "xcvu37p-fsvh2892-2L-e");
        assert_eq!(info.clock_period, "3.33");
    }

    #[test]
    fn resolve_platform_dir_normalizes_separators() {
        let dir = tempfile::tempdir().expect("tempdir");
        let raw = "weird_platform:1.0";
        let normalized = "weird_platform_1_0";
        let target = dir.path().join(normalized);
        std::fs::create_dir_all(&target).expect("mkdir");
        let qualified = dir.path().join(raw);
        let resolved = resolve_platform_dir(qualified.to_str().expect("utf-8"))
            .expect("must resolve normalized basename");
        assert_eq!(resolved, target);
    }

    #[test]
    fn settings_persisted_before_unported_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_design(dir.path());
        let ctx = ctx_with_work_dir(dir.path());
        let args = parse_synth(&[
            "--part-num",
            "xcvu37p-fsvh2892-2L-e",
            "--clock-period",
            "3.33",
        ]);
        let err = run_native(&args, &ctx).expect_err("HLS pipeline must be unported");
        assert!(matches!(err, CliError::InvalidArg(_)));
        let settings = settings_io::load_settings(dir.path()).expect("settings persisted");
        assert_eq!(settings.get("part_num"), Some(&json!("xcvu37p-fsvh2892-2L-e")));
        assert_eq!(settings.get("clock_period"), Some(&json!("3.33")));
        assert_eq!(settings.get("platform"), Some(&Value::Null));
    }
}
