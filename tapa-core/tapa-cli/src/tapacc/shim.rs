//! The tapacc HLS analysis shim, embedded into the binary.
//!
//! [`run_tapacc`](crate::steps::analyze) writes [`TAPACC_HLS_SHIM`] into the
//! work dir and force-includes it (`-include`) so tapacc's clang can
//! type-check the Vitis HLS headers it would otherwise reject. See the header
//! comment for why this is analysis-only and never reaches synthesis.

/// Verbatim contents of the analysis shim header.
pub const TAPACC_HLS_SHIM: &str = include_str!("tapacc_hls_shim.h");

/// File name for the shim written into the work dir before `-include`.
pub const TAPACC_HLS_SHIM_FILE: &str = "tapacc_shim.h";
