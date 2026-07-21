//! Concrete [`Solver`](crate::solver::Solver) backends.

pub mod cbc;

pub use cbc::CbcSolver;
