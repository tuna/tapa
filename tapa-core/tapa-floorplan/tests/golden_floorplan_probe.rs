//! Probe: run the real `tapa-floorplan` planner on the golden floorplanned
//! designs (`vadd-floorplanned`, `frame-views-floorplanned`) and record
//! each chosen `FloorplanResult`.
//!
//! This test is `#[ignore]`d on purpose: it shells out to the external
//! `cbc` solver and *writes into the source tree*. Run it explicitly:
//!
//! ```sh
//! cd tapa-core && cargo test -p tapa-floorplan \
//!   --test golden_floorplan_probe -- --ignored --nocapture
//! ```
//!
//! Output: `testdata/golden/<case>/floorplan.json` — the blessed floorplan
//! of each case, i.e. this probe IS the regeneration path (see each case's
//! `PROVENANCE.md`). The solve is deterministic (CBC with tuned options +
//! a lexicographic tie-break), so re-running it after a planner change
//! reproduces the files or honestly records the new plans.
//!
//! The case construction lives in `tests/common/` and is shared with the
//! model-fingerprint gate (`golden_model_fingerprints.rs`), so both always
//! see identical planner inputs.

mod common;

#[test]
#[ignore = "spawns the external cbc solver and writes into the source tree"]
fn probe_plans_the_golden_floorplanned_designs() {
    for case in [common::VADD, common::FRAME_VIEWS] {
        let result = tapa_floorplan::plan_with_inputs(
            &case.work_state(),
            &common::case_plan_options(),
            &case.plan_inputs(),
        )
        .unwrap_or_else(|error| panic!("planner solves {}: {error}", case.dir));

        println!(
            "{}: device {} grid {:?}",
            case.dir, result.device, result.grid
        );
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

        let out_path = case.path().join("floorplan.json");
        let json = serde_json::to_string_pretty(&result).expect("serialize FloorplanResult");
        std::fs::write(&out_path, format!("{json}\n"))
            .unwrap_or_else(|e| panic!("cannot write {}: {e}", out_path.display()));
        println!("wrote {}", out_path.display());
    }
}
