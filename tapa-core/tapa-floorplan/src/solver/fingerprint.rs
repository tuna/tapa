//! Deterministic fingerprints of the exact [`LpModel`]s the formulations
//! hand to a solver, so refactor commits can prove solver-input identity
//! without re-recording anything.
//!
//! Every solve call is fingerprinted twice:
//!
//! * the **structure** fingerprint — per-variable domains in model order,
//!   the canonical objective, and the canonical row multiset. Canonical
//!   forms sum duplicate coefficients per variable, drop zero
//!   coefficients, and sort, so reordered-equivalent or
//!   aggregate-identical models fingerprint *equal* while any real change
//!   (a coefficient, a bound, a right-hand side, an operator, a row, the
//!   objective) differs; and
//! * the **exact LP text** digest — the CPLEX-LP rendering CBC actually
//!   parses, order-sensitive, the strictest possible gate short of the
//!   solution itself.
//!
//! Each record also captures the solve outcome (status and achieved
//! objective), so an outcome drift shows even when the model did not
//! change. Hashes are FNV-1a 64 over unambiguous canonical strings; with a
//! handful of models per design, pairwise collision odds are astronomically
//! below the structural differences this gate hunts for.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fmt::Write as _;

use super::model::{Comparison, LpModel, LpVar, Sense, VarKind};
use super::solve::{LpSolution, LpStatus, SolveOpts, Solver, SolverError};

/// FNV-1a 64: dependency-free, deterministic across platforms and runs.
const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// Feed bytes into a running FNV-1a 64 state.
fn fnv1a64(state: u64, bytes: &[u8]) -> u64 {
    bytes.iter().fold(state, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(FNV_PRIME)
    })
}

/// Render a hash state in its stable `.json` form.
fn hex64(hash: u64) -> String {
    format!("0x{hash:016x}")
}

/// The fingerprints from one `solve` call: the model as handed over, plus
/// how that solve answered.
#[derive(Debug, Clone)]
pub struct SolveFingerprint {
    /// 0-based sequence number within the phase's recording.
    pub index: usize,
    /// Objective sense, `"minimize"` or `"maximize"`.
    pub sense: &'static str,
    /// Number of variables.
    pub vars: usize,
    /// Number of constraint rows (as built, before the writer drops any
    /// vacuous constant rows).
    pub constraints: usize,
    /// Nonzero coefficients across the canonical objective plus rows.
    pub nonzeros: usize,
    /// FNV hash of every variable's `(kind, lower, upper)` in model order.
    pub var_domains: String,
    /// FNV hash of the canonical objective (order-independent).
    pub objective: String,
    /// FNV hash of the canonical, sorted row strings (order-independent).
    pub rows: String,
    /// FNV hash of the exact rendered CPLEX-LP text (order-sensitive).
    pub exact_lp_text: String,
    /// The solve outcome on this model.
    pub solve_status: &'static str,
    /// The achieved objective the solver reported.
    pub solve_objective: f64,
}

impl SolveFingerprint {
    /// Fingerprint the model; the outcome fields are filled post-solve.
    fn of_model(model: &LpModel, index: usize) -> Self {
        Self {
            index,
            sense: sense_str(model.sense),
            vars: model.vars.len(),
            constraints: model.constraints.len(),
            nonzeros: count_nonzeros(model),
            var_domains: var_domains_hash(model),
            objective: hex64(fnv1a64(
                FNV_OFFSET,
                canonical_expr(&model.objective).as_bytes(),
            )),
            rows: rows_hash(model),
            exact_lp_text: exact_lp_hash(model),
            solve_status: "unrecorded",
            solve_objective: f64::NAN,
        }
    }
}

fn sense_str(sense: Sense) -> &'static str {
    match sense {
        Sense::Minimize => "minimize",
        Sense::Maximize => "maximize",
    }
}

fn op_str(op: Comparison) -> &'static str {
    match op {
        Comparison::Le => "<=",
        Comparison::Eq => "=",
        Comparison::Ge => ">=",
    }
}

pub fn status_str(status: LpStatus) -> &'static str {
    match status {
        LpStatus::Optimal => "optimal",
        LpStatus::Feasible => "feasible",
        LpStatus::Infeasible => "infeasible",
        LpStatus::Unbounded => "unbounded",
        LpStatus::NotSolved => "not_solved",
    }
}

