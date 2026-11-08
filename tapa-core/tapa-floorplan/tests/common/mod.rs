//! The golden floorplanned planner cases, built once and shared by the
//! integration tests that must see identical planner inputs: the floorplan
//! probe (`golden_floorplan_probe.rs`, the blessed-*result* regeneration
//! path) and the model-fingerprint gate (`golden_model_fingerprints.rs`,
//! the per-commit model-identity gate, on the `vadd` case).
//!
//! Each consumer declares `mod common;` at its target root; files under
//! `tests/common/` do not become integration targets of their own.
//!
//! The inputs mirror what `tapa cli`'s floorplan step would build on a
//! u280: one `MemoryInterface` per direct M-AXI endpoint (with banks from a
//! plausible `--connectivity` file) and the distributed-control interface
//! the top's `s_axi_control` slave implies.
#![allow(
    dead_code,
    reason = "every integration target compiles its own copy and uses only part of the vocabulary"
)]

use tapa_floorplan::{ControlInterface, MemoryInterface, PlanInputs, PlanOptions};
use tapa_ir::{AxiChannelWidths, AxiEndpoint, FlowSettings, MemoryBank, TaskGraph, WorkState};
use tapa_protocol::{axi_subport_from_suffix, axi_subport_width, m_axi_channel};

/// One golden floorplanned case: its directory under `testdata/golden/`
/// and its `--connectivity` story, one `((instance, port, top port),
/// bank)` per direct M-AXI endpoint. The banks are far apart, so the
/// planner has to spread their endpoints over slots.
pub struct Case {
    pub dir: &'static str,
    memory: &'static [((&'static str, &'static str, &'static str), &'static str)],
}

/// `tests/apps/vadd`: three top mmap ports on three banks.
pub const VADD: Case = Case {
    dir: "vadd-floorplanned",
    memory: &[
        (("Mmap2Stream_0", "mmap_port", "a"), "HBM[0]"),
        (("Mmap2Stream_1", "mmap_port", "b"), "HBM[16]"),
        (("Stream2Mmap_0", "mmap_port", "c"), "DDR[1]"),
    ],
};

/// `tests/functional/frame-views`: its source and destination on the
/// banks of `VADD`'s `a` and `b`.
pub const FRAME_VIEWS: Case = Case {
    dir: "frame-views-floorplanned",
    memory: &[
        (("Load_0", "src", "src"), "HBM[0]"),
        (("Store_0", "dst", "dst"), "HBM[16]"),
    ],
};

impl Case {
    /// The case directory.
    pub fn path(&self) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("testdata")
            .join("golden")
            .join(self.dir)
    }

    /// The work state: `design.json` plus the flow settings the CLI's
    /// floorplan step would have on hand (part number + platform).
    pub fn work_state(&self) -> WorkState {
        let path = self.path().join("design.json");
        let json = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
        let graph = TaskGraph::from_json(&json)
            .expect("case design.json conforms to the task-graph schema");
        let mut state = WorkState::new(graph);
        state.flow = FlowSettings {
            part_num: Some("u280".to_string()),
            platform: Some("xilinx_u280_gen3x16_xdma_1_202211_1".to_string()),
            ..FlowSettings::default()
        };
        state
    }

    /// The transient plan inputs: the connectivity story plus the top's
    /// control interface.
    pub fn plan_inputs(&self) -> PlanInputs {
        PlanInputs {
            memory: self
                .memory
                .iter()
                .map(|&((instance, port, top_port), bank)| MemoryInterface {
                    endpoint: AxiEndpoint {
                        instance: instance.to_string(),
                        port: port.to_string(),
                        top_port: top_port.to_string(),
                    },
                    bank: bank.parse::<MemoryBank>().expect("bank tag"),
                    channel_widths: direct_mmap_channel_widths(32, 1),
                    bridge_instance: None,
                })
                .collect(),
            control: Some(ControlInterface {
                has_s_axi_control: true,
            }),
            projection: None,
        }
    }
}

/// Quick, deterministic planner options; the schedules under test are tiny
/// relative to the default budgets.
pub fn case_plan_options() -> PlanOptions {
    PlanOptions {
        max_seconds: 60,
        threads: 1,
        ..PlanOptions::default()
    }
}

/// Physical widths of the five AXI ready/valid channels for a plain direct
/// mmap child — the same arithmetic `tapa-codegen`'s `DirectMmapInterface`
/// computes from the child's compact M-AXI port set (replicated here via
/// the shared protocol tables so these tests never depend on codegen).
fn direct_mmap_channel_widths(data_width: u32, id_width: u32) -> AxiChannelWidths {
    let physical_width = |channel: &str| {
        m_axi_channel(channel)
            .expect("known AXI channel")
            .compact_suffixes()
            .map(|suffix| {
                axi_subport_width(axi_subport_from_suffix(suffix), data_width, 64, id_width)
            })
            .sum()
    };

    AxiChannelWidths {
        read_address: physical_width("AR"),
        read_data: physical_width("R"),
        write_address: physical_width("AW"),
        write_data: physical_width("W"),
        write_response: physical_width("B"),
    }
}
