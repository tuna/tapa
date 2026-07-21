//! Drive `tapa_codegen::generate_rtl` against the HLS-produced Verilog
//! and persist the resulting RTL tree under `<work_dir>/rtl/`, plus
//! custom-RTL port shells under `<work_dir>/template/`.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use tapa_codegen::rtl_state::TopologyWithRtl;
use tapa_codegen::{generate_rtl, support_assets::VerilogAssets};
use tapa_ir::{Design, SynthTarget};
use tapa_rtl::VerilogModule;

use crate::error::{CliError, Result};

use camino::Utf8PathBuf;

pub type TaskHdlInputs = BTreeMap<String, Vec<Utf8PathBuf>>;

pub fn generate_rtl_tree(
    work_dir: &Path,
    design: &Design,
    hdl_inputs: &TaskHdlInputs,
) -> Result<Vec<PathBuf>> {
    let rtl_dir = work_dir.join("rtl");
    fs::create_dir_all(&rtl_dir)?;
    let mut written = write_verilog_support_assets(&rtl_dir)?;

    let mut state = TopologyWithRtl::new(design.clone());

    for (task_name, files) in hdl_inputs {
        let Some(module_path) = pick_top_verilog(files, task_name) else {
            continue;
        };
        let source = fs::read_to_string(&module_path)?;
        let parsed = VerilogModule::parse(&source).map_err(|e| {
            CliError::Codegen(format!(
                "failed to parse HLS Verilog `{}` for task `{task_name}`: {e}",
                module_path.as_str(),
            ))
        })?;
        state
            .attach_module(task_name, parsed)
            .map_err(|e| codegen_to_cli_error("attach", task_name, &e))?;
        for src in files {
            let Some(name) = src.file_name() else {
                continue;
            };
            let dest = rtl_dir.join(name);
            if dest == src.as_path() {
                continue;
            }
            fs::copy(src, &dest)?;
        }
    }

    generate_rtl(&mut state).map_err(|e| codegen_to_cli_error("generate", &design.top, &e))?;

    for (name, content) in &state.generated_files {
        let path = rtl_dir.join(name);
        fs::write(&path, content)?;
        written.push(path);
    }
    if !state.template_files.is_empty() {
        let template_dir = work_dir.join("template");
        fs::create_dir_all(&template_dir)?;
        for (name, content) in &state.template_files {
            let path = template_dir.join(name);
            fs::write(&path, content)?;
            written.push(path);
        }
    }
    Ok(written)
}

fn write_verilog_support_assets(rtl_dir: &Path) -> Result<Vec<PathBuf>> {
    let mut written = Vec::new();
    for name in VerilogAssets::iter() {
        let content = VerilogAssets::get(&name).expect("iterated asset exists");
        let path = rtl_dir.join(name.as_ref());
        fs::write(&path, &content.data)?;
        written.push(path);
    }
    Ok(written)
}

/// Write the typed port list of each `synth == ignore` task to
/// `<work_dir>/templates_info.json`. `--custom-rtl` at pack time keys
/// off the task names; the values are the same `tapa_ir::Port` schema
/// the rest of the pipeline speaks.
pub fn write_templates_info(work_dir: &Path, design: &Design) -> Result<()> {
    let templates: BTreeMap<&String, &Vec<tapa_ir::Port>> = design
        .tasks
        .iter()
        .filter(|(_, t)| t.synth == SynthTarget::Ignore)
        .map(|(name, t)| (name, &t.ports))
        .collect();
    let bytes = serde_json::to_vec(&templates)?;
    crate::state::json::write_bytes_atomic(work_dir, "templates_info.json", &bytes)?;
    Ok(())
}

fn pick_top_verilog(files: &[Utf8PathBuf], task_name: &str) -> Option<Utf8PathBuf> {
    files
        .iter()
        .find(|p| p.file_stem() == Some(task_name))
        .cloned()
}

