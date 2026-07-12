//! Error types for tapa-slotting.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum SlottingError {
    #[error("empty source input")]
    EmptySource,

    #[error("function '{0}' not found")]
    FunctionNotFound(String),

    #[error("invalid port index in '{0}': must be a numeric index")]
    InvalidPortIndex(String),

    #[error("unknown port category: {0}")]
    UnknownPortCategory(String),

    #[error("tree-sitter error: {0}")]
    TreeSitter(String),

    #[error("floorplan: graph missing '{0}' field")]
    MissingGraphField(String),

    #[error("floorplan: top task `{0}` is a leaf; cannot transform")]
    TopIsLeaf(String),

    #[error("floorplan: instance `{0}` not found among top's leaf children")]
    UnknownFloorplanInstance(String),

    #[error("floorplan: top contains more than one instance named `{0}`")]
    DuplicateGraphInstanceName(String),

    #[error("floorplan: slot `{0}` has no assigned instances")]
    EmptyFloorplanSlot(String),

    #[error("floorplan: instance `{0}` is assigned to more than one slot")]
    DuplicateFloorplanInstance(String),

    #[error("floorplan: instance `{0}` is not assigned to any slot")]
    UnassignedFloorplanInstance(String),

    #[error("floorplan: slot name `{0}` collides with an existing task")]
    SlotNameCollision(String),
}
