//! `tapa analyze` orchestration.
//!
//! Hands the original input translation units to `tapacc`, which mirrors and
//! rewrites them into `<work_dir>/rewritten/` as one guarded source tree, then
//! writes the work dir's one state file, `<work_dir>/tapa.json`, plus the
//! verbatim `tapacc` output as a debug artifact.

use std::fs;
use std::path::PathBuf;

use clap::{Parser, ValueEnum};
use serde_json::{json, Value};
use tapa_ir::Graph;

use crate::context::CliContext;
use crate::error::{CliError, Result};
use crate::state::json::write_bytes_atomic;
use crate::state::work::{self as work_io, WorkState};
use crate::tapacc::cflags::{get_system_cflags, get_tapacc_cflags};
use crate::tapacc::discover::find_clang_binary;

/// Verbatim `tapacc` stdout, kept under the work dir for provenance and
/// debugging. **Nothing reads this back** — `analyze` parses `tapacc`'s
/// output in-process into the typed model that `tapa.json` carries. Keeping
/// it write-only is what stops the work dir growing a second schema-bearing
/// file that can drift from the first.
const TAPACC_ARTIFACT: &str = "tapacc.json";

mod build_design;
mod run_tapacc;

use build_design::is_top_leaf;
use run_tapacc::run_tapacc;

/// Target flows accepted by `tapa analyze`. Kebab-case spellings match
/// the wire strings persisted into `tapa.json` and read by `synth` /
/// `pack` (`xilinx-vitis`, `xilinx-slash`, `xilinx-hls`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum AnalyzeTarget {
    #[value(name = "xilinx-vitis")]
    XilinxVitis,
    #[value(name = "xilinx-slash")]
    XilinxSlash,
    #[value(name = "xilinx-hls")]
    XilinxHls,
}

impl AnalyzeTarget {
    /// Canonical wire string persisted into `tapa.json`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::XilinxVitis => "xilinx-vitis",
            Self::XilinxSlash => "xilinx-slash",
            Self::XilinxHls => "xilinx-hls",
        }
    }
}

/// A per-task target mapping's target (`TASK:TARGET`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskTarget {
    Aie,
    Hls,
    Ignore,
}

impl TaskTarget {
    /// The spelling `tapacc -task-target` takes.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Aie => "aie",
            Self::Hls => "hls",
            Self::Ignore => "ignore",
        }
    }
}

/// One `--target` value: the flow, or a mapping that overrides one task's
/// `[[tapa::target]]` so one source builds PL-only and PL+AI Engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetArg {
    Flow(AnalyzeTarget),
    Task { task: String, target: TaskTarget },
}

fn parse_target_arg(value: &str) -> std::result::Result<TargetArg, String> {
    let Some((task, target)) = value.rsplit_once(':') else {
        return AnalyzeTarget::from_str(value, false).map(TargetArg::Flow);
    };
    let target = match target {
        "aie" => TaskTarget::Aie,
        "hls" => TaskTarget::Hls,
        "ignore" => TaskTarget::Ignore,
        other => {
            return Err(format!(
                "`{value}` maps task `{task}` to `{other}`; a task target is one of \
                 aie, hls, ignore"
            ))
        }
    };
    if task.is_empty() {
        return Err(format!("`{value}` names no task before the `:`"));
    }
    Ok(TargetArg::Task {
        task: task.to_string(),
        target,
    })
}

/// Bridge the clap arg enum to the schema enum stored in `tapa.json`.
/// Keeps `tapa-ir` free of a clap dependency.
impl From<AnalyzeTarget> for tapa_ir::Target {
    fn from(target: AnalyzeTarget) -> Self {
        match target {
            AnalyzeTarget::XilinxVitis => Self::XilinxVitis,
            AnalyzeTarget::XilinxSlash => Self::XilinxSlash,
            AnalyzeTarget::XilinxHls => Self::XilinxHls,
        }
    }
}

#[derive(Debug, Clone, Parser)]
#[command(
    name = "analyze",
    about = "Analyze TAPA program and store the program description."
)]
pub struct AnalyzeArgs {
    /// Input file, usually TAPA C++ source code (may repeat).
    #[arg(short = 'f', long = "input", value_name = "FILE", required = true)]
    pub input_files: Vec<PathBuf>,

