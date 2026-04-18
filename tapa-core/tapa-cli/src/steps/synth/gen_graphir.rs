//! `--gen-graphir` → `<work_dir>/graphir.json`.
//!
//! Wires `tapa_lowering::build_project_from_paths` against the current
//! design plus the `--device-config` / `--floorplan-path` inputs and
//! the `<work_dir>/rtl/` directory populated by the preceding RTL
//! codegen step. Matches Python
//! `tapa.graphir_conversion.gen_rs_graphir.get_project_from_floorplanned_program`.

use std::fs;
use std::path::Path;

use tapa_codegen::rtl_state::TopologyWithRtl;
use tapa_lowering::{build_project_from_paths, LoweringInputs};
use tapa_task_graph::Design;

use crate::error::{CliError, Result};
use crate::steps::synth::rtl_codegen::topology_program_from_design;

const OUTPUT_FILENAME: &str = "graphir.json";

/// Build the `GraphIR` Project from the current design + RTL artifacts and
/// persist it as `<work_dir>/graphir.json`.
pub fn emit_graphir(
    work_dir: &Path,
    design: &Design,
    device_config: &Path,
    floorplan_path: &Path,
) -> Result<()> {
    let program = topology_program_from_design(design)?;
    let mut state = TopologyWithRtl::new(program);

    let rtl_dir = work_dir.join("rtl");
    if !rtl_dir.is_dir() {
        return Err(CliError::InvalidArg(format!(
            "`--gen-graphir` requires `{}` from the preceding RTL codegen \
             step; run `tapa synth` (without --gen-graphir) first",
            rtl_dir.display(),
        )));
    }

    let inputs = LoweringInputs::new(&mut state, device_config, floorplan_path, &rtl_dir)
        .map_err(|e| {
            CliError::InvalidArg(format!(
                "failed to prepare graphir lowering inputs: {e}"
            ))
        })?;
    let project = build_project_from_paths(inputs).map_err(|e| {
        CliError::InvalidArg(format!("failed to build graphir project: {e}"))
    })?;

    let path = work_dir.join(OUTPUT_FILENAME);
    let bytes = serde_json::to_vec(&project)?;
    fs::write(&path, bytes)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use indexmap::IndexMap;
    use tapa_task_graph::TaskTopology;

    fn trivial_design() -> Design {
        let mut tasks = IndexMap::new();
        tasks.insert(
            "Top".to_string(),
            TaskTopology {
                name: "Top".to_string(),
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
        Design {
            top: "Top".to_string(),
            target: "xilinx-hls".to_string(),
            tasks,
            slot_task_name_to_fp_region: None,
        }
    }

    #[test]
    fn missing_rtl_dir_surfaces_invalid_arg() {
        let dir = tempfile::tempdir().expect("tempdir");
        let device = dir.path().join("device.json");
        fs::write(&device, r#"{"slots": []}"#).expect("write device");
        let floorplan = dir.path().join("floorplan.json");
        fs::write(&floorplan, "{}").expect("write floorplan");

        let err = emit_graphir(dir.path(), &trivial_design(), &device, &floorplan)
            .expect_err("must require rtl dir");
        assert!(matches!(err, CliError::InvalidArg(ref m) if m.contains("rtl")));
    }

    #[test]
    fn missing_ctrl_s_axi_surfaces_invalid_arg() {
        let dir = tempfile::tempdir().expect("tempdir");
        let rtl_dir = dir.path().join("rtl");
        fs::create_dir_all(&rtl_dir).expect("mkdir rtl");
        let device = dir.path().join("device.json");
        fs::write(&device, r#"{"slots": []}"#).expect("write device");
        let floorplan = dir.path().join("floorplan.json");
        fs::write(&floorplan, "{}").expect("write floorplan");

        let err = emit_graphir(dir.path(), &trivial_design(), &device, &floorplan)
            .expect_err("must require ctrl_s_axi");
        assert!(matches!(err, CliError::InvalidArg(_)));
    }
}
