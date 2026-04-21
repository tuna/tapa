use serde_json::Value as JsonValue;
use std::env;
use std::ffi::OsString;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use zip::ZipArchive;

pub type Result<T> = std::result::Result<T, String>;

pub fn arg_str<'a>(args: &'a [OsString], index: usize, usage: &str) -> Result<&'a str> {
    args.get(index)
        .and_then(|arg| arg.to_str())
        .ok_or_else(|| format!("usage: tapa-test-tools {usage}"))
}

pub fn arg_path(args: &[OsString], index: usize, usage: &str) -> Result<PathBuf> {
    args.get(index)
        .map(PathBuf::from)
        .ok_or_else(|| format!("usage: tapa-test-tools {usage}"))
}

pub fn workspace_path(rel: &str) -> PathBuf {
    let rel = rel.trim_start_matches("_main/");
    let path = Path::new(rel);
    if path.exists() {
        return path.to_path_buf();
    }
    let workspace = env::var("TEST_WORKSPACE").unwrap_or_else(|_| "_main".to_string());
    for env_var in ["TEST_SRCDIR", "RUNFILES_DIR"] {
        if let Ok(base) = env::var(env_var) {
            for candidate in [
                Path::new(&base).join(&workspace).join(rel),
                Path::new(&base).join("_main").join(rel),
                Path::new(&base).join(rel),
            ] {
                if candidate.exists() {
                    return candidate;
                }
            }
        }
    }
    path.to_path_buf()
}

pub fn read_json(path: &Path) -> Result<JsonValue> {
    let data = fs::read_to_string(path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    serde_json::from_str(&data).map_err(|error| format!("invalid JSON {}: {error}", path.display()))
}

pub fn require_file(path: &Path) -> Result<()> {
    if !path.is_file() {
        return Err(format!("missing file {}", path.display()));
    }
    Ok(())
}

pub fn require_dir(path: &Path) -> Result<()> {
    if !path.is_dir() {
        return Err(format!("missing directory {}", path.display()));
    }
    Ok(())
}

pub fn archive_text(archive: &mut ZipArchive<File>, name: &str) -> Result<String> {
    let mut file = archive
        .by_name(name)
        .map_err(|_| format!("zip missing {name}"))?;
    let mut contents = String::new();
    std::io::Read::read_to_string(&mut file, &mut contents)
        .map_err(|error| format!("failed to read {name}: {error}"))?;
    Ok(contents)
}
