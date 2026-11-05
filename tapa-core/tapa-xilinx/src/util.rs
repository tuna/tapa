//! Small shared helpers.

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

/// Copy every regular file under `src` into `dest`, recreating the
/// directory layout as it goes. Does nothing if `src` does not exist.
pub fn copy_tree(src: &camino::Utf8Path, dest: &camino::Utf8Path) -> std::io::Result<()> {
    if !src.is_dir() {
        return Ok(());
    }
    for entry in walkdir::WalkDir::new(src) {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }
        let src_path = entry.path();
        let rel = src_path.strip_prefix(src).expect("prefix must match");
        let rel = utf8(rel);
        let dest_path = dest.join(&rel);
        if let Some(parent) = dest_path.parent() {
            fs_err::create_dir_all(parent)?;
        }
        fs_err::copy(src_path, dest_path)?;
    }
    Ok(())
}

/// Write a `.xo`-shaped zip archive of `entries` at `path` (test helper
/// shared by the `package_xo` and `redact` unit tests).
#[cfg(test)]
pub fn write_xo(path: &camino::Utf8Path, entries: &[(&str, &str)]) {
    use std::io::Write as _;
    let f = std::fs::File::create(path).unwrap();
    let mut zw = zip::ZipWriter::new(f);
    let opts = zip::write::SimpleFileOptions::default();
    for (name, body) in entries {
        zw.start_file((*name).to_string(), opts).unwrap();
        zw.write_all(body.as_bytes()).unwrap();
    }
    zw.finish().unwrap();
}
