//! `--nonpipeline-fifos` → `<work_dir>/grouping_constraints.json`.
//!
//! Ports `tapa.program_codegen.program.get_grouping_constraints` plus
//! the `_find_task_inst_hierarchy` walker from the same module.
//!
//! The input is a JSON list of `"<task>.<fifo>"` strings. For every
//! FIFO listed, we walk every path from the top task to its defining
//! task and emit a triple `[producer_path, fifo_path, consumer_path]`
//! matching the Python emit shape. Producer / consumer path resolution
//! mirrors `Program.get_inst_by_port_arg_name`.

use std::fs;
use std::path::Path;

use tapa_task_graph::Design;
use tapa_topology::program::Program;

use crate::error::{CliError, Result};
use crate::steps::synth::rtl_codegen::topology_program_from_design;

const OUTPUT_FILENAME: &str = "grouping_constraints.json";

/// Read the nonpipeline-FIFO list, compute grouping constraints from the
/// current design, and write the result to `<work_dir>/grouping_constraints.json`.
pub fn emit_grouping_constraints(
    work_dir: &Path,
    design: &Design,
    fifos_path: &Path,
) -> Result<()> {
    let raw = fs::read_to_string(fifos_path).map_err(|e| {
        CliError::InvalidArg(format!(
            "failed to read `--nonpipeline-fifos` file `{}`: {e}",
            fifos_path.display(),
        ))
    })?;
    let entries: Vec<String> = serde_json::from_str(&raw).map_err(|e| {
        CliError::InvalidArg(format!(
            "`{}` must be a JSON list of `<task>.<fifo>` strings: {e}",
            fifos_path.display(),
        ))
    })?;

    let program = topology_program_from_design(design)?;
    let constraints = compute_grouping_constraints(&program, &entries)?;

    let path = work_dir.join(OUTPUT_FILENAME);
    let bytes = serde_json::to_vec(&constraints)?;
    fs::write(&path, bytes)?;
    Ok(())
}

/// Pure helper: walk the program and produce the `[producer, fifo, consumer]`
/// constraint triples for every `<task>.<fifo>` entry in `entries`.
pub fn compute_grouping_constraints(
    program: &Program,
    entries: &[String],
) -> Result<Vec<Vec<String>>> {
    let mut out: Vec<Vec<String>> = Vec::new();
    for entry in entries {
        let (task_name, fifo_name) = entry.split_once('.').ok_or_else(|| {
            CliError::InvalidArg(format!(
                "`--nonpipeline-fifos` entry `{entry}` is not of the form \
                 `<task>.<fifo>`"
            ))
        })?;
        let task = program.tasks.get(task_name).ok_or_else(|| {
            CliError::InvalidArg(format!(
                "`--nonpipeline-fifos` entry references unknown task `{task_name}`"
            ))
        })?;
        let fifo = task.fifos.get(fifo_name).ok_or_else(|| {
            CliError::InvalidArg(format!(
                "task `{task_name}` has no FIFO `{fifo_name}` (referenced by \
                 `--nonpipeline-fifos`)"
            ))
        })?;
        let consumer_task = fifo
            .consumed_by
            .as_ref()
            .ok_or_else(|| {
                CliError::InvalidArg(format!(
                    "FIFO `{task_name}.{fifo_name}` has no consumer endpoint"
                ))
            })?
            .0
            .clone();
        let producer_task = fifo
            .produced_by
            .as_ref()
            .ok_or_else(|| {
                CliError::InvalidArg(format!(
                    "FIFO `{task_name}.{fifo_name}` has no producer endpoint"
                ))
            })?
            .0
            .clone();

        let producer_inst_name = find_instance_for_arg(task, &producer_task, fifo_name)
            .ok_or_else(|| {
                CliError::InvalidArg(format!(
                    "no instance of `{producer_task}` in `{task_name}` \
                     binds `{fifo_name}`"
                ))
            })?;
        let consumer_inst_name = find_instance_for_arg(task, &consumer_task, fifo_name)
            .ok_or_else(|| {
                CliError::InvalidArg(format!(
                    "no instance of `{consumer_task}` in `{task_name}` \
                     binds `{fifo_name}`"
                ))
            })?;

        let mut hierarchies: Vec<Vec<String>> = Vec::new();
        find_task_inst_hierarchy(
            program,
            task_name,
            &program.top,
            &program.top,
            &[],
            &mut hierarchies,
        );

        for hierarchy in hierarchies {
            let prefix = hierarchy.join("/");
            out.push(vec![
                format!("{prefix}/{producer_inst_name}"),
                format!("{prefix}/{fifo_name}"),
                format!("{prefix}/{consumer_inst_name}"),
            ]);
        }
    }
    Ok(out)
}

