//! TAPA floorplan: device model, `ABGraph` generation, and slot utilities.
//!
//! Floorplanning and device-model helpers.

pub mod abgraph;
pub mod area;
pub mod device;
pub mod gen_abgraph;

pub use abgraph::{ABEdge, ABGraph, ABVertex};
pub use area::{sum_area, Area};
pub use device::{VirtualDevice, VirtualSlot};
pub use tapa_slotting::slot::SlotCoord;

/// Errors from floorplan operations.
#[derive(Debug, thiserror::Error)]
pub enum FloorplanError {
    #[error("missing field: {0}")]
    MissingField(String),

    #[error("invalid device: {0}")]
    InvalidDevice(String),

    #[error("slot not found: ({0}, {1})")]
    SlotNotFound(u32, u32),

    #[error("cannot resolve width of FIFO '{0}' for its placement edge")]
    FifoWidthUnresolved(String),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}
