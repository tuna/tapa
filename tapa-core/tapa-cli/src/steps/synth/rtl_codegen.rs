//! Drive `tapa_codegen::generate_rtl` against the HLS-produced Verilog
//! and persist the resulting RTL tree under `<work_dir>/rtl/`.
//!
//! Mirrors `tapa/codegen/program_rtl.py::generate_task_rtl` +
//! `generate_top_rtl` for the leaf-only vadd happy path.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;
use tapa_codegen::generate_rtl;
use tapa_codegen::rtl_state::TopologyWithRtl;
use tapa_rtl::VerilogModule;
use tapa_task_graph::Design;
use tapa_topology::program::Program;

use crate::error::{CliError, Result};

pub type TaskHdlInputs = BTreeMap<String, Vec<PathBuf>>;

/// Build a typed `tapa_topology::Program` from the JSON-flavored
/// `tapa_task_graph::Design` the CLI persists. Both schemas overlap on
/// the wire, so we round-trip through `serde_json::Value`.
pub fn topology_program_from_design(design: &Design) -> Result<Program> {
    let mut tasks = serde_json::Map::new();
    for (name, t) in &design.tasks {
        let mut task_obj = serde_json::Map::new();
        task_obj.insert("level".to_string(), Value::String(t.level.clone()));
        task_obj.insert("code".to_string(), Value::String(t.code.clone()));
        task_obj.insert(
            "target".to_string(),
            Value::String(t.target.clone().unwrap_or_else(|| "hls".to_string())),
        );
        task_obj.insert("is_slot".to_string(), Value::Bool(t.is_slot));
        task_obj.insert(
            "ports".to_string(),
            serde_json::to_value(&t.ports).map_err(CliError::Json)?,
        );
        let tasks_value = if t.tasks.is_empty() {
            Value::Object(serde_json::Map::new())
        } else {
            Value::Object(t.tasks.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        };
        task_obj.insert("tasks".to_string(), tasks_value);
        let fifos_value = if t.fifos.is_empty() {
            Value::Object(serde_json::Map::new())
        } else {
            Value::Object(t.fifos.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        };
        task_obj.insert("fifos".to_string(), fifos_value);
        tasks.insert(name.clone(), Value::Object(task_obj));
    }
    let program_value = Value::Object(
        [
            ("top".to_string(), Value::String(design.top.clone())),
            ("target".to_string(), Value::String(design.target.clone())),
            ("tasks".to_string(), Value::Object(tasks)),
        ]
        .into_iter()
        .collect(),
    );
    let program: Program = serde_json::from_value(program_value).map_err(CliError::Json)?;
    Ok(program)
}

pub fn generate_rtl_tree(
    work_dir: &Path,
    design: &Design,
    hdl_inputs: &TaskHdlInputs,
) -> Result<Vec<PathBuf>> {
    let rtl_dir = work_dir.join("rtl");
    fs::create_dir_all(&rtl_dir)?;

    let program = topology_program_from_design(design)?;
    let mut state = TopologyWithRtl::new(program);

    for (task_name, files) in hdl_inputs {
        let Some(module_path) = pick_top_verilog(files, task_name) else { continue };
        let source = fs::read_to_string(&module_path)?;
        let parsed = VerilogModule::parse(&source).map_err(|e| {
            CliError::InvalidArg(format!(
                "failed to parse HLS Verilog `{}` for task `{task_name}`: {e}",
                module_path.display(),
            ))
        })?;
        state
            .attach_module(task_name, parsed)
            .map_err(|e| codegen_to_cli_error("attach", task_name, &e))?;
        for src in files {
            let Some(name) = src.file_name() else { continue };
            let dest = rtl_dir.join(name);
            if dest == src.as_path() {
                continue;
            }
            fs::copy(src, &dest)?;
        }
    }

    generate_rtl(&mut state).map_err(|e| codegen_to_cli_error("generate", &design.top, &e))?;

    let mut written = Vec::new();
    for (name, content) in &state.generated_files {
        let path = rtl_dir.join(name);
        fs::write(&path, content)?;
        written.push(path);
    }
    Ok(written)
}

pub fn write_templates_info(work_dir: &Path, design: &Design) -> Result<()> {
    let templates: BTreeMap<String, Vec<String>> = design
        .tasks
        .iter()
        .filter(|(_, t)| t.target.as_deref() == Some("ignore"))
        .map(|(name, t)| {
            let port_strs: Vec<String> =
                t.ports.iter().map(|p| format!("{}: {}", p.name, p.ctype)).collect();
            (name.clone(), port_strs)
        })
        .collect();
    let path = work_dir.join("templates_info.json");
    let bytes = serde_json::to_vec(&templates)?;
    fs::write(&path, bytes)?;
    Ok(())
}

fn pick_top_verilog(files: &[PathBuf], task_name: &str) -> Option<PathBuf> {
    files
        .iter()
        .find(|p| p.file_stem().and_then(|s| s.to_str()) == Some(task_name))
        .cloned()
}

fn codegen_to_cli_error(op: &str, task: &str, err: &dyn std::fmt::Display) -> CliError {
    CliError::InvalidArg(format!(
        "tapa-codegen `{op}` failed for task `{task}`: {err}",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use indexmap::IndexMap;
    use serde_json::json;
    use tapa_task_graph::TaskTopology;

    fn vadd_design() -> Design {
        let mut tasks = IndexMap::new();
        tasks.insert(
            "Add".to_string(),
            TaskTopology {
                name: "Add".to_string(),
                level: "lower".to_string(),
                code: String::new(),
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
                code: String::new(),
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
        Design {
            top: "VecAdd".to_string(),
            target: "xilinx-hls".to_string(),
            tasks,
            slot_task_name_to_fp_region: None,
        }
    }

    #[test]
    fn topology_program_round_trips() {
        let design = vadd_design();
        let program = topology_program_from_design(&design).expect("convert");
        assert_eq!(program.top, "VecAdd");
        assert!(program.tasks.contains_key("Add"));
    }

    #[test]
    fn templates_info_empty_for_vadd() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_templates_info(dir.path(), &vadd_design()).expect("write");
        let raw = fs::read_to_string(dir.path().join("templates_info.json"))
            .expect("read");
        assert_eq!(raw, "{}");
    }
}
