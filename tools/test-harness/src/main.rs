use std::collections::HashSet;
use std::env;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    match run(env::args_os().skip(1)) {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("tapa-test-harness: {error}");
            ExitCode::from(1)
        }
    }
}

fn run(args: impl IntoIterator<Item = OsString>) -> Result<u8, String> {
    let mut args = args.into_iter();
    let python = match args.next() {
        Some(flag) if flag == "--python" => args
            .next()
            .map(PathBuf::from)
            .ok_or_else(|| "--python requires a path".to_string())?,
        Some(other) => {
            return Err(format!(
                "expected '--python <path> pytest ...', got '{}'",
                other.to_string_lossy()
            ));
        }
        None => return Err("expected '--python <path> pytest ...'".to_string()),
    };

    let mode = args
        .next()
        .ok_or_else(|| "expected mode after Python path".to_string())?;
    if mode != "pytest" {
        return Err(format!("unsupported mode '{}'", mode.to_string_lossy()));
    }

    let pytest_args: Vec<OsString> = args.collect();
    let runfiles_dir = env::var_os("RUNFILES_DIR")
        .or_else(|| env::var_os("TEST_SRCDIR"))
        .map(PathBuf::from)
        .ok_or_else(|| "RUNFILES_DIR or TEST_SRCDIR must be set".to_string())?;
    let import_roots = discover_python_import_roots(&runfiles_dir)
        .map_err(|error| format!("failed to scan runfiles: {error}"))?;
    let pythonpath =
        pythonpath_with_roots(import_roots.iter(), env::var("PYTHONPATH").ok().as_deref());

    let status = Command::new(&python)
        .arg("-m")
        .arg("pytest")
        .args(pytest_args)
        .env("PYTHONPATH", pythonpath)
        .status()
        .map_err(|error| format!("failed to run {}: {error}", python.display()))?;

    Ok(status.code().unwrap_or(1).try_into().unwrap_or(1))
}

fn discover_python_import_roots(runfiles_dir: &Path) -> io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    collect_runfiles(runfiles_dir, &mut files)?;
    Ok(python_import_roots(files))
}

fn collect_runfiles(path: &Path, files: &mut Vec<PathBuf>) -> io::Result<()> {
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_runfiles(&path, files)?;
        } else {
            files.push(path);
        }
    }
    Ok(())
}

fn python_import_roots(paths: impl IntoIterator<Item = PathBuf>) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    let mut seen = HashSet::new();

    for path in paths {
        if let Some(root) = site_packages_root(&path).or_else(|| rules_python_root(&path)) {
            if seen.insert(root.clone()) {
                roots.push(root);
            }
        }
    }

    roots
}

fn site_packages_root(path: &Path) -> Option<PathBuf> {
    let mut root = PathBuf::new();
    for component in path.components() {
        root.push(component.as_os_str());
        if component.as_os_str() == "site-packages" {
            return Some(root);
        }
    }
    None
}

fn rules_python_root(path: &Path) -> Option<PathBuf> {
    if path.file_name()? != "__init__.py" {
        return None;
    }
    let runfiles = path.parent()?;
    if runfiles.file_name()? != "runfiles" {
        return None;
    }
    let python = runfiles.parent()?;
    if python.file_name()? != "python" {
        return None;
    }
    python.parent().map(Path::to_path_buf)
}

fn pythonpath_with_roots<'a>(
    roots: impl IntoIterator<Item = &'a PathBuf>,
    existing: Option<&str>,
) -> String {
    let mut entries: Vec<PathBuf> = roots.into_iter().cloned().collect();
    if let Some(existing) = existing {
        entries.extend(env::split_paths(existing));
    }

    env::join_paths(entries)
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::path::{Path, PathBuf};

    #[test]
    fn discovers_python_import_roots_from_runfiles() {
        let files = [
            "repo/site-packages/pytest/__init__.py",
            "repo/site-packages/_pytest/main.py",
            "rules_python/python/runfiles/__init__.py",
            "repo/not_python/readme.txt",
        ];

        let roots =
            super::python_import_roots(files.iter().map(|path| Path::new(path).to_path_buf()));

        assert_eq!(
            roots,
            vec![
                PathBuf::from("repo/site-packages"),
                PathBuf::from("rules_python")
            ]
        );
    }

    #[test]
    fn prepends_import_roots_to_existing_pythonpath() {
        let roots = [
            PathBuf::from("/runfiles/repo/site-packages"),
            PathBuf::from("/runfiles/rules_python"),
        ];

        let pythonpath = super::pythonpath_with_roots(roots.iter(), Some("/already/set"));

        let expected = env::join_paths([
            Path::new("/runfiles/repo/site-packages"),
            Path::new("/runfiles/rules_python"),
            Path::new("/already/set"),
        ])
        .expect("test paths should join")
        .to_string_lossy()
        .into_owned();

        assert_eq!(pythonpath, expected);
    }
}
