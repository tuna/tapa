//! `tapacc` discovery + CFLAGS composition + invocation.

pub mod cflags;
pub mod discover;
pub mod shim;

pub use shim::{TAPACC_HLS_SHIM, TAPACC_HLS_SHIM_FILE};

/// Name of the rewritten-source tree inside the work dir, passed to
/// `tapacc -emit-dir` and the root every task's `srcs` manifest entry is
/// relative to. Written by analyze; resolved by synth.
pub const REWRITTEN_DIR: &str = "rewritten";
