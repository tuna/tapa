//! `tapacc` discovery + CFLAGS composition + invocation.

pub mod cflags;
pub mod discover;

pub use cflags::{
    get_remote_hls_cflags, get_system_cflags, get_tapa_cflags, get_tapacc_cflags,
};
pub use discover::{find_clang_binary, find_resource, POTENTIAL_PATHS};
