//! `tapa` CLI library — exposes argument parsing, dispatch, and step
//! handlers so integration tests can drive them without forking a binary.

pub mod chain;
pub mod context;
pub mod error;
pub mod globals;
pub mod logging;
pub mod options;
pub mod state;
pub mod steps;
pub mod tapacc;

pub use error::{CliError, Result};
