//! `--custom-rtl` overlay: copy user-provided RTL files into
//! `<work_dir>/rtl` after validating their port signatures against the
//! generated placeholder module.
//!
//! 1. Expand each CLI path: files are accepted verbatim, directories
//!    are globbed recursively.
//! 2. For each `.v` file whose module name appears in
//!    `templates_info.json`, compare the parsed port set with the generated
//!    `<work_dir>/rtl/<module>.v` placeholder. Mismatches log a warning;
//!    unknown modules are accepted as helpers.
//! 3. Copy every collected file into `<work_dir>/rtl` (overwriting
//!    generated templates when names collide).
//!
//! Port mismatches only warn, and non-Verilog files are accepted so
//! users can drop `.tcl` helpers alongside `.v` overrides.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use tapa_rtl::VerilogModule;

use crate::error::{CliError, Result};

/// Deserialised shape of `<work_dir>/templates_info.json` — a mapping
/// from task (module) name to its typed port list (only the keys are
/// consulted: the port-shape check runs against the generated
/// placeholder Verilog).
pub(super) type TemplatesInfo = BTreeMap<String, Vec<tapa_ir::Port>>;

/// Load `<work_dir>/templates_info.json` if it exists; otherwise
/// return an empty map. Synth may not emit any template entries when no
/// task uses `target("ignore")`.
pub(super) fn load_templates_info(work_dir: &Path) -> Result<TemplatesInfo> {
    let path = work_dir.join("templates_info.json");
    if !path.exists() {
        return Ok(TemplatesInfo::new());
    }
    let raw = fs_err::read_to_string(&path)?;
    let parsed: TemplatesInfo = serde_json::from_str(&raw)?;
    Ok(parsed)
}

/// Warn about `target("ignore")` tasks whose generated port shell is still
/// what will be packaged.
///
/// The shell has an empty module body, so an artifact containing one is a
/// stub: it elaborates but does nothing. Packaging used to accept that
/// silently, and the omission only surfaced as wrong results in simulation.
/// A file that still matches `<work_dir>/template/<module>.v` byte for byte
/// was not overlaid by `--custom-rtl`.
pub(super) fn warn_unimplemented_templates(
    work_dir: &Path,
    rtl_dir: &Path,
    templates_info: &TemplatesInfo,
) {
    let mut unimplemented = Vec::new();
    for module in templates_info.keys() {
        let file = format!("{module}.v");
        let generated = work_dir.join("template").join(&file);
        let packaged = rtl_dir.join(&file);
        let (Ok(generated), Ok(packaged)) = (fs_err::read(&generated), fs_err::read(&packaged))
        else {
            continue;
        };
        if generated == packaged {
            unimplemented.push(module.as_str());
        }
    }
    if unimplemented.is_empty() {
        return;
    }
    let (noun, verb) = if unimplemented.len() == 1 {
        ("task", "declares")
    } else {
        ("tasks", "declare")
    };
    log::warn!(
        "packaging an empty port shell: {noun} {} {verb} `tapa::target(\"ignore\")` but no \
         --custom-rtl replacement was supplied, so the packaged design will not compute anything",
        unimplemented.join(", "),
    );
}

