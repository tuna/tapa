//! Lowering input validation and bundling.
//!
//! Mirrors Python's `get_project_from_floorplanned_program(device_config,
//! floorplan_path, ...)` boundary: the lowering pass consumes paths, reads
//! `device_config.json`, `floorplan.json`, the `{top}_control_s_axi.v` file,
//! and the leaf RTL sources itself rather than having the caller pre-extract
//! every input.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use tapa_codegen::rtl_state::TopologyWithRtl;

use crate::LoweringError;

/// All inputs needed for the lowering pass.
///
/// Bundles the RTL-bearing topology state together with validated
/// filesystem paths for device config, floorplan, and RTL directory.
///
/// The state is borrowed mutably because `build_project_from_paths` may need
/// to parse additional leaf RTL from `rtl_dir` and attach it to the
/// `TopologyWithRtl` before lowering proceeds.
pub struct LoweringInputs<'a> {
    /// RTL-bearing topology state (program + parsed Verilog modules + FSM modules).
    pub state: &'a mut TopologyWithRtl,
    /// Path to `device_config.json`.
    pub device_config: PathBuf,
    /// Path to `floorplan.json`.
    pub floorplan: PathBuf,
    /// Path to RTL directory containing leaf `.v` files and the
    /// `{top}_control_s_axi.v` file.
    pub rtl_dir: PathBuf,
}

impl<'a> LoweringInputs<'a> {
    /// Create and validate lowering inputs.
    ///
    /// # Errors
    ///
    /// Returns an error if `rtl_dir` does not exist.
    pub fn new(
        state: &'a mut TopologyWithRtl,
        device_config: impl AsRef<Path>,
        floorplan: impl AsRef<Path>,
        rtl_dir: impl AsRef<Path>,
    ) -> Result<Self, LoweringError> {
        let rtl_dir = rtl_dir.as_ref().to_path_buf();
        if !rtl_dir.exists() {
            return Err(LoweringError::PathNotFound(
                rtl_dir.display().to_string(),
            ));
        }
        Ok(Self {
            state,
            device_config: device_config.as_ref().to_path_buf(),
            floorplan: floorplan.as_ref().to_path_buf(),
            rtl_dir,
        })
    }

    /// Read a leaf RTL file from the RTL directory.
    ///
    /// # Errors
    ///
    /// Returns `LoweringError::MissingLeafRtl` if the file is absent.
    pub fn read_leaf_rtl(&self, task_name: &str) -> Result<String, LoweringError> {
        let path = self.rtl_dir.join(format!("{task_name}.v"));
        std::fs::read_to_string(&path).map_err(|_| {
            LoweringError::MissingLeafRtl(path.display().to_string())
        })
    }

    /// Read the `{top}_control_s_axi.v` RTL source.
    ///
    /// # Errors
    ///
    /// Returns `LoweringError::MissingCtrlSAxi` if the file is absent.
    pub fn read_ctrl_s_axi(&self) -> Result<String, LoweringError> {
        let top = &self.state.program.top;
        let path = self.rtl_dir.join(format!("{top}_control_s_axi.v"));
        std::fs::read_to_string(&path).map_err(|_| {
            LoweringError::MissingCtrlSAxi(path.display().to_string())
        })
    }

    /// Access the topology program.
    #[must_use]
    pub fn program(&self) -> &tapa_topology::program::Program {
        &self.state.program
    }

    /// Read and parse `floorplan.json` into slot → instance mapping.
    ///
    /// `floorplan.json` is `{ "inst_name": "SLOT_X0Y0:SLOT_X0Y0", ... }`.
    /// The returned slot name replaces `:` with `_`, matching the slot
    /// module names used by the rest of the lowering pass.
    ///
    /// # Errors
    ///
    /// Returns an error if `floorplan.json` cannot be read or parsed.
    pub fn read_slot_to_instances(&self) -> Result<BTreeMap<String, Vec<String>>, LoweringError> {
        let text = std::fs::read_to_string(&self.floorplan).map_err(|_| {
            LoweringError::PathNotFound(self.floorplan.display().to_string())
        })?;
        let vertex_to_region: BTreeMap<String, String> =
            serde_json::from_str(&text)?;
        let mut slot_to_insts: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (vertex, region) in vertex_to_region {
            let slot_name = region.replace(':', "_");
            slot_to_insts.entry(slot_name).or_default().push(vertex);
        }
        for insts in slot_to_insts.values_mut() {
            insts.sort();
        }
        Ok(slot_to_insts)
    }

