//! Constrained Verilog interface parser and code generator for TAPA RTL.
//!
//! Parses module declarations to extract ports, parameters, signals,
//! and pragmas. Provides protocol-based port classification using
//! `tapa-protocol` constants.
//!
//! The `builder` module provides AST types for programmatic Verilog
//! construction. The `emit` module implements `Display` for all types,
//! enabling Verilog text generation via `to_string()` / `format!`.

pub mod builder;
pub mod classify;
pub mod emit;
pub mod expression;
pub mod module;
pub mod mutation;
pub mod param;
pub mod port;
pub mod pragma;
pub mod signal;

mod error;
pub mod parser;

pub use error::{BuilderError, ParseError};
pub use module::VerilogModule;