/// Expand user-supplied `--custom-rtl` CLI paths. Files are accepted
/// verbatim; directories are walked recursively for any regular-file
/// entries.
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
            let mut had_file = false;
            for entry in walkdir::WalkDir::new(path) {
                let entry = entry.map_err(std::io::Error::other)?;
                if entry.file_type().is_file() {
                    out.push(entry.path().to_path_buf());
                    had_file = true;
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
/// Unknown module names are copied as helpers; port-shape mismatches
/// against known templates only log a warning.
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

    check_custom_rtl_format(&files, rtl_dir, templates_info);

    for src in &files {
        let file_name = src.file_name().ok_or_else(|| {
            CliError::InvalidArg(format!(
                "--custom-rtl path has no file name: {}",
                src.display()
            ))
        })?;
        let dest = rtl_dir.join(file_name);
        let replaced = copy_overlay(src, &dest)?;
        if replaced {
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

/// Overlay `src` onto `dest`. Returns `true` if `dest` already existed
/// (i.e. the file was replaced), `false` if it was newly added.
fn copy_overlay(src: &Path, dest: &Path) -> Result<bool> {
    let replaced = dest.try_exists()?;
    fs_err::copy(src, dest)?;
    Ok(replaced)
}

/// Best-effort port-signature check:
///
/// * Non-`.v` files log a skip message (accepts `.tcl`, `.sv`, etc.).
/// * Unparsable Verilog logs a skip message and moves on.
/// * `.v` files whose top module name is not a key in
///   `templates_info` are silently accepted as helper modules.
/// * Port-signature mismatches against a known template key log a
///   warning and proceed.
fn module_port_shape(
    module: &VerilogModule,
) -> BTreeMap<String, (tapa_rtl::port::Direction, Option<tapa_rtl::port::Width>)> {
    module
        .ports
        .iter()
        .map(|port| (port.name.clone(), (port.direction, port.width.clone())))
        .collect()
}

fn check_custom_rtl_format(rtl_files: &[PathBuf], rtl_dir: &Path, templates_info: &TemplatesInfo) {
    for path in rtl_files {
        if path.extension().and_then(|s| s.to_str()) != Some("v") {
            log::warn!(
                "custom-rtl: skip format check for non-verilog file {}",
                path.display(),
            );
            continue;
        }
        let Ok(src) = fs_err::read_to_string(path) else {
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
        // Unknown module names are helper modules,
        // not mistyped keys — skip silently.
        if !templates_info.contains_key(&module.name) {
            continue;
        }
        let placeholder_path = rtl_dir.join(format!("{}.v", module.name));
        let Ok(placeholder_source) = fs_err::read_to_string(&placeholder_path) else {
            log::warn!(
                "custom-rtl: cannot check {} because placeholder {} is missing",
                path.display(),
                placeholder_path.display(),
            );
            continue;
        };
        let Ok(placeholder) = VerilogModule::parse(&placeholder_source) else {
            log::warn!(
                "custom-rtl: cannot parse generated placeholder {}",
                placeholder_path.display(),
            );
            continue;
        };
        let expected = module_port_shape(&placeholder);
        let got = module_port_shape(&module);
        if expected != got {
            log::warn!(
                "custom-rtl: {} does not match template {} ports. \
                 Expected: {:?} Got: {:?}",
                path.display(),
                module.name,
                expected,
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
            fs_err::create_dir_all(parent).expect("mkdir");
        }
        fs_err::write(path, body).expect("write");
    }

    #[test]
    fn expands_files_and_dirs() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("a.v");
        write(&file, "module a(); endmodule\n");
        let sub = dir.path().join("sub");
        fs_err::create_dir_all(&sub).expect("mkdir");
        let nested = sub.join("b.v");
        write(&nested, "module b(); endmodule\n");

        let expanded = expand_custom_rtl_paths(&[file.clone(), sub]).expect("expand");
        assert!(expanded.contains(&file));
        assert!(expanded.contains(&nested));
    }

    #[test]
    fn rejects_missing_path() {
        let err =
            expand_custom_rtl_paths(&[PathBuf::from("/nope")]).expect_err("missing path must fail");
        assert!(matches!(err, CliError::InvalidArg(ref m) if m.contains("does not exist")));
    }

    #[test]
    fn copy_overlay_reports_pre_copy_destination_state() {
        let dir = tempfile::tempdir().expect("tempdir");
        let src = dir.path().join("src.v");
        let dest = dir.path().join("dest.v");
        write(&src, "first");

        assert!(
            !copy_overlay(&src, &dest).unwrap(),
            "first copy must be an add"
        );
        assert_eq!(fs_err::read_to_string(&dest).unwrap(), "first");

        write(&src, "second");
        assert!(
            copy_overlay(&src, &dest).unwrap(),
            "second copy must be a replace"
        );
        assert_eq!(fs_err::read_to_string(&dest).unwrap(), "second");
    }

    #[test]
    fn pack_custom_rtl_replaces_placeholder() {
        let dir = tempfile::tempdir().expect("tempdir");
        let rtl_dir = dir.path().join("rtl");
        fs_err::create_dir_all(&rtl_dir).expect("mkdir rtl");

        let seed = rtl_dir.join("Foo.v");
        write(
            &seed,
            "module Foo(input wire clk, input wire rst); endmodule\n",
        );

        let src = dir.path().join("overlay").join("Foo.v");
        write(
            &src,
            "module Foo(input wire clk, input wire rst); endmodule\n",
        );

        let mut templates = TemplatesInfo::new();
        templates.insert("Foo".to_string(), Vec::new());

        apply_custom_rtl(&rtl_dir, std::slice::from_ref(&src), &templates).expect("apply");

        let copied = fs_err::read_to_string(rtl_dir.join("Foo.v")).expect("read");
        assert!(
            copied.contains("rst"),
            "placeholder template must be overwritten by the overlay"
        );
    }

    #[test]
    fn port_shape_matches_ansi_and_nonansi_declarations() {
        let ansi = VerilogModule::parse(
            "module Foo(input wire clk, input wire [31:0] data, output wire done); endmodule\n",
        )
        .expect("parse ANSI module");
        let nonansi = VerilogModule::parse(
            "module Foo(clk, data, done); input clk; input [31:0] data; output done; endmodule\n",
        )
        .expect("parse non-ANSI module");
        assert_eq!(module_port_shape(&ansi), module_port_shape(&nonansi));

        let wrong_width = VerilogModule::parse(
            "module Foo(input wire clk, input wire [15:0] data, output wire done); endmodule\n",
        )
        .expect("parse mismatched module");
        assert_ne!(module_port_shape(&ansi), module_port_shape(&wrong_width));
    }

    /// Unknown module names are helper modules, not mistyped keys:
    /// `apply_custom_rtl` must silently copy the file through.
    #[test]
    fn pack_custom_rtl_unknown_module_name_is_copied_through() {
        let dir = tempfile::tempdir().expect("tempdir");
        let rtl_dir = dir.path().join("rtl");
        fs_err::create_dir_all(&rtl_dir).expect("mkdir rtl");

        let src = dir.path().join("Helper.v");
        write(&src, "module Helper(); endmodule\n");

        let mut templates = TemplatesInfo::new();
        templates.insert("Foo".to_string(), Vec::new());

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
        fs_err::create_dir_all(&rtl_dir).expect("mkdir rtl");
        let src = dir.path().join("Anything.v");
        write(&src, "module Anything(); endmodule\n");
        let templates = BTreeMap::new();
        apply_custom_rtl(&rtl_dir, &[src], &templates).expect("no templates → no check");
    }
}