/// Canonical term list: coefficients summed per variable, exact zeros
/// dropped, entries sorted by variable index — the mathematical model, not
/// the insertion history.
fn canonical_terms(terms: &[(f64, LpVar)]) -> BTreeMap<u32, f64> {
    let mut combined: BTreeMap<u32, f64> = BTreeMap::new();
    for (coefficient, var) in terms {
        if *coefficient == 0.0 {
            continue;
        }
        *combined.entry(var.0).or_insert(0.0) += *coefficient;
    }
    combined.retain(|_, coefficient| *coefficient != 0.0);
    combined
}

/// Serialize the canonical terms plus a constant as one unambiguous string.
fn canonical_terms_string(terms: &[(f64, LpVar)]) -> String {
    let mut out = String::new();
    for (var, coefficient) in canonical_terms(terms) {
        write!(out, "{:016x}@x{var};", coefficient.to_bits()).expect("String write is infallible");
    }
    out
}

/// The canonical objective: combined terms followed by the constant.
fn canonical_expr(expr: &super::model::LinExpr) -> String {
    format!(
        "{}C{:016x}",
        canonical_terms_string(&expr.terms),
        expr.constant.to_bits()
    )
}

/// One canonical row: the comparison, the constant-folded right-hand side
/// (matching what the LP writer hands to CBC), and the canonical terms.
fn canonical_row(constraint: &super::model::ConstraintDef) -> String {
    let rhs = constraint.rhs - constraint.expr.constant;
    format!(
        "{}{:016x}|{}",
        op_str(constraint.op),
        rhs.to_bits(),
        canonical_terms_string(&constraint.expr.terms),
    )
}

/// The row-multiset hash: canonical rows sorted as whole strings, then fed
/// to one hasher with a `\0` delimiter that cannot occur inside them.
fn rows_hash(model: &LpModel) -> String {
    let mut rows: Vec<String> = model.constraints.iter().map(canonical_row).collect();
    rows.sort_unstable();
    let mut state = FNV_OFFSET;
    for row in rows {
        state = fnv1a64(state, row.as_bytes());
        state = fnv1a64(state, b"\0");
    }
    hex64(state)
}

/// The variable-domain hash: `(kind, lower, upper)` per variable, in model
/// order (order-sensitive on purpose: variable order is solution identity).
fn var_domains_hash(model: &LpModel) -> String {
    let mut state = FNV_OFFSET;
    for var in &model.vars {
        let kind = match var.kind {
            VarKind::Binary => "bin",
            VarKind::Integer => "int",
            VarKind::Continuous => "con",
        };
        state = fnv1a64(state, kind.as_bytes());
        state = fnv1a64(state, &var.lower.to_bits().to_be_bytes());
        state = fnv1a64(state, &var.upper.to_bits().to_be_bytes());
    }
    hex64(state)
}

/// Hash of the exact CPLEX-LP text the CBC backend would parse.
fn exact_lp_hash(model: &LpModel) -> String {
    let text = super::lp_writer::write_cplex_lp(model)
        .expect("formulation models always render to CPLEX-LP text");
    hex64(fnv1a64(FNV_OFFSET, text.as_bytes()))
}

/// Nonzero canonical coefficients across the objective and every row.
fn count_nonzeros(model: &LpModel) -> usize {
    canonical_terms(&model.objective.terms).len()
        + model
            .constraints
            .iter()
            .map(|constraint| canonical_terms(&constraint.expr.terms).len())
            .sum::<usize>()
}

/// A [`Solver`] decorator that fingerprints every model it is asked to
/// solve, then delegates to a real solver. The plan's solve outcomes are
/// untouched: the recording rides along.
pub struct RecordingSolver<S: Solver> {
    inner: S,
    solved: RefCell<Vec<SolveFingerprint>>,
}

impl<S: Solver> RecordingSolver<S> {
    /// Wrap `inner`, recording each model before delegating the solve.
    pub fn new(inner: S) -> Self {
        Self {
            inner,
            solved: RefCell::new(Vec::new()),
        }
    }

    /// The recorded fingerprints in solve order.
    pub fn records(&self) -> Vec<SolveFingerprint> {
        self.solved.borrow().clone()
    }
}