    /// Read and parse `device_config.json` + `floorplan.json` into island →
    /// pblock range mapping. Mirrors Python's `get_island_to_pblock_range`.
    ///
    /// # Errors
    ///
    /// Returns an error if either file cannot be read or parsed.
    pub fn read_island_to_pblock_range(
        &self,
    ) -> Result<BTreeMap<String, Vec<String>>, LoweringError> {
        let device_text = std::fs::read_to_string(&self.device_config).map_err(|_| {
            LoweringError::PathNotFound(self.device_config.display().to_string())
        })?;
        let floorplan_text = std::fs::read_to_string(&self.floorplan).map_err(|_| {
            LoweringError::PathNotFound(self.floorplan.display().to_string())
        })?;
        let device: DeviceConfig = serde_json::from_str(&device_text)?;
        let floorplan: BTreeMap<String, String> = serde_json::from_str(&floorplan_text)?;
        let used_slots: std::collections::HashSet<String> = floorplan.into_values().collect();

        let mut out = BTreeMap::new();
        for slot in device.slots {
            let canonical = format!("SLOT_X{x}Y{y}:SLOT_X{x}Y{y}", x = slot.x, y = slot.y);
            if !used_slots.contains(&canonical) {
                continue;
            }
            let key = canonical.replace(':', "_TO_");
            out.insert(key, slot.pblock_ranges);
        }
        Ok(out)
    }

    /// Extract the FPGA `part_num` from `device_config.json` (optional).
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be read or parsed.
    pub fn read_part_num(&self) -> Result<Option<String>, LoweringError> {
        let text = std::fs::read_to_string(&self.device_config).map_err(|_| {
            LoweringError::PathNotFound(self.device_config.display().to_string())
        })?;
        let cfg: DeviceConfig = serde_json::from_str(&text)?;
        Ok(cfg.part_num)
    }
}

#[derive(Debug, Deserialize)]
struct DeviceConfig {
    #[serde(default)]
    slots: Vec<DeviceSlot>,
    #[serde(default)]
    part_num: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DeviceSlot {
    x: u32,
    y: u32,
    #[serde(default)]
    pblock_ranges: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_state() -> TopologyWithRtl {
        let program: tapa_topology::program::Program = serde_json::from_str(
            r#"{
                "top": "top", "target": "xilinx-hls",
                "tasks": {"top": {"level": "upper", "code": "", "target": "xilinx-hls", "ports": [], "tasks": {}, "fifos": {}}}
            }"#,
        )
        .unwrap();
        TopologyWithRtl::new(program)
    }

    #[test]
    fn lowering_inputs_rejects_nonexistent_dir() {
        let mut state = make_state();
        let result = LoweringInputs::new(
            &mut state,
            "/tmp/device.json",
            "/tmp/floorplan.json",
            "/nonexistent/rtl/dir",
        );
        match result {
            Err(err) => assert!(err.to_string().contains("not found"), "got: {err}"),
            Ok(_) => panic!("should reject non-existent rtl_dir"),
        }
    }

    #[test]
    fn lowering_inputs_carries_state() {
        let mut state = make_state();
        let inputs = LoweringInputs::new(
            &mut state,
            "/tmp/device.json",
            "/tmp/floorplan.json",
            "/tmp",
        )
        .expect("should accept existing dir");
        assert_eq!(inputs.program().top, "top");
    }

    #[test]
    fn read_leaf_rtl_missing_raises() {
        let mut state = make_state();
        let tmp = tempfile::tempdir().unwrap();
        let inputs = LoweringInputs::new(
            &mut state,
            "/tmp/device.json",
            "/tmp/floorplan.json",
            tmp.path(),
        )
        .unwrap();
        match inputs.read_leaf_rtl("nonexistent_leaf") {
            Err(LoweringError::MissingLeafRtl(msg)) => {
                assert!(msg.contains("nonexistent_leaf.v"), "got: {msg}");
            }
            other => panic!("expected MissingLeafRtl, got {other:?}"),
        }
    }

