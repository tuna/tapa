//! Model fingerprints for the golden `vadd-floorplanned` design.
//!
//! Refactoring the floorplan must keep solver numerics identical per
//! commit: CBC is deterministic given a fixed input,
//! so the models the two formulations hand to the solver are the whole
//! contract. This gate records — per solve call, in order — each model's
//! shape (variables, constraints, nonzeros, sense), its order-independent
//! canonical structure (variable domains, objective, row multiset), the
//! exact CPLEX-LP text digest, and the solve outcome, in
//! `tapa-core/testdata/floorplan-model-fingerprints.json`. Any real model
//! drift fails here; reordered-equivalent models pass by design.
//!
//! The plan runs on the real CBC backend, constructed through the same
//! `tests/common/` case as the floorplan probe, so the two gates always
//! cover the same planner inputs.
//!
//! REGENERATE (only as an intentional, reviewable change):
//!
//! ```sh
//! cd tapa-core && TAPA_BLESS_MODELS=1 cargo test -p tapa-floorplan \
//!   --test golden_model_fingerprints
//! ```

mod common;

use std::path::PathBuf;

use tapa_floorplan::partition::IlpError;
use tapa_floorplan::pipeline::plan::PipelineError;
use tapa_floorplan::route::RouteError;
use tapa_floorplan::solver::SolverError;
use tapa_floorplan::PlanError;

/// The recorded fingerprints, shared with no other gate.
fn fixture_path() -> PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("testdata")
        .join("floorplan-model-fingerprints.json")
}

#[test]
fn golden_models_match_recorded_fingerprints() {
    let state = common::case_work_state();
    let options = common::case_plan_options();
    let inputs = common::case_plan_inputs();

    let rendered = match tapa_floorplan::fingerprint_plan_models_json(&state, &options, &inputs) {
        Ok(rendered) => rendered,
        Err(error) if is_missing_cbc(&error) => panic!(
            "`cbc` was not found on PATH: the CBC solver is required to test \
             tapa-floorplan (Debian/Ubuntu: `sudo apt install coinor-cbc`)"
        ),
        Err(other) => panic!("fingerprinted plan failed: {other}"),
    };

    let path = fixture_path();
    if std::env::var_os("TAPA_BLESS_MODELS").is_some() {
        std::fs::write(&path, &rendered)
            .unwrap_or_else(|e| panic!("cannot bless {}: {e}", path.display()));
        println!("blessed {}", path.display());
        return;
    }

    let recorded = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "cannot read {}: {e} — record it with TAPA_BLESS_MODELS=1",
            path.display()
        )
    });
    assert_equal(&recorded, &rendered);
}

/// The spawn-failure shape of `cbc`, wherever the plan surfaces it.
fn is_missing_cbc(error: &PlanError) -> bool {
    matches!(
        error,
        PlanError::Ilp(IlpError::Solver(SolverError::Spawn { .. }))
            | PlanError::Pipeline(PipelineError::Route(RouteError::Solver(
                SolverError::Spawn { .. },
            )))
    )
}

/// Where the recorded and rendered documents first differ.
fn first_mismatch(recorded: &str, rendered: &str) -> String {
    recorded
        .lines()
        .zip(rendered.lines())
        .enumerate()
        .find(|(_, (recorded, rendered))| recorded != rendered)
        .map_or_else(
            || "the trailing content differs".to_string(),
            |(index, (recorded, rendered))| {
                format!(
                    "line {}:\n  recorded: {recorded}\n  rendered: {rendered}",
                    index + 1
                )
            },
        )
}

/// Assert byte equality with an actionable first-mismatch report.
fn assert_equal(recorded: &str, rendered: &str) {
    if recorded == rendered {
        return;
    }
    let mismatch = first_mismatch(recorded, rendered);
    panic!(
        "the golden design's solver models drifted from the recording (first \
         mismatch at {mismatch}). If this change is intentional, re-record \
         with TAPA_BLESS_MODELS=1 and review the fixture diff like any source \
         change; the floorplan-model fingerprints exist to make solver-input \
         drift impossible to miss."
    );
}