impl<S: Solver> Solver for RecordingSolver<S> {
    fn solve(&self, model: &LpModel, opts: &SolveOpts) -> Result<LpSolution, SolverError> {
        let index = self.solved.borrow().len();
        let mut fingerprint = SolveFingerprint::of_model(model, index);
        let solution = self.inner.solve(model, opts)?;
        fingerprint.solve_status = status_str(solution.status);
        fingerprint.solve_objective = solution.objective;
        self.solved.borrow_mut().push(fingerprint);
        Ok(solution)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::solver::model::{LinExpr, Sense};

    fn two_row_model() -> LpModel {
        let mut model = LpModel::new(Sense::Minimize);
        let a = model.add_binary("a");
        let b = model.add_binary("b");
        model.set_objective(LinExpr::sum([(1.0, a), (2.0, b)]).plus_constant(1.0));
        model.add_constraint(
            "row_a",
            LinExpr::sum([(1.0, a), (3.0, b)]),
            Comparison::Le,
            4.0,
        );
        model.add_constraint("row_b", LinExpr::sum([(2.0, a)]), Comparison::Ge, 1.0);
        model
    }

    #[test]
    fn term_reordering_fingerprints_equal() {
        let model = two_row_model();

        let mut reordered = LpModel::new(Sense::Minimize);
        let a = reordered.add_binary("a");
        let b = reordered.add_binary("b");
        // Same rows with terms inserted in the other order.
        reordered.set_objective(LinExpr::sum([(2.0, b), (1.0, a)]).plus_constant(1.0));
        reordered.add_constraint("row_b", LinExpr::sum([(2.0, a)]), Comparison::Ge, 1.0);
        reordered.add_constraint(
            "row_a",
            LinExpr::sum([(3.0, b), (1.0, a)]),
            Comparison::Le,
            4.0,
        );

        let first = SolveFingerprint::of_model(&model, 0);
        let second = SolveFingerprint::of_model(&reordered, 0);
        assert_eq!(first.objective, second.objective, "canonical objective");
        assert_eq!(first.rows, second.rows, "canonical rows");
        assert_eq!(first.nonzeros, second.nonzeros);
        assert_eq!(first.var_domains, second.var_domains);
        assert_ne!(
            first.exact_lp_text, second.exact_lp_text,
            "the exact LP text remains order-sensitive by design",
        );
    }

    #[test]
    fn duplicate_terms_combine_before_hashing() {
        let mut combined = LpModel::new(Sense::Minimize);
        let x = combined.add_binary("x");
        combined.set_objective(LinExpr::new());
        combined.add_constraint("row", LinExpr::sum([(5.0, x)]), Comparison::Le, 1.0);

        let mut split = LpModel::new(Sense::Minimize);
        let y = split.add_binary("x");
        split.set_objective(LinExpr::new());
        split.add_constraint(
            "row",
            LinExpr::sum([(2.0, y), (3.0, y)]),
            Comparison::Le,
            1.0,
        );

        assert_eq!(
            SolveFingerprint::of_model(&combined, 0).rows,
            SolveFingerprint::of_model(&split, 0).rows,
            "aggregating duplicate terms is CBC-equivalent",
        );
    }

    #[test]
    fn real_changes_show() {
        let baseline = SolveFingerprint::of_model(&two_row_model(), 0);

        let mut changed = two_row_model();
        changed.constraints[1].rhs = 2.0;
        assert_ne!(
            baseline.rows,
            SolveFingerprint::of_model(&changed, 0).rows,
            "a right-hand-side change must show",
        );

        let mut changed = two_row_model();
        changed.objective.terms.push((3.0, LpVar(0)));
        let drifted = SolveFingerprint::of_model(&changed, 0);
        assert_ne!(baseline.objective, drifted.objective, "objective terms");
        assert_eq!(
            baseline.nonzeros, drifted.nonzeros,
            "duplicate objective terms combine, so the nonzero count holds",
        );

        let mut changed = two_row_model();
        changed.constraints[1].expr.terms.push((3.0, LpVar(1)));
        let drifted = SolveFingerprint::of_model(&changed, 0);
        assert_ne!(
            baseline.nonzeros, drifted.nonzeros,
            "a genuinely new term changes the nonzero count",
        );
        assert_ne!(baseline.rows, drifted.rows);

        let mut changed = two_row_model();
        changed.vars[1].kind = VarKind::Continuous;
        assert_ne!(
            baseline.var_domains,
            SolveFingerprint::of_model(&changed, 0).var_domains,
            "a variable-domain change must show",
        );
    }
}
