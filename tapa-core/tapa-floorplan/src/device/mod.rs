//! Device model and per-part table selection.

pub mod model;
pub mod select;

pub use model::{
    add_area, penalized_distance, Coor, Device, DirCaps, DirRegions, Slot, DEFAULT_USAGE_LIMIT,
    PP_DIST, UNIT_DIST_X, UNIT_DIST_Y, USABLE_WIRE_RATIO, VERTICAL_DIST_PENALTY, WIRE_CAPACITY_INF,
};
pub use select::{device_keys, select_device, SelectError};
