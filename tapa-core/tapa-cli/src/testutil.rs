//! Test-only fixture builders shared across the step test modules:
//! the mock floorplan publication marker, a JSON [`WorkState`] parser,
//! minimal typed task/port builders, and a minimal [`CliContext`].

use std::collections::BTreeMap;
use std::path::Path;

use tapa_ir::{
    ArgCategory, FloorplanResult, Port, SynthTarget, Task, TaskGraph, TaskLevel, WorkState,
};

use crate::context::CliContext;

/// A minimal lower-level HLS task: `{name}.cpp` as its only source, no
/// ports or children. Call sites override fields with struct-update
/// syntax.
pub fn task(name: &str) -> Task {
    Task {
        level: TaskLevel::Lower,
        srcs: vec![format!("{name}.cpp")],
        include_dirs: Vec::new(),
        defines: Vec::new(),
        pl_kernel_defines: Vec::new(),
        ports: Vec::new(),
        tasks: BTreeMap::new(),
        fifos: BTreeMap::new(),
        readable_name: String::new(),
        aie_location: Vec::new(),
        aie_stack_bytes: None,
        synth: SynthTarget::Hls,
        self_area: None,
        total_area: None,
        clock_period: None,
    }
}

/// A port with no stream, AIE, or mmap metadata; only the identity
/// fields vary across the test fixtures.
pub fn port(name: &str, cat: ArgCategory, ctype: &str, width: u32) -> Port {
    Port {
        cat,
        name: name.to_string(),
        ctype: ctype.to_string(),
        width,
        chan_count: None,
        chan_size: None,
        frame: None,
        aie_plio: None,
        stream_depth: None,
        mmap_addr_width: None,
        aie_movers: None,
        ring: None,
    }
}

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

/// A minimal context rooted at `work_dir`: no remote runner.
pub fn ctx_at(work_dir: &Path) -> CliContext {
    CliContext {
        work_dir: work_dir.to_path_buf(),
        remote_config: None,
    }
}
