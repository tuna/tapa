//! Subcommand handlers — one module per click command.
//!
//! Each module exposes a clap `Args` struct (named `<Step>Args` to keep
//! clap's argument-group names unique under flatten) and a
//! `run(&args, ctx)` entry point. The dispatcher in `chain.rs` wires
//! each module up through clap's `Subcommand` derive.

pub mod analyze;
pub mod find_clang_binary;
pub mod floorplan;
pub mod gcc;
pub mod meta;
pub mod pack;
pub mod synth;
pub mod version;
