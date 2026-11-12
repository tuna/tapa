//! Subcommand handlers — one module per CLI command.
//!
//! Each module exposes a clap `Args` struct (named `<Step>Args` to keep
//! clap's argument-group names unique under flatten) and a
//! `run(&args, ctx)` entry point. The dispatcher in `chain.rs` wires
//! each module up through clap's `Subcommand` derive.
//!
//! Flow selection stays typed as `tapa_ir::Target`; dispatch is exhaustive so
//! adding a target flags every step that must handle it.

pub mod analyze;
pub mod floorplan;
pub mod gcc;
pub mod link;
pub mod meta;
pub mod pack;
pub mod synth;
pub mod version;
