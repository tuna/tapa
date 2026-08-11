//! Device model and per-part table selection.

pub mod model;
pub mod select;

pub use model::{
    Coor, Device, DirCaps, Resource, Slot, DEFAULT_USAGE_LIMIT, UNIT_DIST_X, UNIT_DIST_Y,
    VERTICAL_DIST_PENALTY, WIRE_CAPACITY_INF,
};
pub use select::{select_device, SelectError};
