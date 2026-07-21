//! Small shared helpers.

use std::path::PathBuf;

use camino::Utf8PathBuf;

/// Convert a path to a [`Utf8PathBuf`] via a lossy conversion. The
/// synth/pack paths all originate from TAPA-controlled directories, so
/// the lossy branch is a last resort rather than a real data path.
pub fn utf8(p: impl AsRef<std::path::Path>) -> Utf8PathBuf {
    Utf8PathBuf::from(p.as_ref().to_string_lossy().into_owned())
}

/// Render a compile-time-known minijinja template. Template parse
/// and render failures are programming errors (the templates are
/// `include_str!` constants), so they panic rather than propagate.
pub fn render_template(name: &str, src: &str, ctx: minijinja::Value) -> String {
    let mut env = minijinja::Environment::new();
    env.add_template(name, src).expect("template parses");
    env.get_template(name)
        .expect("template exists")
        .render(ctx)
        .expect("render succeeds")
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
