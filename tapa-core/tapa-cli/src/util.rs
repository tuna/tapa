//! Small shared helpers.

use std::path::PathBuf;

use camino::Utf8PathBuf;

/// Convert a [`PathBuf`] to a [`Utf8PathBuf`], falling back to a lossy
/// conversion when the path is not valid UTF-8. The synth/pack paths all
/// originate from TAPA-controlled directories, so the lossy branch is a
/// last resort rather than a real data path.
pub fn utf8(p: PathBuf) -> Utf8PathBuf {
    Utf8PathBuf::from_path_buf(p)
        .unwrap_or_else(|p| Utf8PathBuf::from(p.to_string_lossy().into_owned()))
}

/// Resolve the first Xilinx HLS/Vitis installation root (preferring
/// `XILINX_HLS`) whose `include/` subdir exists. Used both to add
/// `-isystem` flags (`tapa g++`) and to seed vendor include probing
/// (`tapacc` cflags).
pub fn vendor_hls_root() -> Option<PathBuf> {
    for env_name in ["XILINX_HLS", "XILINX_VITIS"] {
        if let Some(root) = std::env::var_os(env_name) {
            let root = PathBuf::from(root);
            if root.join("include").exists() {
                return Some(root);
            }
        }
    }
    None
}