    /// Name of the top-level task.
    #[arg(short = 't', long = "top", value_name = "TASK", required = true)]
    pub top: String,

    /// Compiler flags for the kernel; may appear many times.
    #[arg(short = 'c', long = "cflags", value_name = "FLAG")]
    pub cflags: Vec<String>,

    /// Emit flat RTL with every leaf at top and original controller ownership.
    #[arg(long = "flatten-hierarchy", default_value_t = false)]
    pub flatten_hierarchy: bool,

    /// Counterpart to `--flatten-hierarchy`; default behavior.
    #[arg(long = "keep-hierarchy", conflicts_with = "flatten_hierarchy")]
    pub keep_hierarchy: bool,

    /// Target flow (`xilinx-vitis`, the default, `xilinx-slash`, or `xilinx-hls`), or a
    /// per-task mapping `TASK:TARGET` with TARGET one of `aie`, `hls`,
    /// `ignore`, which wins over the task's `[[tapa::target]]`. Repeatable;
    /// at most one flow. Restricted to known flow targets so typos fail at
    /// parse time instead of producing a `tapa.json` that only blows up
    /// later in `synth` or `pack`.
    #[arg(long = "target", value_name = "TARGET", value_parser = parse_target_arg)]
    pub targets: Vec<TargetArg>,

    /// Explicit path to the `tapacc` binary. Overrides the walk-up
    /// `find_resource` search anchored at the `tapa` binary. Used by
    /// Bazel driver rules (`bazel/tapa_rules.bzl::_tapa_xo_impl`)
    /// that locate the toolchain inputs through their own dep graph
    /// and pass them down explicitly.
    #[arg(long = "tapacc", value_name = "FILE")]
    pub tapacc: Option<PathBuf>,
}

impl AnalyzeArgs {
    /// The flow target: the one non-mapping `--target`, `xilinx-vitis` by
    /// default.
    pub fn flow(&self) -> Result<AnalyzeTarget> {
        let mut flows = self.targets.iter().filter_map(|target| match target {
            TargetArg::Flow(flow) => Some(*flow),
            TargetArg::Task { .. } => None,
        });
        let flow = flows.next().unwrap_or(AnalyzeTarget::XilinxVitis);
        if flows.next().is_some() {
            return Err(CliError::InvalidArg(
                "`--target` names more than one flow; give one of xilinx-vitis, \
                 xilinx-slash, or xilinx-hls"
                    .to_string(),
            ));
        }
        Ok(flow)
    }

    /// The per-task mappings, as `tapacc -task-target` values.
    fn task_targets(&self) -> Vec<String> {
        self.targets
            .iter()
            .filter_map(|target| match target {
                TargetArg::Flow(_) => None,
                TargetArg::Task { task, target } => Some(format!("{task}:{}", target.as_str())),
            })
            .collect()
    }
}

