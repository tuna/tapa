//! Enriched topology + RTL state for code generation.
//!
//! `rtl_state` holds `TopologyWithRtl`; `mmap` hosts the M-AXI connection
//! aggregation and direct-M-AXI catalog (re-exported via `rtl_state`);
//! `views` hosts the narrowed per-concern borrow views the pass pipeline
//! consumes (see [`crate::passes`]).

mod mmap;
pub mod rtl_state;
pub mod views;
