//! Test fixtures shared by the synth step's test modules: the
//! rewritten-tree ownership manifest writer, the source-staging
//! shortcut, and the two-task `VecAdd` design. The typed task/port
//! builders live in [`crate::testutil`].

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use tapa_ir::{ClockPeriod, Design, Task, TaskInstance, TaskLevel};

use crate::testutil::task;

use super::hls_sources::{
    stage_hls_sources, TaskSources, TREE_MANIFEST_HEADER, TREE_MANIFEST_NAME,
};

/// `count` child instances with empty args and step 0, mirroring
/// the `[{"args": {}, "step": 0}, ...]` JSON the untyped fixtures
/// used.
pub fn instances(count: usize) -> Vec<TaskInstance> {
    vec![
        TaskInstance {
            name: None,
            args: BTreeMap::new(),
            step: 0,
            position: 0,
        };
        count
    ]
}

/// Write the rewritten-tree ownership manifest covering `entries`.
pub fn write_manifest(work: &Path, entries: &[&str]) {
    let tree = work.join(crate::tapacc::REWRITTEN_DIR);
    fs::create_dir_all(&tree).expect("mkdir rewritten");
    let mut contents = TREE_MANIFEST_HEADER.to_string();
    for entry in entries {
        contents.push_str(entry);
        contents.push('\n');
    }
    fs::write(tree.join(TREE_MANIFEST_NAME), contents).expect("write tree manifest");
}

/// Stage `design`'s manifest against `work`, mirroring what
/// `run_native` does before the HLS plan pass.
pub fn staged(work: &Path, design: &Design) -> TaskSources {
    stage_hls_sources(work, design).expect("stage sources")
}

/// The two-task `VecAdd` design: lower `Add` instantiated once by upper
/// top `VecAdd`, both HLS-targeted.
pub fn vadd_design() -> Design {
    let mut tasks = BTreeMap::new();
    tasks.insert("Add".to_string(), task("Add"));
    tasks.insert(
        "VecAdd".to_string(),
        Task {
            level: TaskLevel::Upper,
            tasks: BTreeMap::from([("Add".to_string(), instances(1))]),
            clock_period: Some(ClockPeriod::from_picoseconds(3330)),
            ..task("VecAdd")
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
