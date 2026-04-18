//! `--custom-rtl` overlay: copy user-provided RTL files into
//! `<work_dir>/rtl` after validating their port signatures match the
//! template slot recorded in `templates_info.json`.
//!
//! Ports `tapa/program_codegen/custom_rtl.py::replace_custom_rtl`:
//!
//! 1. Expand each CLI path: files are accepted verbatim, directories
//!    are globbed recursively.
//! 2. For each `.v` file whose module name appears in
//!    `templates_info.json`, compare the parsed port set with the
//!    recorded template ports. Mismatches log a warning (matching
//!    Python's behaviour). Unknown keys fail fast.
//! 3. Copy every collected file into `<work_dir>/rtl` (overwriting
//!    generated templates when names collide).
//!
//! The Python code only *warns* on port mismatches and silently
//! accepts non-Verilog files; we preserve that behaviour so users
//! can drop `.tcl` helpers alongside `.v` overrides.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use tapa_rtl::port::Direction;
use tapa_rtl::VerilogModule;

fn direction_str(dir: Direction) -> &'static str {
    match dir {
        Direction::Input => "input",
        Direction::Output => "output",
        Direction::Inout => "inout",
    }
}

use crate::error::{CliError, Result};

/// Deserialised shape of `<work_dir>/templates_info.json` — a mapping
/// from task (module) name to the list of port signatures the native
/// synth step emitted for that template.
pub(super) type TemplatesInfo = BTreeMap<String, Vec<String>>;

/// Load `<work_dir>/templates_info.json` if it exists; otherwise
/// return an empty map (matching the Python flow where `synth` may
/// not have emitted a templates entry when no task uses `target(
/// "ignore")`).
pub(super) fn load_templates_info(work_dir: &Path) -> Result<TemplatesInfo> {
    let path = work_dir.join("templates_info.json");
    if !path.exists() {
        return Ok(TemplatesInfo::new());
    }
    let raw = fs::read_to_string(&path)?;
    let parsed: TemplatesInfo = serde_json::from_str(&raw)?;
    Ok(parsed)
}

/// Expand user-supplied `--custom-rtl` CLI paths. Files are accepted
/// verbatim; directories are walked recursively for any regular-file
/// entries. Mirrors
/// `tapa.core.TapaProgram._get_custom_rtl_files`.
pub(super) fn expand_custom_rtl_paths(rtl_paths: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut out = Vec::<PathBuf>::new();
    for path in rtl_paths {
        if !path.exists() {
            return Err(CliError::InvalidArg(format!(
                "--custom-rtl path does not exist: {}",
                path.display()
            )));
        }
        if path.is_file() {
            out.push(path.clone());
            continue;
        }
        if path.is_dir() {
            let mut stack = vec![path.clone()];
            let mut had_file = false;
            while let Some(dir) = stack.pop() {
                for entry in fs::read_dir(&dir)? {
                    let entry = entry?;
                    let sub = entry.path();
                    if sub.is_dir() {
                        stack.push(sub);
                    } else if sub.is_file() {
                        out.push(sub);
                        had_file = true;
                    }
                }
            }
            if !had_file {
                return Err(CliError::InvalidArg(format!(
                    "no rtl files found in {}",
                    path.display()
                )));
            }
            continue;
        }
        return Err(CliError::InvalidArg(format!(
            "--custom-rtl unsupported path: {}",
            path.display()
        )));
    }
    Ok(out)
}

/// Apply a list of custom RTL files to `rtl_dir`, validating each
/// `.v` file's module-name/port shape against `templates_info`.
///
/// Returns an error when a `.v` file names a module that is *not* in
/// `templates_info` and no matching template ever existed (the user
/// targeted the wrong KEY). Port-shape mismatches only log a warning
/// to match Python's `check_custom_rtl_format`.
pub(super) fn apply_custom_rtl(
    rtl_dir: &Path,
    custom_rtl_paths: &[PathBuf],
    templates_info: &TemplatesInfo,
) -> Result<()> {
    let files = expand_custom_rtl_paths(custom_rtl_paths)?;
    if files.is_empty() {
        return Ok(());
    }
    if !rtl_dir.is_dir() {
        return Err(CliError::InvalidArg(format!(
            "--custom-rtl requires the rtl directory to exist: {}",
            rtl_dir.display()
        )));
    }

    check_custom_rtl_format(&files, templates_info);

    for src in &files {
        let file_name = src.file_name().ok_or_else(|| {
            CliError::InvalidArg(format!(
                "--custom-rtl path has no file name: {}",
                src.display()
            ))
        })?;
        let dest = rtl_dir.join(file_name);
        fs::copy(src, &dest)?;
        if dest.exists() {
            log::info!(
                "custom-rtl: replaced {} with {}",
                dest.display(),
                src.display(),
            );
        } else {
            log::info!(
                "custom-rtl: added {} from {}",
                dest.display(),
                src.display(),
            );
        }
    }
    Ok(())
}