/// Find the `{child_task}_{idx}` instance within `parent` whose `args`
/// contain a binding to `arg_name` (mirrors Python
/// `Program.get_inst_by_port_arg_name`; `arg.name == arg_name`).
fn find_instance_for_arg(
    parent: &tapa_topology::task::TaskDesign,
    child_task: &str,
    arg_name: &str,
) -> Option<String> {
    let instances = parent.tasks.get(child_task)?;
    for (idx, inst) in instances.iter().enumerate() {
        if inst.args.values().any(|a| a.arg == arg_name) {
            return Some(format!("{child_task}_{idx}"));
        }
    }
    None
}

/// Replicate `tapa.program_codegen.program._find_task_inst_hierarchy`: yield
/// every path from the top task down to `target_task`, where each path is
/// `[top_inst, …, parent_inst]` (the parent hierarchy stops one level above
/// the target so callers can append the producer / consumer / fifo leaf).
fn find_task_inst_hierarchy(
    program: &Program,
    target_task: &str,
    current_task: &str,
    current_inst: &str,
    current_hierarchy: &[String],
    out: &mut Vec<Vec<String>>,
) {
    let mut new_hierarchy: Vec<String> = current_hierarchy.to_vec();
    new_hierarchy.push(current_inst.to_string());
    if current_task == target_task {
        out.push(new_hierarchy.clone());
    }
    let Some(task) = program.tasks.get(current_task) else {
        return;
    };
    for (child_task_name, instances) in &task.tasks {
        for (idx, _inst) in instances.iter().enumerate() {
            let inst_name = format!("{child_task_name}_{idx}");
            find_task_inst_hierarchy(
                program,
                target_task,
                child_task_name,
                &inst_name,
                &new_hierarchy,
                out,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn two_level_program() -> Program {
        serde_json::from_str(
            r#"{
                "top": "Top",
                "target": "xilinx-hls",
                "tasks": {
                    "Top": {
                        "level": "upper",
                        "code": "",
                        "target": "xilinx-hls",
                        "ports": [],
                        "tasks": {
                            "Producer": [{"args": {"out": {"arg": "q", "cat": "ostream"}}, "step": 0}],
                            "Consumer": [{"args": {"in": {"arg": "q", "cat": "istream"}}, "step": 0}]
                        },
                        "fifos": {
                            "q": {
                                "depth": 2,
                                "produced_by": ["Producer", 0],
                                "consumed_by": ["Consumer", 0]
                            }
                        }
                    },
                    "Producer": {"level": "lower", "code": "", "target": "xilinx-hls", "ports": [], "tasks": {}, "fifos": {}},
                    "Consumer": {"level": "lower", "code": "", "target": "xilinx-hls", "ports": [], "tasks": {}, "fifos": {}}
                }
            }"#,
        )
        .expect("parse program")
    }

    #[test]
    fn top_level_fifo_produces_single_triple() {
        let prog = two_level_program();
        let constraints =
            compute_grouping_constraints(&prog, &["Top.q".to_string()]).expect("compute");
        assert_eq!(constraints.len(), 1, "one hierarchy from Top → Top");
        assert_eq!(
            constraints[0],
            vec![
                "Top/Producer_0".to_string(),
                "Top/q".to_string(),
                "Top/Consumer_0".to_string(),
            ],
        );
    }

    #[test]
    fn unknown_task_surfaces_invalid_arg() {
        let prog = two_level_program();
        let err = compute_grouping_constraints(&prog, &["Ghost.q".to_string()])
            .expect_err("must reject unknown task");
        assert!(matches!(err, CliError::InvalidArg(ref m) if m.contains("Ghost")));
    }

    #[test]
    fn malformed_entry_surfaces_invalid_arg() {
        let prog = two_level_program();
        let err = compute_grouping_constraints(&prog, &["no-dot".to_string()])
            .expect_err("must reject missing `.`");
        assert!(matches!(err, CliError::InvalidArg(ref m) if m.contains("<task>.<fifo>")));
    }
}
