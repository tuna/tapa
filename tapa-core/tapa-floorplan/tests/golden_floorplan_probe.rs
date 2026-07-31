//! Probe: run the real `tapa-floorplan` planner on the golden
//! `vadd-floorplanned` design and record the chosen `FloorplanResult`.
//!
//! This test is `#[ignore]`d on purpose: it shells out to the external
//! `cbc` solver and *writes into the source tree*. Run it explicitly:
//!
//! ```sh
//! cd tapa-core && cargo test -p tapa-floorplan \
//!   --test golden_floorplan_probe -- --ignored --nocapture
//! ```
//!
//! Output: `testdata/golden/vadd-floorplanned/floorplan.json` — the blessed
//! floorplan of the golden case, i.e. this probe IS the regeneration path
//! (see the case's `PROVENANCE.md`). The solve is deterministic (CBC with
//! tuned options + a lexicographic tie-break), so re-running it after a
//! planner change reproduces the file or honestly records the new plan.
//!
//! The planner inputs mirror what `tapa cli`'s floorplan step would build
//! for `tests/apps/vadd` on a u280: three `MemoryInterface`s (one per
//! direct M-AXI endpoint, with banks from a plausible `--connectivity`
//! file) and the distributed-control interface the top's `s_axi_control`
//! slave implies.

use tapa_floorplan::{ControlInterface, MemoryInterface, PlanInputs};
use tapa_ir::{AxiChannelWidths, AxiEndpoint, FlowSettings, MemoryBank, TaskGraph, WorkState};
use tapa_protocol::{
    axi_subport_from_suffix, axi_subport_width, M_AXI_SUFFIXES_BY_CHANNEL, M_AXI_SUFFIXES_COMPACT,
};

fn load_case_design() -> TaskGraph {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("testdata")
        .join("golden")
        .join("vadd-floorplanned")
        .join("design.json");
    let json = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    TaskGraph::from_json(&json).expect("case design.json conforms to the task-graph schema")
}

/// Physical widths of the five AXI ready/valid channels for a plain direct
/// mmap child — the same arithmetic `tapa-codegen`'s `DirectMmapInterface`
/// computes from the child's compact M-AXI port set (replicated here via
/// the shared protocol tables so this probe never depends on codegen).
fn direct_mmap_channel_widths(data_width: u32, id_width: u32) -> AxiChannelWidths {
    let physical_width = |channel: &str| {
        M_AXI_SUFFIXES_BY_CHANNEL[channel]
            .ports
            .iter()
            .filter(|suffix| M_AXI_SUFFIXES_COMPACT.contains(suffix))
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

/// The `--connectivity` story: the three top mmap ports land on different
/// banks so the planner has to place their endpoints in different slots.
fn memory_inputs() -> Vec<MemoryInterface> {
    let endpoints = [
        (("Mmap2Stream_0", "mmap_port", "a"), "HBM[0]"),
        (("Mmap2Stream_1", "mmap_port", "b"), "HBM[16]"),
        (("Stream2Mmap_0", "mmap_port", "c"), "DDR[1]"),
    ];
    endpoints
        .into_iter()
        .map(|((instance, port, top_port), bank)| MemoryInterface {
            endpoint: AxiEndpoint {
                instance: instance.to_string(),
                port: port.to_string(),
                top_port: top_port.to_string(),
            },
            bank: bank.parse::<MemoryBank>().expect("bank tag"),
            channel_widths: direct_mmap_channel_widths(32, 1),
            bridge_instance: None,
        })
        .collect()
}

#[test]
#[ignore = "spawns the external cbc solver and writes into the source tree"]
fn probe_plans_the_golden_vadd_design() {
    let graph = load_case_design();
    let mut state = WorkState::new(graph);
    state.flow = FlowSettings {
        part_num: Some("u280".to_string()),
        platform: Some("xilinx_u280_gen3x16_xdma_1_202211_1".to_string()),
        ..FlowSettings::default()
    };

    let inputs = PlanInputs {
        memory: memory_inputs(),
        control: Some(ControlInterface {
            has_s_axi_control: true,
        }),
    };

    let options = tapa_floorplan::PlanOptions {
        // Keep the probe quick and deterministic; the schedules under test
        // are tiny relative to the default budgets.
        max_seconds: 60,
        threads: 1,
        ..tapa_floorplan::PlanOptions::default()
    };
    let result = tapa_floorplan::plan_with_inputs(&state, &options, &inputs)
        .expect("planner solves the vadd design");

    println!("device: {} grid: {:?}", result.device, result.grid);
    println!("regions ({}):", result.regions.len());
    for (instance, region) in &result.regions {
        println!("  {instance} -> {region}");
    }
    println!("routes ({}):", result.routes.len());
    for route in &result.routes {
        println!(
            "  {:?} scheme={:?} path={:?} reg_regions={:?}",
            route.channel, route.scheme, route.route, route.reg_regions
        );
    }

    let out_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("testdata")
        .join("golden")
        .join("vadd-floorplanned")
        .join("floorplan.json");
    let json = serde_json::to_string_pretty(&result).expect("serialize FloorplanResult");
    std::fs::write(&out_path, format!("{json}\n"))
        .unwrap_or_else(|e| panic!("cannot write {}: {e}", out_path.display()));
    println!("wrote {}", out_path.display());
}
