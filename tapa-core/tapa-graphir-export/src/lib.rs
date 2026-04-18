//! Verilog file export from a `GraphIR` Project.
//!
//! Replaces Python `tapa/verilog/graphir_exporter/`.

pub mod verilog;

use std::path::Path;

use tapa_graphir::{AnyModuleDefinition, Project};

/// Errors from export operations.
#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("DRC failure in module {module}: undefined wires: {wires:?}")]
    MissingWire {
        module: String,
        wires: Vec<String>,
    },

    #[error("export destination does not exist: {0}")]
    MissingDestination(String),

    #[error("export destination is not a directory: {0}")]
    NotADirectory(String),
}

/// Export a full project to a destination directory.
///
/// The destination must already exist and be a directory; we refuse to create
/// it implicitly so that typos in the output path surface as errors instead of
/// quietly writing into a freshly-created tree (matching the planned negative
/// test for `export_project`).
///
/// Writes one `.v` file per non-stub module definition, plus blackbox files.
/// Reorganizes `.xci` files into per-stem subdirectories.
pub fn export_project(project: &Project, dest: &Path) -> Result<(), ExportError> {
    if !dest.exists() {
        return Err(ExportError::MissingDestination(dest.display().to_string()));
    }
    if !dest.is_dir() {
        return Err(ExportError::NotADirectory(dest.display().to_string()));
    }

    // Export module definitions
    for module in &project.modules.module_definitions {
        export_module(module, dest)?;
    }

    // Export blackboxes
    for bb in &project.blackboxes {
        let file_path = dest.join(&bb.path);
        if let Some(parent) = file_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if let Ok(data) = bb.get_binary() {
            std::fs::write(&file_path, data)?;
        }
    }

    // Reorganize XCI files
    create_xci_sub_folders(dest)?;

    // Create stub files for XCI and Xilinx primitives
    create_stub_files(dest)?;

    Ok(())
}

/// Export a single module definition to a `.v` file.
fn export_module(module: &AnyModuleDefinition, dest: &Path) -> Result<(), ExportError> {
    // Run DRC on grouped modules
    if let AnyModuleDefinition::Grouped { base, grouped, .. }
    | AnyModuleDefinition::InternalGrouped { base, grouped, .. } = module
    {
        let missing = verilog::check_missing_wire(base, grouped);
        if !missing.is_empty() {
            return Err(ExportError::MissingWire {
                module: base.name.clone(),
                wires: missing,
            });
        }
    }

    let content = verilog::render_module(module);

    // Write all modules including stubs (matching Python dispatcher behavior)
    let file_path = dest.join(format!("{}.v", module.name()));
    std::fs::write(file_path, content)?;
    Ok(())
}

/// Move `.xci` files into per-stem subdirectories (Vivado requirement).
fn create_xci_sub_folders(dest: &Path) -> Result<(), ExportError> {
    collect_xci_files(dest, dest)
}

/// Create stub files for XCI modules and Xilinx primitives.
///
/// For each `.xci` file found (after relocation), generates a matching `.v`
/// stub with a module declaration derived from the stem name.
/// Also writes bundled Xilinx primitive stubs (LUT6, FDRE, BUFGCE).
fn create_stub_files(dest: &Path) -> Result<(), ExportError> {
    // Generate stubs for XCI files
    generate_xci_stubs(dest, dest)?;

    // Write Xilinx primitive stubs
    for (name, body) in XILINX_PRIMITIVE_STUBS {
        let path = dest.join(format!("{name}.v"));
        std::fs::write(path, body)?;
    }
    Ok(())
}

/// Recursively find `.xci` files and generate matching `.v` stub modules.
fn generate_xci_stubs(dest: &Path, dir: &Path) -> Result<(), ExportError> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Ok(());
    };
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            generate_xci_stubs(dest, &path)?;
        } else if path.extension().is_some_and(|e| e == "xci") {
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                // Generate a simple stub module for this XCI
                let stub = format!("module {stem} ();\nendmodule\n");
                let stub_path = dest.join(format!("{stem}.v"));
                std::fs::write(stub_path, stub)?;
            }
        }
    }
    Ok(())
}

/// Bundled Xilinx primitive stub modules.
const XILINX_PRIMITIVE_STUBS: &[(&str, &str)] = &[
    (
        "LUT6",
        "\
module LUT6 #(
    parameter INIT = 64'h0000000000000000
)(
    input  I0,
    input  I1,
    input  I2,
    input  I3,
    input  I4,
    input  I5,
    output O
);
endmodule
",
    ),
    (
        "FDRE",
        "\
module FDRE #(
    parameter INIT = 1'b0
)(
    input C,
    input CE,
    input D,
    input R,
    output Q
);
endmodule
",
    ),
    (
        "BUFGCE",
        "\
module BUFGCE (
    input  I,
    input  CE,
    output O
);
endmodule
",
    ),
];

/// Recursively find and relocate `.xci` files.
fn collect_xci_files(base: &Path, dir: &Path) -> Result<(), ExportError> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Ok(());
    };
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_xci_files(base, &path)?;
        } else if path.extension().is_some_and(|e| e == "xci") {
            if let (Some(stem), Some(name)) = (path.file_stem(), path.file_name()) {
                let target_dir = base.join(stem);
                std::fs::create_dir_all(&target_dir)?;
                let target = target_dir.join(name);
                if target != path {
                    std::fs::rename(&path, target)?;
                }
            }
        }
    }
    Ok(())
}
