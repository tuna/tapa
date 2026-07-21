//! The [`Solver`] abstraction: turn an [`LpModel`](crate::solver::LpModel)
//! into an [`LpSolution`]. Backends (CBC first) implement it.

use std::collections::HashMap;
use std::time::Duration;

use crate::solver::model::{Comparison, LpModel, LpVar, VarKind};

/// Tolerance used when checking CBC's decimal solution-file output.
const SOLUTION_TOLERANCE: f64 = 1e-6;

/// The outcome status the solver reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LpStatus {
    /// The solver proved that the returned solution is optimal.
    Optimal,
    /// The solver stopped early with a feasible, integral incumbent.
    Feasible,
    /// The model has no feasible solution.
    Infeasible,
    /// The objective is unbounded.
    Unbounded,
    /// The solver stopped without a usable solution.
    NotSolved,
}

/// A solved model's status, objective, and variable assignment.
#[derive(Debug, Clone)]
pub struct LpSolution {
    pub status: LpStatus,
    pub objective: f64,
    /// Value of every model variable in a usable solution.
    pub values: HashMap<LpVar, f64>,
}

impl LpSolution {
    /// The value of `var`.
    ///
    /// Solver backends must return every model variable for a usable solution.
    /// Missing values are a malformed solver result, never an implicit zero.
    #[must_use]
    pub fn value(&self, var: LpVar) -> f64 {
        *self
            .values
            .get(&var)
            .unwrap_or_else(|| panic!("solver result omitted variable x{}", var.0))
    }

    /// Whether a binary variable resolved to 1 (tolerant of solver rounding).
    #[must_use]
    pub fn is_set(&self, var: LpVar) -> bool {
        self.value(var) > 0.5
    }

    /// Whether a usable solution was found.
    #[must_use]
    pub fn is_found(&self) -> bool {
        matches!(self.status, LpStatus::Optimal | LpStatus::Feasible)
    }

    /// Verify that a claimed incumbent is complete, finite, in-domain,
    /// integral, feasible, and has the reported objective value.
    pub(crate) fn validate_for(&self, model: &LpModel) -> Result<(), SolverError> {
        if !self.is_found() {
            return Ok(());
        }
        if !self.objective.is_finite() {
            return Err(SolverError::InvalidSolution(
                "the incumbent objective is not finite".to_string(),
            ));
        }
        if self.values.len() != model.vars.len() {
            return Err(SolverError::InvalidSolution(format!(
                "the incumbent contains {} values for a {}-variable model",
                self.values.len(),
                model.vars.len(),
            )));
        }

        for var in self.values.keys() {
            let Ok(index) = usize::try_from(var.0) else {
                return Err(SolverError::InvalidSolution(format!(
                    "the incumbent contains unknown variable x{}",
                    var.0,
                )));
            };
            if index >= model.vars.len() {
                return Err(SolverError::InvalidSolution(format!(
                    "the incumbent contains unknown variable x{}",
                    var.0,
                )));
            }
        }

        for (index, definition) in model.vars.iter().enumerate() {
            let var = LpVar(u32::try_from(index).expect("variable count fits u32"));
            let value = self.values.get(&var).copied().ok_or_else(|| {
                SolverError::InvalidSolution(format!("the incumbent omitted variable x{index}"))
            })?;
            if !value.is_finite() {
                return Err(SolverError::InvalidSolution(format!(
                    "variable x{index} is not finite",
                )));
            }

            let tolerance = scaled_tolerance([value, definition.lower, definition.upper]);
            if value < definition.lower - tolerance || value > definition.upper + tolerance {
                return Err(SolverError::InvalidSolution(format!(
                    "variable x{index} = {value} is outside [{}, {}]",
                    definition.lower, definition.upper,
                )));
            }
            if matches!(definition.kind, VarKind::Binary | VarKind::Integer)
                && (value - value.round()).abs() > SOLUTION_TOLERANCE
            {
                return Err(SolverError::InvalidSolution(format!(
                    "integer variable x{index} has fractional value {value}",
                )));
            }
        }

        for constraint in &model.constraints {
            let lhs = evaluate(&constraint.expr, &self.values)?;
            let tolerance = scaled_tolerance([lhs, constraint.rhs]);
            let satisfied = match constraint.op {
                Comparison::Le => lhs <= constraint.rhs + tolerance,
                Comparison::Eq => (lhs - constraint.rhs).abs() <= tolerance,
                Comparison::Ge => lhs >= constraint.rhs - tolerance,
            };
            if !satisfied {
                return Err(SolverError::InvalidSolution(format!(
                    "constraint `{}` is violated: {lhs} {:?} {}",
                    constraint.name, constraint.op, constraint.rhs,
                )));
            }
        }

        let objective = evaluate(&model.objective, &self.values)?;
        if (objective - self.objective).abs() > scaled_tolerance([objective, self.objective]) {
            return Err(SolverError::InvalidSolution(format!(
                "reported objective {} does not match evaluated objective {objective}",
                self.objective,
            )));
        }
        Ok(())
    }
}

