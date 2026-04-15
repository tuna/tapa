//! TAPA `GraphIR` schema — serde structs for `graphir.json`.

pub mod project;
pub mod module;
pub mod expression;
pub mod interface;
pub mod blackbox;

mod error;

pub use error::ParseError;
pub use project::Project;
