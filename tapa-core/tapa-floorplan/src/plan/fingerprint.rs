//! Model-fingerprint instrumentation for the floorplan formulations.
//!
//! [`fingerprint_plan_models_json`] runs the ordinary [`plan_with_inputs`]
//! orchestration with fingerprint-recording solvers over the real CBC
//! backend — the placement phase and the finish (routing) phase recorded
//! separately — and renders every model the formulations hand to the solver
//! as one canonical JSON document. `tests/golden_model_fingerprints.rs`
//! compares that rendering against
//! `tapa-core/testdata/floorplan-model-fingerprints.json`; re-recording is
//! a deliberate, reviewable act (`TAPA_BLESS_MODELS=1`), never part of a
//! refactor commit.
//!
//! [`plan_with_inputs`]: super::plan_with_inputs

use serde_json::{json, Value};
use tapa_ir::WorkState;

use super::{plan_with_retry_ceiling_and_solvers, PlanInputs, PlanOptions};
use crate::error::PlanError;
use crate::partition::ilp::MAX_USAGE_LIMIT;
use crate::solver::fingerprint::{RecordingSolver, SolveFingerprint};
use crate::solver::CbcSolver;

/// Run the plan for `state`/`inputs`, fingerprinting every model the
/// placement and routing formulations hand to CBC, and render the canonical
/// fingerprint document (with a trailing newline, matching the fixture
/// file byte for byte).
#[doc(hidden)]
pub fn fingerprint_plan_models_json(
    state: &WorkState,
    options: &PlanOptions,
    inputs: &PlanInputs,
) -> Result<String, PlanError> {
    let placement = RecordingSolver::new(CbcSolver::new());
    let finish = RecordingSolver::new(CbcSolver::new());
    // Mirror the plan_with_inputs retry ceiling.
    plan_with_retry_ceiling_and_solvers(
        state,
        options,
        inputs,
        options.usage_limit.max(MAX_USAGE_LIMIT),
        &placement,
        &finish,
    )?;
    let mut models = Vec::new();
    for (phase, records) in [
        ("partition", placement.records()),
        ("routing", finish.records()),
    ] {
        models.extend(records.iter().map(|record| record_json(phase, record)));
    }
    let document = json!({
        "fixture": "tapa-floorplan model fingerprints for the golden vadd-floorplanned design",
        "regen": "TAPA_BLESS_MODELS=1 cargo test -p tapa-floorplan --test golden_model_fingerprints",
        "scope": "every LpModel the two formulations hand to the solver, in solve order, recorded through RecordingSolver over the real CBC backend",
        "fields": {
            "index": "0-based solve sequence within its phase",
            "phase": "partition = placement ILP; routing = finish-plan routing ILP(s)",
            "sense": "objective sense",
            "vars / constraints": "variable and constraint counts as built",
            "nonzeros": "nonzero canonical coefficients, objective plus rows",
            "var_domains": "fnv1a-64 of every variable's (kind, lower, upper), in model order",
            "objective": "fnv1a-64 of the canonical objective: coefficients summed per variable, exact zeros dropped, terms sorted by variable index, constant last (reordering-equivalent models hash equal)",
            "rows": "fnv1a-64 of canonical rows (operator, constant-folded rhs, canonical terms) sorted as whole strings (reordering-equivalent models hash equal)",
            "exact_lp_text": "fnv1a-64 of the exact CPLEX-LP text CBC parses (order-sensitive; the strongest gate)",
            "solve_status / solve_objective": "the outcome CBC returned for this model"
        },
        "models": models,
    });
    let rendered = serde_json::to_string_pretty(&document).expect("json! documents serialize");
    Ok(format!("{rendered}\n"))
}

/// One model record in solve order.
fn record_json(phase: &str, record: &SolveFingerprint) -> Value {
    json!({
        "index": record.index,
        "phase": phase,
        "sense": record.sense,
        "vars": record.vars,
        "constraints": record.constraints,
        "nonzeros": record.nonzeros,
        "var_domains": record.var_domains,
        "objective": record.objective,
        "rows": record.rows,
        "exact_lp_text": record.exact_lp_text,
        "solve_status": record.solve_status,
        "solve_objective": record.solve_objective,
    })
}
