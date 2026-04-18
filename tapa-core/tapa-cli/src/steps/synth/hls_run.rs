//! Per-task Vitis HLS invocation, ported from
//! `tapa/program/hls.py::ProgramHlsMixin._run_hls_task` + `.run_hls`.

use std::fs;
use std::path::{Path, PathBuf};

use tapa_task_graph::Design;
use tapa_xilinx::{run_hls, HlsJob, HlsOutput, ToolRunner};

use crate::error::{CliError, Result};
use crate::steps::synth::cpp_extract::cpp_path_for;

#[derive(Debug, Clone)]
pub struct TaskHlsLayout {
    pub reports_dir: PathBuf,
    pub hdl_dir: PathBuf,
}

impl TaskHlsLayout {
    pub fn new(work_dir: &Path, task_name: &str) -> Self {
        let base = work_dir.join("hls").join(task_name);
        Self {
            reports_dir: base.join("report"),
            hdl_dir: base.join("verilog"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct HlsRunOptions {
    pub part_num: String,
    pub clock_period: String,
    pub other_configs: String,
    pub cflags: Vec<String>,
    pub skip_based_on_mtime: bool,
}

/// Run HLS for every task that targets HLS. Mirrors Python's
/// `ProgramHlsMixin.run_hls`, which iterates **all** `_tasks.values()`
/// (not just leaves) — the upper-task shell is needed by codegen so
/// the parent module's port surface is parseable. Tasks whose
/// `target == "ignore"` are skipped (Python promotes them to
/// `gen_templates`).
pub fn run_hls_for_leaves(
    runner: &dyn ToolRunner,
    work_dir: &Path,
    design: &Design,
    options: &HlsRunOptions,
) -> Result<Vec<(String, TaskHlsLayout, HlsOutput)>> {
    let mut out = Vec::new();
    for (task_name, task) in &design.tasks {
        if task.target.as_deref() == Some("ignore") {
            continue;
        }
        let layout = TaskHlsLayout::new(work_dir, task_name);
        fs::create_dir_all(&layout.reports_dir)?;
        fs::create_dir_all(&layout.hdl_dir)?;

        let cpp_source = cpp_path_for(work_dir, task_name);
        if !cpp_source.is_file() {
            return Err(CliError::InvalidArg(format!(
                "missing extracted C++ source `{}` for task `{task_name}`",
                cpp_source.display(),
            )));
        }

        if options.skip_based_on_mtime && hdl_dir_is_newer_than(&layout.hdl_dir, &cpp_source) {
            log::info!(
                "skipping HLS for `{task_name}` (mtime cache hit at {})",
                layout.hdl_dir.display(),
            );
            let verilog_files = list_verilog_files(&layout.hdl_dir)?;
            out.push((
                task_name.clone(),
                layout,
                HlsOutput {
                    csynth: tapa_xilinx::CsynthReport::default(),
                    verilog_files,
                    report_paths: Vec::new(),
                    stdout: String::new(),
                    stderr: String::new(),
                },
            ));
            continue;
        }

        let job = HlsJob {
            task_name: task_name.clone(),
            cpp_source,
            cflags: options.cflags.clone(),
            target_part: options.part_num.clone(),
            top_name: task_name.clone(),
            clock_period: options.clock_period.clone(),
            reports_out_dir: layout.reports_dir.clone(),
            hdl_out_dir: layout.hdl_dir.clone(),
            other_configs: options.other_configs.clone(),
            ..HlsJob::default()
        };

        let hls_out = run_hls(runner, &job)?;
        out.push((task_name.clone(), layout, hls_out));
    }
    Ok(out)
}

fn hdl_dir_is_newer_than(hdl_dir: &Path, cpp_source: &Path) -> bool {
    let Ok(hdl_meta) = fs::metadata(hdl_dir) else { return false };
    let Ok(cpp_meta) = fs::metadata(cpp_source) else { return false };
    let (Ok(hdl_t), Ok(cpp_t)) = (hdl_meta.modified(), cpp_meta.modified()) else {
        return false;
    };
    hdl_t > cpp_t
}

fn list_verilog_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    if !dir.is_dir() {
        return Ok(out);
    }
    for ent in fs::read_dir(dir)? {
        let ent = ent?;
        let p = ent.path();
        if p.extension().and_then(|s| s.to_str()) == Some("v") {
            out.push(p);
        }
    }
    out.sort();
    Ok(out)
}
