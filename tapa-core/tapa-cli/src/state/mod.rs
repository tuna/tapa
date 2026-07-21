//! Work-directory state bridge.
//!
//! One versioned state file — `<work_dir>/tapa.json` — carries everything the
//! pipeline persists between steps; see [`work`]. Steps that emit their own
//! side artifacts (`templates_info.json`) share the atomic-write plumbing in
//! [`json`].

pub(crate) mod json;
pub mod work;
