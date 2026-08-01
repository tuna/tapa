//! The lexicographic tie-breaking refinement loop shared by the placement
//! and routing formulations.
//!
//! Both formulations re-solve after the primary solve to make equal-cost
//! optima deterministic across solver versions: pin the achieved primary
//! objective as a new `≤` row, then minimize the *stable candidate
//! ranking* `Σ candidate_index·var` over the formulation's one-of-k rows.
//! [`refine`] is that loop, parameterized on the pinned objective
//! expression — the one place the formulations genuinely differ (the
//! placement pins its achieved objective expression, the router pins its
//! norm variable or hop objective).

use crate::solver::sparse::SparseRow;
use crate::solver::{
    Comparison, LinExpr, LpModel, LpSolution, LpVar, SolveOpts, Solver, SolverError,
};

/// Row name pinning the achieved primary objective; both formulations
/// used this exact name.
const PIN_ROW: &str = "lexicographic_pin";

/// Pin the achieved primary objective and re-solve minimizing the stable
/// candidate ranking over `rank_rows` (each row's candidate position is
/// its rank, so the lowest-ranked feasible candidate wins and the optimum
/// is unique). `pinned` is set as `pinned <= achieved` and the rank
/// objective replaces the model's objective, in that order — the same
/// mutation order the formulations used inline.
///
/// Returns the refined incumbent when the refinement solve finds one;
/// `None` means the caller falls back to the primary incumbent.
pub fn refine(
    lp: &mut LpModel,
    solver: &dyn Solver,
    opts: &SolveOpts,
    pinned: LinExpr,
    achieved: f64,
    rank_rows: &[Vec<LpVar>],
) -> Result<Option<LpSolution>, SolverError> {
    lp.add_constraint(PIN_ROW.to_string(), pinned, Comparison::Le, achieved);
    lp.set_objective(rank_objective(rank_rows));
    let refined = solver.solve(lp, opts)?;
    Ok(refined.is_found().then_some(refined))
}

/// The stable candidate ranking `Σ candidate_index·var` over the one-of-k
/// rows; the zero-ranked candidate of every row is free.
fn rank_objective(rank_rows: &[Vec<LpVar>]) -> LinExpr {
    let mut terms = SparseRow::new();
    for row in rank_rows {
        for (index, &var) in row.iter().enumerate() {
            let rank = u32::try_from(index).expect("candidate count fits u32");
            if rank > 0 {
                terms.push(f64::from(rank), var);
            }
        }
    }
    terms.into_expr()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::solver::assign::add_one_of_k_row;
    use crate::solver::{LpStatus, Sense};

    /// A scripted solver: hands back a fixed solution and forgets nothing
    /// about the model, letting the test inspect the mutated model.
    struct ScriptedSolver {
        solution: LpSolution,
    }

    impl Solver for ScriptedSolver {
        fn solve(&self, model: &LpModel, _opts: &SolveOpts) -> Result<LpSolution, SolverError> {
            let mut solution = self.solution.clone();
            if solution.is_found() {
                solution.values = model
                    .vars
                    .iter()
                    .enumerate()
                    .map(|(index, _)| (LpVar(u32::try_from(index).expect("index fits u32")), 0.0))
                    .collect();
                solution.objective = 0.0;
            }
            Ok(solution)
        }
    }

    fn tiny_model() -> (LpModel, Vec<Vec<LpVar>>) {
        let mut model = LpModel::new(Sense::Minimize);
        let first = add_one_of_k_row(&mut model, "first", 2, |i| format!("a_{i}"));
        let second = add_one_of_k_row(&mut model, "second", 3, |i| format!("b_{i}"));
        model.set_objective(LinExpr::sum([(1.0, first[0])]));
        (model, vec![first, second])
    }

    #[test]
    fn rank_is_the_candidate_index_with_rank_zero_free() {
        let (_, rows) = tiny_model();
        let objective = rank_objective(&rows);
        assert_eq!(
            objective.terms,
            vec![(1.0, rows[0][1]), (1.0, rows[1][1]), (2.0, rows[1][2]),],
            "candidate 0 of each row is free, later candidates pay their index",
        );
    }

    #[test]
    fn refine_pins_first_then_replaces_the_objective() {
        let (mut model, rows) = tiny_model();
        let rows_before = model.num_constraints();
        let pin = model.objective.clone();
        let solver = ScriptedSolver {
            solution: LpSolution {
                status: LpStatus::Optimal,
                objective: 0.0,
                values: std::collections::HashMap::new(),
            },
        };

        let refined = refine(&mut model, &solver, &SolveOpts::default(), pin, 42.0, &rows)
            .expect("scripted solve succeeds");
        assert!(refined.is_some(), "a found incumbent is returned");
        assert_eq!(model.num_constraints(), rows_before + 1);
        let pin_row = &model.constraints[rows_before];
        assert_eq!(pin_row.name, PIN_ROW);
        assert_eq!(pin_row.op, Comparison::Le);
        assert_eq!(pin_row.rhs.to_bits(), 42.0_f64.to_bits());
        assert_eq!(model.objective, rank_objective(&rows));
    }

    #[test]
    fn refine_returns_none_when_the_refinement_finds_nothing() {
        let (mut model, rows) = tiny_model();
        let pin = model.objective.clone();
        let solver = ScriptedSolver {
            solution: LpSolution {
                status: LpStatus::Infeasible,
                objective: 0.0,
                values: std::collections::HashMap::new(),
            },
        };
        let refined = refine(&mut model, &solver, &SolveOpts::default(), pin, 1.0, &rows)
            .expect("scripted solve succeeds");
        assert!(refined.is_none(), "no incumbent, caller falls back");
    }
}