    #[test]
    fn read_ctrl_s_axi_missing_raises() {
        let mut state = make_state();
        let tmp = tempfile::tempdir().unwrap();
        let inputs = LoweringInputs::new(
            &mut state,
            "/tmp/device.json",
            "/tmp/floorplan.json",
            tmp.path(),
        )
        .unwrap();
        match inputs.read_ctrl_s_axi() {
            Err(LoweringError::MissingCtrlSAxi(msg)) => {
                assert!(msg.contains("top_control_s_axi.v"), "got: {msg}");
            }
            other => panic!("expected MissingCtrlSAxi, got {other:?}"),
        }
    }

    #[test]
    fn read_slot_to_instances_parses_floorplan() {
        let mut state = make_state();
        let tmp = tempfile::tempdir().unwrap();
        let floorplan = tmp.path().join("floorplan.json");
        std::fs::write(
            &floorplan,
            r#"{"child_0": "SLOT_X0Y0:SLOT_X0Y0", "child_1": "SLOT_X1Y0:SLOT_X1Y0"}"#,
        )
        .unwrap();
        let inputs = LoweringInputs::new(
            &mut state,
            tmp.path().join("device.json"),
            &floorplan,
            tmp.path(),
        )
        .unwrap();
        let mapping = inputs.read_slot_to_instances().unwrap();
        assert_eq!(mapping["SLOT_X0Y0_SLOT_X0Y0"], vec!["child_0".to_owned()]);
        assert_eq!(mapping["SLOT_X1Y0_SLOT_X1Y0"], vec!["child_1".to_owned()]);
    }

    #[test]
    fn read_pblock_ranges_parses_device_and_floorplan() {
        let mut state = make_state();
        let tmp = tempfile::tempdir().unwrap();
        let floorplan = tmp.path().join("floorplan.json");
        std::fs::write(
            &floorplan,
            r#"{"child_0": "SLOT_X0Y0:SLOT_X0Y0"}"#,
        )
        .unwrap();
        let device = tmp.path().join("device.json");
        std::fs::write(
            &device,
            r#"{
                "part_num": "xc_part",
                "slots": [
                    {"x": 0, "y": 0, "pblock_ranges": ["-add CLOCKREGION_X0Y0"]},
                    {"x": 1, "y": 0, "pblock_ranges": ["-add CLOCKREGION_X1Y0"]}
                ]
            }"#,
        )
        .unwrap();
        let inputs = LoweringInputs::new(&mut state, &device, &floorplan, tmp.path()).unwrap();
        let pblock = inputs.read_island_to_pblock_range().unwrap();
        assert_eq!(
            pblock["SLOT_X0Y0_TO_SLOT_X0Y0"],
            vec!["-add CLOCKREGION_X0Y0".to_owned()]
        );
        assert!(
            !pblock.contains_key("SLOT_X1Y0_TO_SLOT_X1Y0"),
            "unused slots must be dropped"
        );
        assert_eq!(inputs.read_part_num().unwrap().as_deref(), Some("xc_part"));
    }

    #[test]
    fn read_device_config_missing_raises() {
        let mut state = make_state();
        let tmp = tempfile::tempdir().unwrap();
        let floorplan = tmp.path().join("floorplan.json");
        std::fs::write(&floorplan, "{}").unwrap();
        let device = tmp.path().join("missing_device.json");
        let inputs = LoweringInputs::new(&mut state, &device, &floorplan, tmp.path()).unwrap();
        let err = inputs
            .read_island_to_pblock_range()
            .expect_err("missing device_config must error");
        assert!(err.to_string().contains("missing_device.json"), "got: {err}");
    }

    #[test]
    fn read_device_config_malformed_raises() {
        let mut state = make_state();
        let tmp = tempfile::tempdir().unwrap();
        let floorplan = tmp.path().join("floorplan.json");
        std::fs::write(&floorplan, "{}").unwrap();
        let device = tmp.path().join("device.json");
        std::fs::write(&device, "not valid json").unwrap();
        let inputs = LoweringInputs::new(&mut state, &device, &floorplan, tmp.path()).unwrap();
        let err = inputs
            .read_island_to_pblock_range()
            .expect_err("malformed device_config must error");
        assert!(
            matches!(err, LoweringError::Json(_)),
            "expected Json error, got: {err:?}"
        );
    }
}
