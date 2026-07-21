//! The ILP solver abstraction: an [`LpModel`] built in memory, rendered to
//! CPLEX-LP text by [`write_cplex_lp`], and handed to a [`Solver`] backend.

pub mod backends;
pub mod lp_writer;
pub mod model;
pub mod solve;

pub use backends::CbcSolver;
pub use lp_writer::write_cplex_lp;
pub use model::{Comparison, LinExpr, LpModel, LpVar, Sense, VarKind};
pub use solve::{LpSolution, LpStatus, SolveOpts, Solver, SolverError};
