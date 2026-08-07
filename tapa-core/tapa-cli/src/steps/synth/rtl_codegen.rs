//! Drive `tapa_codegen::generate_rtl` against the HLS-produced Verilog
//! and persist the resulting RTL tree under `<work_dir>/rtl/`, plus
//! custom-RTL port shells under `<work_dir>/template/`.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use tapa_codegen::generate_rtl;
use tapa_codegen::rtl_state::TopologyWithRtl;
use tapa_ir::{Design, FloorplanResult, SynthTarget};
use tapa_rtl::VerilogModule;

use crate::error::{CliError, Result};

use camino::Utf8PathBuf;

pub type TaskHdlInputs = BTreeMap<String, Vec<Utf8PathBuf>>;

/// Parse and attach every HLS task's exact top module without writing outputs.
///
/// Floorplanning needs the RTL interface shapes before it solves placement,
/// while code generation needs the same parsed modules afterwards. Keeping
/// preparation separate lets both phases share one authoritative parse.
pub fn prepare_rtl_state(design: &Design, hdl_inputs: &TaskHdlInputs) -> Result<TopologyWithRtl> {
    let mut state = TopologyWithRtl::new(design.clone());

    for (task_name, task) in &design.tasks {
        if task.synth != SynthTarget::Hls {
            continue;
        }
        let files = hdl_inputs.get(task_name).ok_or_else(|| {
            CliError::Codegen(format!(
                "missing HLS Verilog inputs for task `{task_name}`; run `synth` first",
            ))
        })?;
        let module_path = pick_top_verilog(files, task_name).ok_or_else(|| {
            CliError::Codegen(format!(
                "HLS Verilog inputs for task `{task_name}` do not contain the expected \
                 top module file `{task_name}.v`; run `synth` again",
            ))
        })?;
        let source = fs::read_to_string(&module_path)?;
        let parsed = VerilogModule::parse(&source).map_err(|e| {
            CliError::Codegen(format!(
                "failed to parse HLS Verilog `{}` for task `{task_name}`: {e}",
                module_path.as_str(),
            ))
        })?;
        if parsed.name != *task_name {
            return Err(CliError::Codegen(format!(
                "HLS top file `{}` declares module `{}` instead of the expected `{task_name}`",
                module_path, parsed.name,
            )));
        }
        state
            .attach_module(task_name, parsed)
            .map_err(|e| codegen_to_cli_error("attach", task_name, &e))?;
    }

    Ok(state)
}

/// Generate and persist RTL from an already-prepared topology.
///
/// Packaging is a copy operation: `generate_rtl` returns the complete
/// [`tapa_codegen::ArtifactManifest`] (generated RTL, FSM files,
/// `Ignore`-task template shells, and the embedded support assets), and
/// this function writes every manifest entry to its relative path.
pub fn emit_prepared_rtl_tree(
    work_dir: &Path,
    state: &mut TopologyWithRtl,
    hdl_inputs: &TaskHdlInputs,
) -> Result<Vec<PathBuf>> {
    let top = state.design.top.clone();
    let rtl_dir = work_dir.join("rtl");
    fs::create_dir_all(&rtl_dir)?;

    // The HLS outputs are user inputs to codegen, not artifacts it
    // returns; the pack tree still ships them, so copy them in verbatim.
    for files in hdl_inputs.values() {
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

    let manifest = generate_rtl(state).map_err(|e| codegen_to_cli_error("generate", &top, &e))?;

    let mut written = Vec::new();
    for (relative, content) in manifest.files() {
        let path = work_dir.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, content)?;
        written.push(path);
    }
    Ok(written)
}

pub fn generate_rtl_tree(
    work_dir: &Path,
    design: &Design,
    hdl_inputs: &TaskHdlInputs,
    floorplan: Option<&FloorplanResult>,
) -> Result<Vec<PathBuf>> {
    let mut state = prepare_rtl_state(design, hdl_inputs)?;
    state.floorplan = floorplan.cloned();
    emit_prepared_rtl_tree(work_dir, &mut state, hdl_inputs)
}

