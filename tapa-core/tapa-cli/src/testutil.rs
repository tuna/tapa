//! Test-only fixture builders shared across the step test modules:
//! synthed [`WorkState`] snapshots, the mock floorplan publication
//! marker, a minimal [`CliContext`], and the synthesized-HLS module
//! writer.

use std::collections::BTreeMap;
use std::path::Path;

use tapa_ir::{FloorplanResult, TaskGraph, WorkState};

use crate::context::CliContext;

/// An empty floorplan publication marker: its presence alone flips the
/// floorplan path, so regions and routes are irrelevant here.
pub fn mock_floorplan_result(device: &str, grid: (u32, u32)) -> FloorplanResult {
    FloorplanResult {
        device: device.to_string(),
        grid,
        regions: BTreeMap::new(),
        routes: Vec::new(),
        slot_usage: BTreeMap::new(),
    }
}

/// Parse a task-graph JSON fixture into a fresh [`WorkState`].
pub fn state_from_json(json: &str) -> WorkState {
    WorkState::new(TaskGraph::from_json(json).expect("parse fixture graph"))
}

/// A minimal context rooted at `work_dir`: no temp dir, no remote, no
/// verbosity.
pub fn ctx_at(work_dir: &Path) -> CliContext {
    CliContext {
        work_dir: work_dir.to_path_buf(),
        temp_dir: None,
        clang_format_quota_in_bytes: 0,
        remote_config: None,
        verbose: 0,
        quiet: 0,
    }
}

/// A minimal synthesized vadd state: top `VecAdd` with A -> fifo -> B.
pub fn synthed_vadd_state() -> WorkState {
    let mut state = state_from_json(
        r#"{
            "cflags": [], "top": "VecAdd", "target": "xilinx-vitis",
            "tasks": {
                "VecAdd": {
                    "readable_name": "VecAdd", "code": "void VecAdd() {}", "level": "upper", "synth": "hls",
                    "ports": [],
                    "tasks": {
                        "A": [{"args": {"out": {"arg": "fifo", "cat": "ostream"}}, "step": 0}],
                        "B": [{"args": {"in": {"arg": "fifo", "cat": "istream"}}, "step": 0}]
                    },
                    "fifos": {"fifo": {"depth": 2, "consumed_by": ["B", 0], "produced_by": ["A", 0]}}
                },
                "A": {"readable_name": "A", "code": "void A() {}", "level": "lower", "synth": "hls",
                    "ports": [{"cat": "ostream", "name": "out", "type": "float", "width": 32}],
                    "self_area": {"LUT": 100, "FF": 200}},
                "B": {"readable_name": "B", "code": "void B() {}", "level": "lower", "synth": "hls",
                    "ports": [{"cat": "istream", "name": "in", "type": "float", "width": 32}],
                    "self_area": {"LUT": 50, "FF": 60}}
            }
        }"#,
    );
    state.flow.part_num = Some("xcu280-fsvh2892-2L-e".to_string());
    state.flow.synthed = true;
    state
}

/// A synthesized direct-M-AXI state: top `Top` forwards `mem` to `Reader`.
pub fn synthed_direct_mmap_state() -> WorkState {
    let mut state = state_from_json(
        r#"{
            "cflags": [], "top": "Top", "target": "xilinx-vitis",
            "tasks": {
                "Top": {
                    "readable_name": "Top", "code": "", "level": "upper", "synth": "hls",
                    "ports": [{"cat":"mmap","name":"mem","type":"int*","width":32}],
                    "tasks": {"Reader": [{"args":{"data":{"arg":"mem","cat":"mmap"}}}]},
                    "fifos": {}
                },
                "Reader": {
                    "readable_name": "Reader", "code": "", "level": "lower", "synth": "hls",
                    "ports": [{"cat":"mmap","name":"data","type":"int*","width":32}],
                    "self_area": {"LUT":10,"FF":20}
                }
            }
        }"#,
    );
    state.flow.part_num = Some("xcu280-fsvh2892-2L-e".to_string());
    state.flow.platform = Some("xilinx_u280_gen3x16_xdma_1_202211_1".to_string());
    state.flow.synthed = true;
    state
}

/// Write a fake HLS Verilog module under `hls/<task>/verilog/<task>.v`.
pub fn write_hls_module(work_dir: &Path, task: &str, src: &str) {
    let dir = work_dir.join("hls").join(task).join("verilog");
    fs_err::create_dir_all(&dir).expect("hls dir");
    fs_err::write(dir.join(format!("{task}.v")), src).expect("hls verilog");
}
