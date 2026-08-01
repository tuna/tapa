//! Enriched topology + RTL state for code generation.
//!
//! `rtl_state` holds `TopologyWithRtl`; `views` hosts the narrowed
//! per-concern borrow views the pass pipeline consumes (Phase 1b, see
//! [`crate::passes`]).

pub mod rtl_state;
pub mod views;
