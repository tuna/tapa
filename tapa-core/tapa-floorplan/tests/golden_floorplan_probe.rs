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
//! The case construction lives in `tests/common/` and is shared with the
//! model-fingerprint gate (`golden_model_fingerprints.rs`), so both always
//! see identical planner inputs.

mod common;

#[test]
#[ignore = "spawns the external cbc solver and writes into the source tree"]
fn probe_plans_the_golden_vadd_design() {
    let state = common::case_work_state();
    let inputs = common::case_plan_inputs();
    let options = common::case_plan_options();

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
