//! Test-only fixture builders shared across the step test modules:
//! the mock floorplan publication marker, a JSON [`WorkState`] parser,
//! and a minimal [`CliContext`].

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
