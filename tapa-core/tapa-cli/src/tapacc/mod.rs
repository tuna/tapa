//! `tapacc` discovery + CFLAGS composition + invocation.

pub mod cflags;
pub mod discover;
pub mod shim;

pub use cflags::{get_remote_hls_cflags, get_system_cflags, get_tapa_cflags, get_tapacc_cflags};
pub use discover::{find_clang_binary, find_resource};
pub use shim::{TAPACC_HLS_SHIM, TAPACC_HLS_SHIM_FILE};