fn codegen_to_cli_error(op: &str, task: &str, err: &dyn std::fmt::Display) -> CliError {
    CliError::Codegen(format!(
        "tapa-codegen `{op}` failed for task `{task}`: {err}",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    use indexmap::IndexMap;
    use tapa_ir::{SynthTarget, Task, TaskInstance, TaskLevel};

    fn vadd_design() -> Design {
        let mut tasks = BTreeMap::new();
        tasks.insert(
            "Add".to_string(),
            Task {
                level: TaskLevel::Lower,
                code: String::new(),
                ports: Vec::new(),
                tasks: BTreeMap::new(),
                fifos: BTreeMap::new(),
                readable_name: String::new(),
                synth: SynthTarget::Hls,
                self_area: IndexMap::new(),
                total_area: IndexMap::new(),
                clock_period: "0".to_string(),
            },
        );
        let mut child_tasks = BTreeMap::new();
        child_tasks.insert(
            "Add".to_string(),
            vec![TaskInstance {
                name: None,
                args: BTreeMap::new(),
                step: 0,
            }],
        );
        tasks.insert(
            "VecAdd".to_string(),
            Task {
                level: TaskLevel::Upper,
                code: String::new(),
                ports: Vec::new(),
                tasks: child_tasks,
                fifos: BTreeMap::new(),
                readable_name: String::new(),
                synth: SynthTarget::Hls,
                self_area: IndexMap::new(),
                total_area: IndexMap::new(),
                clock_period: "3.33".to_string(),
            },
        );
        Design {
            top: "VecAdd".to_string(),
            target: tapa_ir::Target::XilinxHls,
            tasks,
            cflags: Vec::new(),
        }
    }

    #[test]
    fn templates_info_empty_for_vadd() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_templates_info(dir.path(), &vadd_design()).expect("write");
        let raw = fs::read_to_string(dir.path().join("templates_info.json")).expect("read");
        assert_eq!(raw, "{}");
    }

    #[test]
    fn generate_rtl_tree_copies_verilog_support_assets() {
        let dir = tempfile::tempdir().expect("tempdir");
        let written =
            generate_rtl_tree(dir.path(), &vadd_design(), &TaskHdlInputs::new()).expect("generate");

        let rtl_dir = dir.path().join("rtl");
        let fifo = fs::read_to_string(rtl_dir.join("fifo.v")).expect("fifo.v");
        assert!(fifo.contains("module fifo"), "got:\n{fifo}");
        assert!(rtl_dir.join("fifo_srl.v").is_file());
        assert!(rtl_dir.join("fifo_bram.v").is_file());
        assert!(rtl_dir.join("fifo_fwd.v").is_file());
        assert!(rtl_dir.join("axis_adapter.v").is_file());
        assert!(written.iter().any(|p| p.ends_with("fifo.v")));
    }

    #[test]
    fn generate_rtl_tree_writes_ignored_task_template_and_placeholder() {
        use tapa_ir::{ArgCategory, Port};

        let mut design = vadd_design();
        design.tasks.insert(
            "Add_Upper".to_string(),
            Task {
                level: TaskLevel::Lower,
                code: String::new(),
                ports: vec![Port {
                    cat: ArgCategory::Scalar,
                    name: "n".to_string(),
                    ctype: "uint64_t".to_string(),
                    width: 64,
                    chan_count: None,
                    chan_size: None,
                }],
                tasks: BTreeMap::new(),
                fifos: BTreeMap::new(),
                readable_name: String::new(),
                synth: SynthTarget::Ignore,
                self_area: IndexMap::new(),
                total_area: IndexMap::new(),
                clock_period: "0".to_string(),
            },
        );
        let top = design.tasks.get_mut("VecAdd").expect("top task");
        top.tasks = BTreeMap::from_iter([(
            "Add_Upper".to_string(),
            vec![TaskInstance {
                name: None,
                args: BTreeMap::from([(
                    "n".to_string(),
                    tapa_ir::Arg {
                        arg: "1".to_string(),
                        cat: ArgCategory::Scalar,
                    },
                )]),
                step: 0,
            }],
        )]);

        let dir = tempfile::tempdir().expect("tempdir");
        let top_rtl = dir.path().join("VecAdd.v");
        fs::write(
            &top_rtl,
            "module VecAdd(\n\
             input wire ap_clk, input wire ap_rst_n, input wire ap_start,\n\
             output wire ap_done, output wire ap_idle, output wire ap_ready\n\
             ); endmodule\n",
        )
        .expect("write top RTL");
        let top_rtl = Utf8PathBuf::from_path_buf(top_rtl).expect("UTF-8 path");
        let hdl_inputs = TaskHdlInputs::from_iter([("VecAdd".to_string(), vec![top_rtl])]);

        let written = generate_rtl_tree(dir.path(), &design, &hdl_inputs).expect("generate");
        let template_path = dir.path().join("template/Add_Upper.v");
        let placeholder_path = dir.path().join("rtl/Add_Upper.v");
        let template = fs::read_to_string(&template_path).expect("author template");
        let placeholder = fs::read_to_string(&placeholder_path).expect("package placeholder");
        let top = fs::read_to_string(dir.path().join("rtl/VecAdd.v")).expect("generated top");

        assert!(template.contains("module Add_Upper"), "got:\n{template}");
        assert!(template.contains("input wire [63:0] n"), "got:\n{template}");
        assert_eq!(placeholder, template);
        assert!(
            top.contains("Add_Upper Add_Upper_0"),
            "top should instantiate the ignored task:\n{top}",
        );
        assert!(
            written.contains(&template_path) && written.contains(&placeholder_path),
            "template and its package placeholder must both be reported: {written:?}",
        );
        assert!(
            !dir.path().join("rtl/Add_Upper_template.v").exists(),
            "the authoring copy belongs under work/template, not rtl/",
        );
    }

    #[test]
    fn templates_info_emits_typed_ports_for_ignore_tasks() {
        use tapa_ir::{ArgCategory, Port};
        let mut design = vadd_design();
        // Drop a `target(\"ignore\")` task that carries a port so the
        // writer folds it into the emitted schema.
        design.tasks.insert(
            "Stub".to_string(),
            Task {
                level: TaskLevel::Lower,
                code: String::new(),
                ports: vec![Port {
                    cat: ArgCategory::Scalar,
                    name: "n".to_string(),
                    ctype: "uint64_t".to_string(),
                    width: 64,
                    chan_count: None,
                    chan_size: None,
                }],
                tasks: BTreeMap::new(),
                fifos: BTreeMap::new(),
                readable_name: String::new(),
                synth: SynthTarget::Ignore,
                self_area: IndexMap::new(),
                total_area: IndexMap::new(),
                clock_period: "0".to_string(),
            },
        );
        let dir = tempfile::tempdir().expect("tempdir");
        write_templates_info(dir.path(), &design).expect("write");
        let raw = fs::read_to_string(dir.path().join("templates_info.json")).expect("read");
        let parsed: serde_json::Value = serde_json::from_str(&raw).expect("parse");
        assert_eq!(parsed["Stub"][0]["cat"], "scalar");
        assert_eq!(parsed["Stub"][0]["name"], "n");
        assert_eq!(parsed["Stub"][0]["type"], "uint64_t");
        assert_eq!(parsed["Stub"][0]["width"], 64);
    }
}
