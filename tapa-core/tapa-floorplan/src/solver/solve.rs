//! The [`Solver`] abstraction: turn an [`LpModel`](crate::solver::LpModel)
//! into an [`LpSolution`]. Backends (CBC first) implement it.

use std::collections::HashMap;
use std::time::Duration;

use crate::solver::model::{LpModel, LpVar};

/// The outcome status the solver reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LpStatus {
    /// A solution was found. Following CBC/PuLP semantics, this covers both a
    /// proven optimum and a time-limited incumbent — both are usable.
    Optimal,
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
    /// Value of each variable that appeared in the solution.
    pub values: HashMap<LpVar, f64>,
}

impl LpSolution {
    /// The value of `var`, defaulting to `0.0` if the solver omitted it (CBC
    /// only lists nonzero variables).
    #[must_use]
    pub fn value(&self, var: LpVar) -> f64 {
        self.values.get(&var).copied().unwrap_or(0.0)
    }

    /// Whether a binary variable resolved to 1 (tolerant of solver rounding).
    #[must_use]
    pub fn is_set(&self, var: LpVar) -> bool {
        self.value(var) > 0.5
    }

    /// Whether a usable solution was found.
    #[must_use]
    pub fn is_found(&self) -> bool {
        self.status == LpStatus::Optimal
    }
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
}

/// Why a [`Solver`] run failed to produce a solution.
#[derive(Debug, thiserror::Error)]
pub enum SolverError {
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
}

/// A backend that solves an [`LpModel`].
pub trait Solver {
    /// Solve `model` under `opts`.
    ///
    /// Returns `Ok` with an [`LpSolution`] whenever the solver ran to
    /// completion — including [`LpStatus::Infeasible`], which is an answer,
    /// not an error. `Err` is reserved for the solver failing to run or
    /// produce parseable output.
    fn solve(&self, model: &LpModel, opts: &SolveOpts) -> Result<LpSolution, SolverError>;
}