/// Reconstruct [`TaskHdlInputs`] from the on-disk HLS output, for a re-run of
/// codegen (e.g. the floorplan step) that no longer has the live HLS results.
///
/// Every HLS task — the top and every leaf, since synth runs HLS for all
/// non-`Ignore` tasks — has its Verilog and generator sidecars under
/// `<work_dir>/hls/<task>/verilog`.
pub fn collect_hdl_inputs(work_dir: &Path, design: &Design) -> Result<TaskHdlInputs> {
    let mut inputs = TaskHdlInputs::new();
    for (task_name, task) in &design.tasks {
        if task.synth != SynthTarget::Hls {
            continue;
        }
        let layout = super::hls_run::TaskHlsLayout::new(work_dir, task_name);
        let files = super::hls_run::list_hdl_files(&layout.hdl_dir)?;
        if pick_top_verilog(&files, task_name).is_none() {
            return Err(CliError::MissingState {
                name: format!("HLS top module `{task_name}.v` (run `synth` again)"),
                path: layout
                    .hdl_dir
                    .join(format!("{task_name}.v"))
                    .into_std_path_buf(),
            });
        }
        inputs.insert(task_name.clone(), files);
    }
    Ok(inputs)
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
    let expected = format!("{task_name}.v");
    files
        .iter()
        .find(|path| path.file_name() == Some(expected.as_str()))
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
    use tapa_ir::ClockPeriod;

    use tapa_ir::{SynthTarget, Task, TaskInstance, TaskLevel};

    fn write_stub_module(dir: &Path, task_name: &str) -> Utf8PathBuf {
        let path = dir.join(format!("{task_name}.v"));
        fs::write(
            &path,
            format!(
                "module {task_name}(\n  input wire ap_clk,\n  input wire ap_rst_n,\n  \
                 input wire ap_start,\n  output wire ap_done,\n  output wire ap_idle,\n  \
                 output wire ap_ready\n);\nendmodule\n"
            ),
        )
        .expect("write HLS module");
        Utf8PathBuf::from_path_buf(path).expect("UTF-8 path")
    }

    fn write_stub_inputs(dir: &Path, task_names: &[&str]) -> TaskHdlInputs {
        task_names
            .iter()
            .map(|task_name| {
                (
                    (*task_name).to_string(),
                    vec![write_stub_module(dir, task_name)],
                )
            })
            .collect()
    }

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
                self_area: None,
                total_area: None,
                clock_period: None,
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
                self_area: None,
                total_area: None,
                clock_period: Some(ClockPeriod::from_picoseconds(3330)),
            },
        );
        Design {
            schema_version: tapa_ir::graph::SCHEMA_VERSION,
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
        let hdl_inputs = write_stub_inputs(dir.path(), &["Add", "VecAdd"]);
        let written =
            generate_rtl_tree(dir.path(), &vadd_design(), &hdl_inputs, None).expect("generate");

        let rtl_dir = dir.path().join("rtl");
        let fifo = fs::read_to_string(rtl_dir.join("fifo.v")).expect("fifo.v");
        assert!(fifo.contains("module fifo"), "got:\n{fifo}");
        assert!(rtl_dir.join("fifo_srl.v").is_file());
        assert!(rtl_dir.join("fifo_bram.v").is_file());
        assert!(rtl_dir.join("fifo_fwd.v").is_file());
        assert!(rtl_dir.join("axis_adapter.v").is_file());
        let hs_pipeline =
            fs::read_to_string(rtl_dir.join("tapa_hs_pipeline.v")).expect("pipeline asset");
        assert!(hs_pipeline.contains("module tapa_hs_pipeline"));
        assert!(written.iter().any(|p| p.ends_with("fifo.v")));
        assert!(written
            .iter()
            .any(|path| path.ends_with("tapa_hs_pipeline.v")));
    }

    #[test]
    fn reconstructed_hdl_inputs_copy_generator_sidecars() {
        let dir = tempfile::tempdir().expect("tempdir");
        let work_dir = dir.path().join("work");
        for task_name in ["Add", "VecAdd"] {
            let hdl_dir = work_dir.join("hls").join(task_name).join("verilog");
            fs::create_dir_all(&hdl_dir).expect("create HLS output directory");
            write_stub_module(&hdl_dir, task_name);
        }
        let generator = work_dir.join("hls/Add/verilog/Add_sort_ip.tcl");
        fs::write(&generator, "create_ip -module_name Add_sort_ip\n")
            .expect("write generator sidecar");

        let inputs = collect_hdl_inputs(&work_dir, &vadd_design()).expect("collect HLS outputs");
        assert!(
            inputs["Add"].iter().any(|path| path == &generator),
            "reconstructed inputs must retain non-Verilog generator assets"
        );

        let candidate_dir = dir.path().join("candidate");
        generate_rtl_tree(&candidate_dir, &vadd_design(), &inputs, None)
            .expect("emit candidate RTL");
        assert_eq!(
            fs::read_to_string(candidate_dir.join("rtl/Add_sort_ip.tcl"))
                .expect("read copied generator sidecar"),
            "create_ip -module_name Add_sort_ip\n"
        );
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
                    stream_depth: None,
                    mmap_addr_width: None,
                }],
                tasks: BTreeMap::new(),
                fifos: BTreeMap::new(),
                readable_name: String::new(),
                synth: SynthTarget::Ignore,
                self_area: None,
                total_area: None,
                clock_period: None,
            },
        );
        let top = design.tasks.get_mut("VecAdd").expect("top task");
        top.tasks = BTreeMap::from_iter([(
            "Add_Upper".to_string(),
            vec![TaskInstance {
                name: None,
                args: BTreeMap::from([(
                    "n".to_string(),
                    tapa_ir::Arg::named("1".to_string(), ArgCategory::Scalar),
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
        let hdl_inputs = TaskHdlInputs::from_iter([
            (
                "Add".to_string(),
                vec![write_stub_module(dir.path(), "Add")],
            ),
            ("VecAdd".to_string(), vec![top_rtl]),
        ]);

        let written = generate_rtl_tree(dir.path(), &design, &hdl_inputs, None).expect("generate");
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
    fn prepare_requires_each_hls_tasks_exact_top_module() {
        let dir = tempfile::tempdir().expect("tempdir");
        let auxiliary = dir.path().join("Add.sv");
        fs::write(&auxiliary, "module Add; endmodule\n").expect("write wrong extension");
        let inputs = TaskHdlInputs::from_iter([
            (
                "Add".to_string(),
                vec![Utf8PathBuf::from_path_buf(auxiliary).expect("UTF-8 path")],
            ),
            (
                "VecAdd".to_string(),
                vec![write_stub_module(dir.path(), "VecAdd")],
            ),
        ]);

        let Err(error) = prepare_rtl_state(&vadd_design(), &inputs) else {
            panic!("a different extension cannot stand in for the exact task top file");
        };
        assert!(
            error.to_string().contains("Add.v"),
            "the diagnostic must name the expected top module: {error}",
        );
    }

    #[test]
    fn prepare_rejects_wrong_module_name_in_exact_top_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let add_path = dir.path().join("Add.v");
        fs::write(&add_path, "module Add_helper; endmodule\n").expect("write wrong module");
        let inputs = TaskHdlInputs::from_iter([
            (
                "Add".to_string(),
                vec![Utf8PathBuf::from_path_buf(add_path).expect("UTF-8 path")],
            ),
            (
                "VecAdd".to_string(),
                vec![write_stub_module(dir.path(), "VecAdd")],
            ),
        ]);

        let Err(error) = prepare_rtl_state(&vadd_design(), &inputs) else {
            panic!("the top file must declare the task's exact module name");
        };
        assert!(
            error.to_string().contains("Add_helper")
                && error.to_string().contains("expected `Add`"),
            "the diagnostic must identify both module names: {error}",
        );
    }

    #[test]
    fn prepared_state_is_reused_for_emission() {
        let dir = tempfile::tempdir().expect("tempdir");
        let inputs = write_stub_inputs(dir.path(), &["Add", "VecAdd"]);
        let mut state = prepare_rtl_state(&vadd_design(), &inputs).expect("prepare");
        assert_eq!(state.module_map.len(), 2, "both HLS modules are attached");

        emit_prepared_rtl_tree(dir.path(), &mut state, &inputs).expect("emit");
        assert!(dir.path().join("rtl/VecAdd.v").is_file());
        assert!(dir.path().join("rtl/VecAdd_fsm.v").is_file());
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
                    stream_depth: None,
                    mmap_addr_width: None,
                }],
                tasks: BTreeMap::new(),
                fifos: BTreeMap::new(),
                readable_name: String::new(),
                synth: SynthTarget::Ignore,
                self_area: None,
                total_area: None,
                clock_period: None,
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
