//! `tapa` CLI library — argument parsing, dispatch, and step handlers,
//! factored out of the thin `main.rs` binary entry point (the bin/lib
//! split). The sole integration test drives the compiled binary over
//! `std::process::Command`, not this crate.

mod aie;
pub mod chain;
pub mod context;
pub mod error;
pub mod globals;
pub mod logging;
pub mod remote_config;
pub mod state;
pub mod steps;
pub mod tapacc;
pub mod update;
mod util;

#[cfg(test)]
mod testutil;
