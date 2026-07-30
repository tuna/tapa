//! Concrete [`Solver`](crate::solver::Solver) backends.

pub mod cbc;

#[cfg(test)]
pub(crate) use cbc::missing_cbc;
pub use cbc::CbcSolver;
