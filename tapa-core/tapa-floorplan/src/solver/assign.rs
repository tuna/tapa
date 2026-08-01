//! One-of-k binary assignment rows, shared by the placement and routing
//! formulations.
//!
//! Both formulations give each chooser (a vertex's candidate regions, a
//! net's candidate paths) one binary per candidate and force exactly one
//! selection. [`add_one_of_k_row`] builds that shape in one place and
//! [`read_one_of_k`] validates the solved selection; the callers keep their
//! own variable labels and row names, so the constructed model stays
//! byte-identical to what each formulation built inline.

use crate::solver::solve::SOLUTION_TOLERANCE;
use crate::solver::{Comparison, LinExpr, LpModel, LpSolution, LpVar};

/// Allocate `count` binary choice variables and add the one-of-k row
/// `Σ vars = 1` named `row_name`; return the sparse variable row.
///
/// `label(index)` names each variable. Variables are created first, then
/// the row, matching the construction order the formulations used inline.
pub fn add_one_of_k_row(
    lp: &mut LpModel,
    row_name: &str,
    count: usize,
    label: impl Fn(usize) -> String,
) -> Vec<LpVar> {
    let vars: Vec<LpVar> = (0..count)
        .map(|index| lp.add_binary(label(index)))
        .collect();
    lp.add_constraint(
        row_name,
        LinExpr::sum(vars.iter().map(|&var| (1.0, var))),
        Comparison::Eq,
        1.0,
    );
    vars
}

/// Why a solved one-of-k selection failed readback validation. Callers map
/// these into their formulation-specific error types and wording.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum OneOfKError {
    /// The solution omitted the choice variable at `position`; a malformed
    /// solver result, never an implicit zero.
    #[error("solver result omitted the one-of-k variable at position {position}")]
    MissingVariable {
        /// Position within the choice row.
        position: usize,
    },
    /// The choice variable at `position` was not finite and binary within
    /// the solver's solution tolerance.
    #[error("one-of-k variable at position {position} is not binary: {value}")]
    NonBinary {
        /// Position within the choice row.
        position: usize,
        /// The offending value.
        value: f64,
        /// Row values validated so far, including the offending one.
        values: Vec<f64>,
    },
    /// The row selected zero or several choices instead of exactly one.
    #[error("one-of-k row selected {selected} choices instead of exactly one")]
    SelectionCount {
        /// Number of choices within tolerance of 1.
        selected: usize,
        /// All row values.
        values: Vec<f64>,
    },
}

/// Read back which choice of a one-of-k row (built by [`add_one_of_k_row`])
/// a solution selected.
///
/// Every variable must be present, finite, and binary within the solver's
/// [`SOLUTION_TOLERANCE`], and exactly one may be selected. The missing,
/// non-binary, and exactly-one checks run in per-variable order, matching
/// the readbacks the placement and routing formulations hand-wrote inline.
pub fn read_one_of_k(solution: &LpSolution, vars: &[LpVar]) -> Result<usize, OneOfKError> {
    let mut values = Vec::with_capacity(vars.len());
    let mut selected = Vec::new();
    for (position, &var) in vars.iter().enumerate() {
        let Some(value) = solution.values.get(&var).copied() else {
            return Err(OneOfKError::MissingVariable { position });
        };
        values.push(value);
        if !value.is_finite()
            || (value.abs() > SOLUTION_TOLERANCE && (value - 1.0).abs() > SOLUTION_TOLERANCE)
        {
            return Err(OneOfKError::NonBinary {
                position,
                value,
                values,
            });
        }
        if (value - 1.0).abs() <= SOLUTION_TOLERANCE {
            selected.push(position);
        }
    }
    if let [only] = selected.as_slice() {
        Ok(*only)
    } else {
        Err(OneOfKError::SelectionCount {
            selected: selected.len(),
            values,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::solver::{LpStatus, Sense, VarKind};

    #[test]
    fn one_of_k_row_shape() {
        let mut model = LpModel::new(Sense::Minimize);
        let vars = add_one_of_k_row(&mut model, "chooser", 3, |index| format!("c_{index}"));
        assert_eq!(vars.len(), 3);
        assert_eq!(model.num_vars(), 3);
        assert_eq!(model.num_constraints(), 1);

        let text = crate::solver::write_cplex_lp(&model).expect("render");
        assert!(text.contains("chooser: 1 x0 + 1 x1 + 1 x2 = 1"), "{text}");
        for var in &model.vars {
            assert_eq!(var.kind, VarKind::Binary);
        }
    }

    fn solution(vars: &[LpVar], values: &[f64]) -> LpSolution {
        LpSolution {
            status: LpStatus::Optimal,
            objective: 0.0,
            values: vars.iter().copied().zip(values.iter().copied()).collect(),
        }
    }

    #[test]
    fn read_one_of_k_accepts_a_tolerant_single_selection() {
        let mut model = LpModel::new(Sense::Minimize);
        let vars = add_one_of_k_row(&mut model, "chooser", 3, |index| format!("c_{index}"));
        let solution = solution(&vars, &[1e-9, 1.0 - 1e-9, 0.0]);
        assert_eq!(
            read_one_of_k(&solution, &vars),
            Ok(1),
            "solver-rounded binary values read back as the single selected choice"
        );
    }

    #[test]
    fn read_one_of_k_rejects_malformed_selections() {
        let mut model = LpModel::new(Sense::Minimize);
        let vars = add_one_of_k_row(&mut model, "chooser", 3, |index| format!("c_{index}"));

        let missing = solution(&vars[1..], &[1.0, 0.0]);
        assert!(matches!(
            read_one_of_k(&missing, &vars),
            Err(OneOfKError::MissingVariable { position: 0 })
        ));

        let fractional = solution(&vars, &[0.5, 1.0, 0.0]);
        assert!(matches!(
            read_one_of_k(&fractional, &vars),
            Err(OneOfKError::NonBinary { position: 0, .. })
        ));

        let none = solution(&vars, &[0.0, 0.0, 0.0]);
        assert!(matches!(
            read_one_of_k(&none, &vars),
            Err(OneOfKError::SelectionCount { selected: 0, .. })
        ));

        let two = solution(&vars, &[1.0, 1.0, 0.0]);
        assert!(matches!(
            read_one_of_k(&two, &vars),
            Err(OneOfKError::SelectionCount { selected: 2, .. })
        ));
    }
}