/// Run tapacc on each input and write the work dir.
///
/// Merges the task graphs into `<work_dir>/tapa.json` and materializes the
/// mirrored source tree under `<work_dir>/rewritten/`. The stored graph always
/// retains hierarchy; `--flatten-hierarchy` selects the later RTL layout.
pub fn run(args: &AnalyzeArgs, ctx: &CliContext) -> Result<()> {
    // `--tapacc` overrides the walk-up `find_resource` search. Used by
    // the Bazel driver to inject the exact sandbox paths; direct
    // `tapa analyze` runs on a developer machine still fall through to
    // the default discovery path.
    let tapacc = if let Some(p) = args.tapacc.as_ref() {
        p.clone()
    } else {
        find_clang_binary("tapacc-binary")?
    };

    // Vitis HLS only supports up to C++14; keep it after user flags.
    let mut user_cflags = args.cflags.clone();
    user_cflags.push("-std=c++14".to_string());

    let mut all_cflags = user_cflags.clone();
    all_cflags.extend(get_tapacc_cflags(false));
    all_cflags.extend(get_system_cflags());

    let work_dir = ctx.work_dir.as_path();
    fs::create_dir_all(work_dir)?;
    let flow = args.flow()?;
    let stdout = run_tapacc(
        &tapacc,
        &args.input_files,
        &args.top,
        &all_cflags,
        flow.as_str(),
        &args.task_targets(),
        work_dir,
    )?;

    // Persist the raw bytes first, so a `tapacc` output that fails the parse
    // below is still on disk for inspection.
    write_bytes_atomic(work_dir, TAPACC_ARTIFACT, stdout.as_bytes())?;

    let mut graph_dict: Value = serde_json::from_str(&stdout)?;

    // Analyze owns the two root facts `tapacc` does not emit: the user's
    // cflags tuple (plus the required C++ standard flag) and the flow
    // target. Both are injected before the strict parse below, so the graph
    // conforms to the schema `tapa.json` carries.
    if let Some(obj) = graph_dict.as_object_mut() {
        obj.insert(
            "cflags".to_string(),
            Value::Array(user_cflags.iter().cloned().map(Value::String).collect()),
        );
        obj.insert(
            "target".to_string(),
            json!(tapa_ir::Target::from(flow).as_str()),
        );
    }

    let graph: Graph = serde_json::from_value(graph_dict).map_err(|e| {
        CliError::Codegen(format!(
            "cannot parse the task graph produced by `tapacc`: {e}"
        ))
    })?;

    if is_top_leaf(&graph, &args.top) && flow == AnalyzeTarget::XilinxVitis {
        return Err(CliError::InvalidArg(
            "the top task is a leaf task; target `xilinx-vitis` is not supported \
             (Vitis requires an upper top for kernel.xml generation). \
             Rerun with `--target xilinx-hls`."
                .to_string(),
        ));
    }

    // Seed the work dir's one state file. `synth` annotates the graph in
    // place with post-synthesis results and fills in `flow`.
    let mut state = WorkState::new(graph);
    state.flow.flatten_hierarchy = args.flatten_hierarchy;
    work_io::store(work_dir, &state)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;

    use crate::context::CliContext;
    use crate::globals::GlobalArgs;

    #[test]
    fn target_takes_one_flow_and_task_mappings() {
        let parse = |targets: &[&str]| {
            let mut argv = vec!["analyze", "-f", "a.cpp", "-t", "T"];
            for target in targets {
                argv.extend(["--target", target]);
            }
            AnalyzeArgs::try_parse_from(argv)
        };
        let args = parse(&["xilinx-hls", "Pe:aie", "Load:ignore"]).expect("parse");
        assert_eq!(args.flow().expect("one flow"), AnalyzeTarget::XilinxHls);
        assert_eq!(args.task_targets(), ["Pe:aie", "Load:ignore"]);
        let args = parse(&["xilinx-slash"]).expect("parse SLASH flow");
        assert_eq!(args.flow().expect("one flow"), AnalyzeTarget::XilinxSlash);
        assert_eq!(
            tapa_ir::Target::from(args.flow().expect("one flow")),
            tapa_ir::Target::XilinxSlash,
        );
        assert_eq!(
            parse(&["Pe:aie"])
                .expect("parse")
                .flow()
                .expect("default flow"),
            AnalyzeTarget::XilinxVitis,
        );
        parse(&["Pe:gpu"]).expect_err("unknown task target");
        parse(&[":aie"]).expect_err("mapping without a task");
        parse(&["xilinx-hls", "xilinx-vitis"])
            .expect("parse")
            .flow()
            .expect_err("two flows");
    }

    #[cfg(unix)]
    #[test]
    fn analyze_writes_state_and_tapacc_artifact() {
        use std::os::unix::fs::PermissionsExt;

        // Build an isolated tempdir that doubles as both:
        //   - the search anchor for `find_resource` (POTENTIAL_PATHS roots)
        //   - the work_dir for the analyze step.
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();

        // Plant a fake `tapacc` binary that satisfies
        // `find_clang_binary`'s `--version` probe and emits a fixed graph.
        let tapacc_dir = root.join("tapacc");
        fs::create_dir_all(&tapacc_dir).expect("mkdir tapacc");
        let tapacc = tapacc_dir.join("tapacc");

        // tapacc: `--version` is parseable; otherwise it emits a fixed
        // tapacc-shaped task graph on stdout.
        // Mirrors real `tapacc` output: `readable_name` is emitted for every
        // task (equal to the task name for these non-template tasks).
        let fixed_graph = r#"{"schema_version": 3, "cflags": [], "tasks": {"VecAdd": {"srcs": ["VecAdd.cpp"], "include_dirs": [], "defines": [], "level": "upper", "synth": "hls", "readable_name": "VecAdd", "ports": [], "tasks": {"Middle": [{"step": -1, "args": {}}]}, "fifos": {}}, "Middle": {"srcs": ["Middle.cpp"], "include_dirs": [], "defines": [], "level": "upper", "synth": "hls", "readable_name": "Middle", "ports": [], "tasks": {"Add": [{"step": 0, "args": {}}]}, "fifos": {}}, "Add": {"srcs": ["Add.cpp"], "include_dirs": [], "defines": [], "level": "lower", "synth": "hls", "readable_name": "Add", "ports": []}}, "top": "VecAdd"}"#;
        fs::write(
            &tapacc,
            format!(
                "#!/bin/sh\n\
                 if [ \"$1\" = \"--version\" ]; then\n\
                   echo 'fake tapacc version 18.0.0'\n\
                   exit 0\n\
                 fi\n\
                 cat <<'__JSON__'\n{fixed_graph}\n__JSON__\n",
            ),
        )
        .expect("write tapacc");
        fs::set_permissions(&tapacc, fs::Permissions::from_mode(0o755)).expect("chmod tapacc");

        // Plant a trivial input file under the same root.
        let input_file = root.join("vadd.cpp");
        fs::write(&input_file, b"void VecAdd() {}\n").expect("write vadd.cpp");

        // Steer `find_resource` at `root` so the planted binary wins.
        std::env::set_var("TAPA_CLI_SEARCH_ANCHOR", root);
        let work_dir = root.join("work");
        let globals = GlobalArgs::try_parse_from([
            "tapa",
            "--work-dir",
            work_dir.to_str().expect("utf-8 work dir"),
        ])
        .expect("parse globals");
        let ctx = CliContext::from_globals(&globals);

        let args = AnalyzeArgs::try_parse_from([
            "analyze",
            "--input",
            input_file.to_str().expect("utf-8 path"),
            "--top",
            "VecAdd",
            "--target",
            "xilinx-hls",
        ])
        .expect("parse analyze args");

        run(&args, &ctx).expect("native analyze should succeed");

        // The single state file must round-trip with the projected topology.
        let state = work_io::load(&ctx.work_dir).expect("load state");
        assert_eq!(state.version, work_io::VERSION, "state is version-stamped");
        assert_eq!(state.graph.top, "VecAdd");
        // Analyze injects the root flow target `tapacc` does not emit.
        assert_eq!(state.graph.target.as_str(), "xilinx-hls");
        // Analyze stores the user cflags plus `-std=c++14`. The user passed
        // no `-c`, so we expect just that.
        assert_eq!(state.graph.cflags, vec!["-std=c++14".to_string()]);
        assert!(state.graph.tasks.contains_key("VecAdd"));
        assert!(state.graph.tasks.contains_key("Add"));
        assert_eq!(state.graph.tasks["VecAdd"].level, tapa_ir::TaskLevel::Upper);
        assert_eq!(state.graph.tasks["Add"].level, tapa_ir::TaskLevel::Lower);
        assert_eq!(state.graph.tasks["Add"].synth, tapa_ir::SynthTarget::Hls);
        // Nothing has synthesized yet, so the flow block is still pristine.
        assert_eq!(state.flow, crate::state::work::FlowSettings::default());

        // The superseded state files must not reappear: one schema-bearing
        // file in the work dir is the whole point.
        for stale in ["graph.json", "design.json", "settings.json"] {
            assert!(
                !ctx.work_dir.join(stale).exists(),
                "`{stale}` must no longer be written",
            );
        }

        // The verbatim tapacc output is kept for debugging, byte-for-byte,
        // *without* analyze's injected cflags/target.
        let raw = fs::read_to_string(ctx.work_dir.join("tapacc.json")).expect("read tapacc.json");
        assert_eq!(
            raw.trim_end(),
            fixed_graph,
            "tapacc.json must hold tapacc's stdout verbatim",
        );

        let mut flattened_args = args;
        flattened_args.flatten_hierarchy = true;
        run(&flattened_args, &ctx).expect("analyze with flat RTL should succeed");
        let flat_state = work_io::load(&ctx.work_dir).expect("load flat-emission state");
        assert_eq!(
            flat_state.graph, state.graph,
            "physical flattening must retain detached upper ownership and joined children",
        );
        assert!(
            flat_state.flow.flatten_hierarchy,
            "the RTL layout choice must survive between pipeline commands",
        );
    }
}
