//! Lowers a floorplanned topology + RTL state into a `GraphIR` Project.
//!
//! Replaces Python `tapa/graphir_conversion/`.

pub mod iface_roles;
pub mod inputs;
pub mod instantiation_builder;
pub mod module_defs;
pub mod project_builder;
pub mod slot_ports;
pub mod upper_wires;
pub mod utils;

pub use inputs::LoweringInputs;
pub use project_builder::{
    build_project, build_project_from_inputs, build_project_from_paths, build_project_from_state,
};

/// Errors from lowering operations.
#[derive(Debug, thiserror::Error)]
pub enum LoweringError {
    #[error("path not found: {0}")]
    PathNotFound(String),

    #[error("missing module: {0}")]
    MissingModule(String),

    #[error("invalid port category: {0}")]
    InvalidPortCategory(String),

    #[error("invalid interface direction: {0}")]
    InterfaceDirection(String),

    #[error("missing leaf RTL: {0}")]
    MissingLeafRtl(String),

    #[error("missing ctrl_s_axi RTL: {0}")]
    MissingCtrlSAxi(String),

    #[error("missing FSM RTL: {0}")]
    MissingFsmRtl(String),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}
