//! Test fixtures shared by the synth step's test modules: a minimal
//! typed task/port builder, the rewritten-tree ownership manifest
//! writer, the source-staging shortcut, and the two-task `VecAdd` design.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use tapa_ir::{ArgCategory, ClockPeriod, Design, Port, SynthTarget, Task, TaskInstance, TaskLevel};

use super::hls_sources::{
    stage_hls_sources, TaskSources, TREE_MANIFEST_HEADER, TREE_MANIFEST_NAME,
};

/// A minimal lower-level HLS task: `{name}.cpp` as its only source, no
/// ports or children. Call sites override fields with struct-update
/// syntax.
pub fn task(name: &str) -> Task {
    Task {
        level: TaskLevel::Lower,
        srcs: vec![format!("{name}.cpp")],
        include_dirs: Vec::new(),
        defines: Vec::new(),
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
/// fields vary across the synth test fixtures.
pub fn port(name: &str, cat: ArgCategory, ctype: &str, width: u32) -> Port {
    Port {
        cat,
        name: name.to_string(),
        ctype: ctype.to_string(),
        width,
        chan_count: None,
        chan_size: None,
        aie_frame: None,
        aie_plio: None,
        stream_depth: None,
        mmap_addr_width: None,
    }
}

/// `count` child instances with empty args and step 0, mirroring
/// the `[{"args": {}, "step": 0}, ...]` JSON the untyped fixtures
/// used.
pub fn instances(count: usize) -> Vec<TaskInstance> {
    vec![
        TaskInstance {
            name: None,
            args: BTreeMap::new(),
            step: 0,
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