fn evaluate(
    expression: &crate::solver::model::LinExpr,
    values: &HashMap<LpVar, f64>,
) -> Result<f64, SolverError> {
    let mut result = expression.constant;
    for (coefficient, var) in &expression.terms {
        let value = values.get(var).copied().ok_or_else(|| {
            SolverError::InvalidSolution(format!("the incumbent omitted variable x{}", var.0))
        })?;
        result += coefficient * value;
    }
    if !result.is_finite() {
        return Err(SolverError::InvalidSolution(
            "evaluating the incumbent produced a non-finite value".to_string(),
        ));
    }
    Ok(result)
}

fn scaled_tolerance(values: impl IntoIterator<Item = f64>) -> f64 {
    let scale = values
        .into_iter()
        .filter(|value| value.is_finite())
        .fold(1.0_f64, |scale, value| scale.max(value.abs()));
    SOLUTION_TOLERANCE * scale
}

/// Tunable limits handed to a [`Solver`].
#[derive(Debug, Clone, Default)]
pub struct SolveOpts {
    /// Wall-clock limit; the solver returns its best incumbent when it expires.
    pub time_limit: Option<Duration>,
    /// Worker thread count. `Some(1)` for deterministic solves.
    pub threads: Option<u32>,
    /// Relative MIP optimality gap to accept.
    pub mip_gap: Option<f64>,
    /// Absolute MIP optimality gap to accept.
    pub mip_gap_abs: Option<f64>,
}

impl SolveOpts {
    /// Reject options that CBC would otherwise reinterpret or silently ignore.
    pub fn validate(&self) -> Result<(), SolverError> {
        if self.time_limit.is_some_and(|limit| limit.is_zero()) {
            return Err(SolverError::InvalidOptions(
                "the solver time limit must be greater than zero".to_string(),
            ));
        }
        if self.threads == Some(0) {
            return Err(SolverError::InvalidOptions(
                "the solver thread count must be greater than zero".to_string(),
            ));
        }
        validate_gap("relative MIP gap", self.mip_gap)?;
        validate_gap("absolute MIP gap", self.mip_gap_abs)?;
        Ok(())
    }
}

fn validate_gap(name: &str, gap: Option<f64>) -> Result<(), SolverError> {
    if gap.is_some_and(|gap| !gap.is_finite() || gap < 0.0) {
        return Err(SolverError::InvalidOptions(format!(
            "the {name} must be finite and non-negative",
        )));
    }
    Ok(())
}

