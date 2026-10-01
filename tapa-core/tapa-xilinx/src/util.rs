//! Small shared path helpers.

use camino::Utf8PathBuf;

/// Lossy conversion to `Utf8PathBuf` (TAPA paths are UTF-8 in
/// practice; a non-UTF-8 path degrades to its lossy form rather than
/// failing the whole operation).
pub fn utf8(p: impl AsRef<std::path::Path>) -> Utf8PathBuf {
    Utf8PathBuf::from(p.as_ref().to_string_lossy().into_owned())
}

/// Absolute form of `p`: already-absolute paths pass through; relative
/// paths are joined onto the current directory and canonicalized when
/// they exist (a not-yet-created target keeps the plain joined form).
pub fn absolutize(p: &camino::Utf8Path) -> Utf8PathBuf {
    if p.is_absolute() {
        return p.to_path_buf();
    }
    let joined = std::env::current_dir()
        .map_or_else(|_| Utf8PathBuf::from(".").join(p), |cwd| utf8(cwd).join(p));
    std::fs::canonicalize(&joined).map_or_else(|_| joined.clone(), utf8)
}
