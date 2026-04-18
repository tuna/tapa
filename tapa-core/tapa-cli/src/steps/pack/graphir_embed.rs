//! `--graphir-path` embedding: deserialise the user-supplied
//! `GraphIR` JSON, run `tapa_graphir_export::export_project` into a
//! staging directory, then merge the exported Verilog into
//! `<work_dir>/rtl` so the `.xo` ships with graphir-derived modules
//! alongside the TAPA-generated ones.
//!
//! Ports `tapa/program/pack.py::ProgramPackMixin.pack_xo`'s
//! `graphir_path` branch (line ~46), minus the graphir-json archive
//! copy — downstream tools consume the emitted `.v` files, not the
//! JSON itself.

use std::fs;
use std::path::{Path, PathBuf};

use tapa_graphir::Project;
use tapa_graphir_export::{export_project, ExportError};

use crate::error::{CliError, Result};

const GRAPHIR_EXPORT_DIR: &str = "graphir_hdl";

/// Read + validate + export + splice the `GraphIR` payload addressed by
/// `graphir_path` into `rtl_dir`. Returns the path to the staging
/// directory the graphir was exported into (under `work_dir /
/// graphir_hdl`) for downstream inspection.
pub(super) fn embed_graphir(
    work_dir: &Path,
    rtl_dir: &Path,
    graphir_path: &Path,
) -> Result<PathBuf> {
    if !graphir_path.is_file() {
        return Err(CliError::InvalidArg(format!(
            "--graphir-path: {} is not a file",
            graphir_path.display()
        )));
    }
    let raw = fs::read_to_string(graphir_path)?;
    let project = Project::from_json(&raw).map_err(|e| {
        CliError::InvalidArg(format!(
            "--graphir-path: {} is not a valid GraphIR project: {e}",
            graphir_path.display()
        ))
    })?;

    let export_root = work_dir.join(GRAPHIR_EXPORT_DIR);
    if !export_root.exists() {
        fs::create_dir_all(&export_root)?;
    }
    export_project(&project, &export_root)
        .map_err(|e| graphir_export_to_cli_error(&e))?;

    if !rtl_dir.is_dir() {
        return Err(CliError::InvalidArg(format!(
            "--graphir-path requires the rtl directory to exist: {}",
            rtl_dir.display()
        )));
    }
    copy_exported_rtl(&export_root, rtl_dir)?;
    Ok(export_root)
}

fn copy_exported_rtl(from: &Path, to: &Path) -> Result<()> {
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let src = entry.path();
        if !src.is_file() {
            continue;
        }
        let Some(name) = src.file_name() else { continue };
        let dest = to.join(name);
        fs::copy(&src, &dest)?;
    }
    Ok(())
}

fn graphir_export_to_cli_error(err: &ExportError) -> CliError {
    CliError::InvalidArg(format!("--graphir-path export failed: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_graphir_json() -> String {
        // Mirrors the smallest valid Project shape used by
        // `tapa-graphir` fixture tests: a single `$root` namespace
        // with one `Verilog`-variant module definition whose source
        // we can read back verbatim after export.
        serde_json::json!({
            "part_num": "xcu250-figd2104-2L-e",
            "modules": {
                "name": "$root",
                "module_definitions": [
                    {
                        "module_type": "verilog_module",
                        "name": "graphir_top",
                        "hierarchical_name": ["graphir_top"],
                        "parameters": [],
                        "ports": [
                            {
                                "name": "clk",
                                "type": "input wire"
                            }
                        ],
                        "verilog": "module graphir_top(input wire clk);\nendmodule\n",
                        "submodules_module_names": []
                    }
                ],
                "top_name": "graphir_top"
            }
        })
        .to_string()
    }

    #[test]
    fn rejects_missing_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let rtl_dir = dir.path().join("rtl");
        fs::create_dir_all(&rtl_dir).expect("mkdir rtl");
        let err = embed_graphir(
            dir.path(),
            &rtl_dir,
            &dir.path().join("does-not-exist.json"),
        )
        .expect_err("missing graphir must fail");
        assert!(matches!(err, CliError::InvalidArg(ref m) if m.contains("is not a file")));
    }

    #[test]
    fn rejects_invalid_json() {
        let dir = tempfile::tempdir().expect("tempdir");
        let rtl_dir = dir.path().join("rtl");
        fs::create_dir_all(&rtl_dir).expect("mkdir rtl");
        let bad = dir.path().join("bad.json");
        fs::write(&bad, "{ not valid").expect("write");
        let err = embed_graphir(dir.path(), &rtl_dir, &bad)
            .expect_err("bad json must fail");
        assert!(
            matches!(err, CliError::InvalidArg(ref m) if m.contains("valid GraphIR"))
        );
    }

    #[test]
    fn pack_graphir_embeds_user_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let rtl_dir = dir.path().join("rtl");
        fs::create_dir_all(&rtl_dir).expect("mkdir rtl");

        let graphir = dir.path().join("graphir.json");
        fs::write(&graphir, minimal_graphir_json()).expect("write json");

        let staging = embed_graphir(dir.path(), &rtl_dir, &graphir).expect("embed");
        assert!(staging.exists(), "staging dir must persist");

        let merged = rtl_dir.join("graphir_top.v");
        assert!(
            merged.is_file(),
            "graphir exports must be spliced into rtl_dir",
        );
        let body = fs::read_to_string(&merged).expect("read");
        assert!(
            body.contains("module graphir_top"),
            "graphir body must reach rtl_dir",
        );
    }
}