/// Why a [`Solver`] run failed to produce a solution.
#[derive(Debug, thiserror::Error)]
pub enum SolverError {
    /// Solver options are outside the backend-independent valid domain.
    #[error("invalid solver options: {0}")]
    InvalidOptions(String),
    /// The solver binary could not be found or spawned.
    #[error("failed to run the solver `{program}`: {source}")]
    Spawn {
        /// The program that could not be launched.
        program: String,
        /// The underlying I/O error.
        source: std::io::Error,
    },
    /// The solver ran but exited with a failure status.
    #[error("solver exited with status {status}: {stderr}")]
    ToolFailure {
        /// The process exit code, or -1 if terminated by a signal.
        status: i32,
        /// Captured standard error.
        stderr: String,
    },
    /// The solver's output could not be parsed.
    #[error("could not parse solver output: {0}")]
    BadOutput(String),
    /// The solver claimed an incumbent that does not satisfy the model.
    #[error("solver returned an invalid incumbent: {0}")]
    InvalidSolution(String),
}

/// A backend that solves an [`LpModel`].
pub trait Solver {
    /// Solve `model` under `opts`.
    ///
    /// Returns `Ok` with an [`LpSolution`] whenever the solver ran to
    /// completion — including [`LpStatus::Infeasible`], which is an answer,
    /// not an error. Invalid options, tool failures, malformed output, and an
    /// invalid claimed incumbent return `Err`.
    fn solve(&self, model: &LpModel, opts: &SolveOpts) -> Result<LpSolution, SolverError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::solver::model::{LinExpr, Sense};

    fn binary_model() -> (LpModel, LpVar) {
        let mut model = LpModel::new(Sense::Minimize);
        let x = model.add_binary("x");
        model.set_objective(LinExpr::sum([(1.0, x)]));
        model.add_constraint("pick", LinExpr::sum([(1.0, x)]), Comparison::Eq, 1.0);
        (model, x)
    }

    #[test]
    fn solve_options_reject_zero_limits_and_invalid_gaps() {
        let zero_time = SolveOpts {
            time_limit: Some(Duration::ZERO),
            ..SolveOpts::default()
        };
        assert!(matches!(
            zero_time.validate(),
            Err(SolverError::InvalidOptions(_))
        ));

        let zero_threads = SolveOpts {
            threads: Some(0),
            ..SolveOpts::default()
        };
        assert!(matches!(
            zero_threads.validate(),
            Err(SolverError::InvalidOptions(_))
        ));

        for gap in [f64::NAN, f64::INFINITY, -0.1] {
            let relative = SolveOpts {
                mip_gap: Some(gap),
                ..SolveOpts::default()
            };
            relative.validate().expect_err("invalid relative gap");
            let absolute = SolveOpts {
                mip_gap_abs: Some(gap),
                ..SolveOpts::default()
            };
            absolute.validate().expect_err("invalid absolute gap");
        }
    }

    #[test]
    fn incumbent_validation_fails_closed() {
        let (model, x) = binary_model();
        let missing = LpSolution {
            status: LpStatus::Optimal,
            objective: 0.0,
            values: HashMap::new(),
        };
        assert!(matches!(
            missing.validate_for(&model),
            Err(SolverError::InvalidSolution(_))
        ));

        let fractional = LpSolution {
            status: LpStatus::Feasible,
            objective: 0.5,
            values: HashMap::from([(x, 0.5)]),
        };
        assert!(matches!(
            fractional.validate_for(&model),
            Err(SolverError::InvalidSolution(_))
        ));

        let infeasible = LpSolution {
            status: LpStatus::Optimal,
            objective: 0.0,
            values: HashMap::from([(x, 0.0)]),
        };
        assert!(matches!(
            infeasible.validate_for(&model),
            Err(SolverError::InvalidSolution(_))
        ));
    }

    #[test]
    fn incumbent_validation_accepts_verified_early_solution() {
        let (model, x) = binary_model();
        let incumbent = LpSolution {
            status: LpStatus::Feasible,
            objective: 1.0,
            values: HashMap::from([(x, 1.0)]),
        };
        incumbent.validate_for(&model).expect("valid incumbent");
        assert!(incumbent.is_found());
    }
}
