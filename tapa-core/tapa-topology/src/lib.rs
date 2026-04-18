//! Topology-only compilation model for TAPA.
//!
//! Provides typed Rust structs for `design.json` with round-trip serde.
//! Contains Program (task collection), Task (with RTL annotations),
//! and Instance types.

pub mod design;
pub mod instance;
pub mod program;
pub mod task;

mod error;

pub use design::Design;
pub use error::ParseError;
