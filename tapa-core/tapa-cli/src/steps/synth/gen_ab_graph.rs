//! `--gen-ab-graph` → `<work_dir>/ab_graph.json`.
//!
//! Wires `tapa_floorplan::gen_abgraph::get_top_level_ab_graph` from a
//! `--floorplan-config` JSON file (whose `cpp_arg_pre_assignments` map
//! seeds port region preassignments) against the design's topology.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde_json::Value;
use tapa_floorplan::gen_abgraph::get_top_level_ab_graph;
use tapa_task_graph::Design;

use crate::error::{CliError, Result};
use crate::steps::synth::rtl_codegen::topology_program_from_design;

const OUTPUT_FILENAME: &str = "ab_graph.json";

/// Build the top-level `AutoBridge` graph and persist it as
/// `<work_dir>/ab_graph.json`.
///
/// Python parity: `tapa.abgraph.gen_abgraph.get_top_level_ab_graph`.
pub fn emit_ab_graph(
    work_dir: &Path,
    design: &Design,
    floorplan_config: &Path,
) -> Result<()> {
    let program = topology_program_from_design(design)?;
    let preassignments = read_cpp_arg_pre_assignments(floorplan_config)?;
    let fsm_name = format!("{}_fsm", program.top);

    let graph = get_top_level_ab_graph(&program, &preassignments, &fsm_name)
        .map_err(|e| CliError::InvalidArg(format!("failed to build ab_graph: {e}")))?;

    let path = work_dir.join(OUTPUT_FILENAME);
    let bytes = serde_json::to_vec(&graph)?;
    fs::write(&path, bytes)?;
    Ok(())
}

/// Read `cpp_arg_pre_assignments` from the floorplan config. Missing or
/// non-object values yield an empty map (matching Python's `.get(...) or {}`
/// semantics — the key is optional on early-stage floorplan configs).
fn read_cpp_arg_pre_assignments(
    floorplan_config: &Path,
) -> Result<BTreeMap<String, String>> {
    let raw = fs::read_to_string(floorplan_config).map_err(|e| {
        CliError::InvalidArg(format!(
            "failed to read `--floorplan-config` file `{}`: {e}",
            floorplan_config.display(),
        ))
    })?;
    let cfg: Value = serde_json::from_str(&raw).map_err(|e| {
        CliError::InvalidArg(format!(
            "`{}` is not valid JSON: {e}",
            floorplan_config.display(),
        ))
    })?;
    let Some(obj) = cfg.get("cpp_arg_pre_assignments").and_then(Value::as_object) else {
        return Ok(BTreeMap::new());
    };
    let mut out = BTreeMap::new();
    for (k, v) in obj {
        let Some(s) = v.as_str() else {
            return Err(CliError::InvalidArg(format!(
                "`cpp_arg_pre_assignments[{k}]` must be a string in `{}`",
                floorplan_config.display(),
            )));
        };
        out.insert(k.clone(), s.to_string());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use indexmap::IndexMap;
    use tapa_task_graph::TaskTopology;

    fn minimal_design() -> Design {
        let mut tasks = IndexMap::new();
        tasks.insert(
            "Top".to_string(),
            TaskTopology {
                name: "Top".to_string(),
                level: "upper".to_string(),
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
    fn emits_ab_graph_json_for_trivial_design() {
        let dir = tempfile::tempdir().expect("tempdir");
        let fp_cfg = dir.path().join("fp.json");
        fs::write(&fp_cfg, r#"{"cpp_arg_pre_assignments": {}}"#).expect("write fp");
        emit_ab_graph(dir.path(), &minimal_design(), &fp_cfg)
            .expect("emit must succeed on trivial design");
        let out = dir.path().join("ab_graph.json");
        assert!(out.is_file(), "ab_graph.json must be written");
        let raw = fs::read_to_string(&out).expect("read");
        let value: Value = serde_json::from_str(&raw).expect("parse");
        assert!(value.get("vs").is_some(), "payload must include vs");
        assert!(value.get("es").is_some(), "payload must include es");
    }

    #[test]
    fn missing_cpp_arg_pre_assignments_is_not_fatal() {
        let dir = tempfile::tempdir().expect("tempdir");
        let fp_cfg = dir.path().join("fp.json");
        fs::write(&fp_cfg, "{}").expect("write");
        emit_ab_graph(dir.path(), &minimal_design(), &fp_cfg)
            .expect("missing `cpp_arg_pre_assignments` must be tolerated");
    }

    #[test]
    fn malformed_floorplan_config_surfaces_invalid_arg() {
        let dir = tempfile::tempdir().expect("tempdir");
        let fp_cfg = dir.path().join("fp.json");
        fs::write(&fp_cfg, "not json").expect("write");
        let err = emit_ab_graph(dir.path(), &minimal_design(), &fp_cfg)
            .expect_err("malformed JSON must error");
        assert!(matches!(err, CliError::InvalidArg(_)));
    }
}