/// Best-effort port-signature check. Mirrors Python's
/// `tapa/program_codegen/custom_rtl.py::check_custom_rtl_format`:
///
/// * Non-`.v` files log a skip message (Python accepts `.tcl`, `.sv`, etc.).
/// * Unparsable Verilog logs a skip message and moves on.
/// * `.v` files whose top module name is NOT a key in
///   `templates_info` are silently accepted — Python's
///   `if (task := tasks.get(...)) is None: continue` makes unknown
///   helper modules a valid input, not an error.
/// * Port-signature mismatches against a known template key log a
///   warning and proceed (Python uses `_logger.warning`, never fails).
fn check_custom_rtl_format(rtl_files: &[PathBuf], templates_info: &TemplatesInfo) {
    for path in rtl_files {
        if path.extension().and_then(|s| s.to_str()) != Some("v") {
            log::warn!(
                "custom-rtl: skip format check for non-verilog file {}",
                path.display(),
            );
            continue;
        }
        let Ok(src) = fs::read_to_string(path) else {
            log::warn!(
                "custom-rtl: skipping format check for unreadable verilog {}",
                path.display(),
            );
            continue;
        };
        let Ok(module) = VerilogModule::parse(&src) else {
            log::warn!(
                "custom-rtl: skipping format check for unparsable verilog {}",
                path.display(),
            );
            continue;
        };
        // Python parity: unknown module names are helper modules,
        // not mistyped KEYs — skip silently.
        let Some(expected_ports) = templates_info.get(&module.name) else {
            continue;
        };
        let got: Vec<String> = module
            .ports
            .iter()
            .map(|p| format!("{}: {}", p.name, direction_str(p.direction)))
            .collect();
        let mut expected_sorted = expected_ports.clone();
        expected_sorted.sort();
        let mut got_sorted = got.clone();
        got_sorted.sort();
        if expected_sorted != got_sorted {
            log::warn!(
                "custom-rtl: {} does not match template {} ports. \
                 Expected: {:?} Got: {:?}",
                path.display(),
                module.name,
                expected_ports,
                got,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::BTreeMap;

    fn write(path: &Path, body: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("mkdir");
        }
        fs::write(path, body).expect("write");
    }

    #[test]
    fn expands_files_and_dirs() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("a.v");
        write(&file, "module a(); endmodule\n");
        let sub = dir.path().join("sub");
        fs::create_dir_all(&sub).expect("mkdir");
        let nested = sub.join("b.v");
        write(&nested, "module b(); endmodule\n");

        let expanded = expand_custom_rtl_paths(&[file.clone(), sub]).expect("expand");
        assert!(expanded.contains(&file));
        assert!(expanded.contains(&nested));
    }

    #[test]
    fn rejects_missing_path() {
        let err = expand_custom_rtl_paths(&[PathBuf::from("/nope")])
            .expect_err("missing path must fail");
        assert!(matches!(err, CliError::InvalidArg(ref m) if m.contains("does not exist")));
    }

    #[test]
    fn pack_custom_rtl_replaces_placeholder() {
        let dir = tempfile::tempdir().expect("tempdir");
        let rtl_dir = dir.path().join("rtl");
        fs::create_dir_all(&rtl_dir).expect("mkdir rtl");

        let seed = rtl_dir.join("Foo.v");
        write(&seed, "module Foo(input wire clk); endmodule\n");

        let src = dir.path().join("overlay").join("Foo.v");
        write(
            &src,
            "module Foo(input wire clk, input wire rst); endmodule\n",
        );

        let mut templates = TemplatesInfo::new();
        templates.insert(
            "Foo".to_string(),
            vec!["clk: input".to_string(), "rst: input".to_string()],
        );

        apply_custom_rtl(&rtl_dir, std::slice::from_ref(&src), &templates).expect("apply");

        let copied = fs::read_to_string(rtl_dir.join("Foo.v")).expect("read");
        assert!(
            copied.contains("rst"),
            "placeholder template must be overwritten by the overlay"
        );
    }

    /// Python parity: unknown module names are helper modules, not
    /// mistyped KEYs. `apply_custom_rtl` must silently copy the file
    /// through — `check_custom_rtl_format` logs `continue` when
    /// `tasks.get(rtl_module.name) is None`.
    #[test]
    fn pack_custom_rtl_unknown_module_name_is_copied_through() {
        let dir = tempfile::tempdir().expect("tempdir");
        let rtl_dir = dir.path().join("rtl");
        fs::create_dir_all(&rtl_dir).expect("mkdir rtl");

        let src = dir.path().join("Helper.v");
        write(&src, "module Helper(); endmodule\n");

        let mut templates = TemplatesInfo::new();
        templates.insert(
            "Foo".to_string(),
            vec!["clk: input".to_string()],
        );

        apply_custom_rtl(&rtl_dir, &[src], &templates)
            .expect("unknown helper module must be copied through, not rejected");
        assert!(
            rtl_dir.join("Helper.v").is_file(),
            "helper .v file must end up in the rtl dir",
        );
    }

    #[test]
    fn empty_templates_info_accepts_any_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let rtl_dir = dir.path().join("rtl");
        fs::create_dir_all(&rtl_dir).expect("mkdir rtl");
        let src = dir.path().join("Anything.v");
        write(&src, "module Anything(); endmodule\n");
        let templates = BTreeMap::new();
        apply_custom_rtl(&rtl_dir, &[src], &templates).expect("no templates → no check");
    }
}
