//! Mapping and program-memory reports from an `aiecompiler` graph build.
//!
//! `compiler_report.json` is used instead of the mapping-analysis text: it
//! carries the same placements as numbers and directly associates PLIO
//! logical names and directions with block instance IDs. Program sizes come
//! from each tile's Chess linker map, whose PM section declares 8-bit units.

use std::collections::{BTreeMap, BTreeSet};

use camino::Utf8Path;
use serde::{Deserialize, Serialize};

use crate::error::{Result, XilinxError};

/// The AI Engine resources assigned to a compiled graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AieReport {
    /// Number of distinct core tiles used by AIE kernels.
    pub tiles_used: usize,
    /// Logical PLIO name → its interface placement, ordered by name.
    pub plio_map: BTreeMap<String, AiePlioReport>,
    /// ADF kernel function name → its placement and memory use, ordered by
    /// name.
    pub per_kernel: BTreeMap<String, AieKernelReport>,
}

/// An AI Engine core tile coordinate (`CR(column,row)`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct AieTile {
    pub column: u32,
    pub row: u32,
}

/// A PLIO's interface placement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AiePlioReport {
    /// Interface column (`IO(column)`).
    pub column: u32,
    pub direction: PlioDirection,
}

/// PLIO direction from the graph's perspective.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PlioDirection {
    /// PL → AIE (`pl2aie` in the compiler report).
    Input,
    /// AIE → PL (`aie2pl` in the compiler report).
    Output,
}

/// An AIE kernel instance's placement and memory use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct AieKernelReport {
    pub tile: AieTile,
    /// Used program memory in bytes, from the `Used` count in this tile's
    /// 8-bit-wide PM linker-map section.
    pub program_bytes: u64,
    /// Per-kernel data memory in bytes. The supported compiler reports do
    /// not define this unambiguously, so this is currently always `None`;
    /// stack, heap, stream buffers, and system memory are not substitutes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_bytes: Option<u64>,
}

/// Read a graph report, excluding the reserved PL-control PLIO when present.
pub fn read_report_with_control(graph_dir: &Utf8Path, pl_control: bool) -> Result<AieReport> {
    let reports = graph_dir.join("Work").join("reports");
    let compiler_path = reports.join("compiler_report.json");
    let compiler_text = read_text(&compiler_path)?;
    let compiler: CompilerReport =
        serde_json::from_str(&compiler_text).map_err(|source| XilinxError::AieReportJson {
            path: compiler_path.clone(),
            source,
        })?;
    let (tiles, plio_map, kernels) =
        parse_compiler(compiler, pl_control).map_err(|detail| XilinxError::AieReportParse {
            path: compiler_path,
            detail,
        })?;

    let mut per_kernel = BTreeMap::new();
    for kernel in kernels {
        let tile = format!("{}_{}", kernel.tile.column, kernel.tile.row);
        let pm_path = graph_dir
            .join("Work")
            .join("aie")
            .join(&tile)
            .join("Release")
            .join(format!("{tile}.map"));
        let pm_text = read_text(&pm_path)?;
        let program_bytes =
            parse_program_size(&pm_text).map_err(|detail| XilinxError::AieReportParse {
                path: pm_path,
                detail,
            })?;
        per_kernel.insert(
            kernel.name,
            AieKernelReport {
                tile: kernel.tile,
                program_bytes,
                data_bytes: None,
            },
        );
    }
    Ok(AieReport {
        tiles_used: tiles.len(),
        plio_map,
        per_kernel,
    })
}

fn read_text(path: &Utf8Path) -> Result<String> {
    fs_err::read_to_string(path).map_err(|source| XilinxError::AieReportIo {
        path: path.to_path_buf(),
        source,
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CompilerReport {
    block_types: BTreeMap<String, BlockType>,
    block_instances: BTreeMap<String, BlockInstance>,
    mapping: Mapping,
}

#[derive(Deserialize)]
struct BlockType {
    #[serde(rename = "type")]
    kind: String,
    function: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BlockInstance {
    block_type: String,
    logical_name: Option<String>,
    fabric: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Mapping {
    block_instance_mapping: BTreeMap<String, BlockMapping>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BlockMapping {
    core_info: Option<CoreInfo>,
    plio_info: Option<PlioInfo>,
}

#[derive(Deserialize)]
struct CoreInfo {
    column: u32,
    row: u32,
}

#[derive(Deserialize)]
struct PlioInfo {
    column: u32,
    direction: String,
}

struct KernelPlacement {
    name: String,
    tile: AieTile,
}

type CompilerPieces = (
    BTreeSet<AieTile>,
    BTreeMap<String, AiePlioReport>,
    Vec<KernelPlacement>,
);

fn parse_compiler(
    report: CompilerReport,
    pl_control: bool,
) -> std::result::Result<CompilerPieces, String> {
    let mut tiles = BTreeSet::new();
    let mut kernel_tiles = BTreeMap::new();
    let mut kernels = Vec::new();
    let mut kernel_names = BTreeSet::new();
    let mut plios = BTreeMap::new();
    for (id, instance) in report.block_instances {
        let block_type = report
            .block_types
            .get(&instance.block_type)
            .ok_or_else(|| {
                format!(
                    "block instance `{id}` refers to missing block type `{}`",
                    instance.block_type
                )
            })?;
        match block_type.kind.as_str() {
            "function" => {
                match instance.fabric.as_deref() {
                    Some("me") => {}
                    Some("fpga") => continue,
                    fabric => {
                        return Err(format!(
                            "function instance `{id}` has unknown or missing fabric `{fabric:?}`"
                        ))
                    }
                }
                let name = required_name(block_type.function.clone(), "function", &id)?;
                let mapping = report
                    .mapping
                    .block_instance_mapping
                    .get(&id)
                    .ok_or_else(|| format!("no blockInstanceMapping for kernel instance `{id}`"))?;
                let core = mapping
                    .core_info
                    .as_ref()
                    .ok_or_else(|| format!("no coreInfo for kernel instance `{id}` (`{name}`)"))?;
                let tile = AieTile {
                    column: core.column,
                    row: core.row,
                };
                if let Some(previous) = kernel_tiles.insert(tile, name.clone()) {
                    return Err(format!(
                        "kernels `{previous}` and `{name}` share CR({},{}); per-kernel program \
                         memory is ambiguous",
                        tile.column, tile.row
                    ));
                }
                if !kernel_names.insert(name.clone()) {
                    return Err(format!("duplicate ADF kernel function name `{name}`"));
                }
                tiles.insert(tile);
                kernels.push(KernelPlacement { name, tile });
            }
            "plio" => {
                let name = required_name(instance.logical_name, "logicalName", &id)?;
                if pl_control && name == "tapa_ctrl" {
                    continue;
                }
                let mapping = report
                    .mapping
                    .block_instance_mapping
                    .get(&id)
                    .ok_or_else(|| format!("no blockInstanceMapping for PLIO instance `{id}`"))?;
                let info = mapping
                    .plio_info
                    .as_ref()
                    .ok_or_else(|| format!("no plioInfo for PLIO instance `{id}` (`{name}`)"))?;
                let direction = match info.direction.as_str() {
                    "pl2aie" => PlioDirection::Input,
                    "aie2pl" => PlioDirection::Output,
                    direction => {
                        return Err(format!("PLIO `{name}` has unknown direction `{direction}`"))
                    }
                };
                let plio = AiePlioReport {
                    column: info.column,
                    direction,
                };
                if plios.insert(name.clone(), plio).is_some() {
                    return Err(format!("duplicate logical PLIO name `{name}`"));
                }
            }
            _ => {}
        }
    }
    Ok((tiles, plios, kernels))
}

fn required_name(
    name: Option<String>,
    field: &str,
    id: &str,
) -> std::result::Result<String, String> {
    match name {
        Some(name) if !name.is_empty() => Ok(name),
        _ => Err(format!("block instance `{id}` has no non-empty {field}")),
    }
}

fn parse_program_size(text: &str) -> std::result::Result<u64, String> {
    let mut in_pm = false;
    let mut found_pm = false;
    let mut byte_units = false;
    let mut used = None;
    for line in text.lines().map(str::trim) {
        if line == "Memory map for memory 'PM':" {
            if found_pm {
                return Err("duplicate PM memory map".into());
            }
            found_pm = true;
            in_pm = true;
            continue;
        }
        if in_pm && line.starts_with("Memory map for memory '") {
            break;
        }
        if !in_pm {
            continue;
        }
        let fields = line.split_whitespace().collect::<Vec<_>>();
        match fields.as_slice() {
            ["Width", "=", "8", "bits"] => byte_units = true,
            ["Width", "=", ..] => {
                return Err(format!("PM width is not 8 bits: `{line}`"));
            }
            ["Used", "=", value] => {
                if used.is_some() {
                    return Err("duplicate Used count in PM memory map".into());
                }
                used = Some(
                    value
                        .parse::<u64>()
                        .map_err(|error| format!("invalid PM Used count `{value}`: {error}"))?,
                );
            }
            _ => {}
        }
    }
    if !found_pm {
        return Err("missing PM memory map".into());
    }
    if !byte_units {
        return Err("PM memory map does not declare its 8-bit units".into());
    }
    used.ok_or_else(|| "PM memory map has no Used count".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    const COMPILER: &str = r#"{
      "blockTypes": {
        "z": {"type": "function", "function": "tapa_aie_Z_0"},
        "a": {"type": "function", "function": "tapa_aie_Add_0"},
        "in": {"type": "plio"},
        "out": {"type": "plio"},
        "ps": {"type": "function", "function": "ps_main"}
      },
      "blockInstances": {
        "i9": {"blockType": "z", "fabric": "me"},
        "i8": {"blockType": "out", "logicalName": "c_q"},
        "i0": {"blockType": "a", "fabric": "me"},
        "i7": {"blockType": "in", "logicalName": "a_q"},
        "ps_i10": {"blockType": "ps", "fabric": "fpga"}
      },
      "mapping": {"blockInstanceMapping": {
        "i9": {"coreInfo": {"column": 25, "row": 1}},
        "i8": {"plioInfo": {"column": 24, "direction": "aie2pl"}},
        "i0": {"coreInfo": {"column": 23, "row": 0}},
        "i7": {"plioInfo": {"column": 26, "direction": "pl2aie"}}
      }}
    }"#;

    fn linker_map(program_bytes: u64) -> String {
        format!(
            "Memory map for memory 'DM_bankA':\n  Width = 8 bits\n  Used = 36\n\n\
             Memory map for memory 'PM':\n  Size = 16384\n  Width = 8 bits\n  \
             Used = {program_bytes}\n\nSection summary for memory 'PM':\n"
        )
    }

    fn write_reports(compiler: &str, maps: &[(&str, &str)]) -> tempfile::TempDir {
        let temp = tempfile::tempdir().expect("temporary graph directory");
        let reports = temp.path().join("Work/reports");
        fs_err::create_dir_all(&reports).expect("report directory");
        fs_err::write(reports.join("compiler_report.json"), compiler).expect("compiler report");
        for (tile, map) in maps {
            let release = temp.path().join("Work/aie").join(tile).join("Release");
            fs_err::create_dir_all(&release).expect("tile release directory");
            fs_err::write(release.join(format!("{tile}.map")), map)
                .expect("program-memory linker map");
        }
        temp
    }

    fn graph_dir(temp: &tempfile::TempDir) -> &Utf8Path {
        Utf8Path::from_path(temp.path()).expect("UTF-8 temporary path")
    }

    #[test]
    fn report_associates_names_tiles_directions_and_bytes_deterministically() {
        let temp = write_reports(
            COMPILER,
            &[("25_1", &linker_map(11668)), ("23_0", &linker_map(0))],
        );
        let report = read_report_with_control(graph_dir(&temp), false).expect("parse report");
        assert_eq!(report.tiles_used, 2, "two distinct core tiles");
        assert_eq!(
            report
                .plio_map
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["a_q", "c_q"],
            "logical PLIO names are sorted"
        );
        assert_eq!(
            report.plio_map["a_q"],
            AiePlioReport {
                column: 26,
                direction: PlioDirection::Input,
            }
        );
        assert_eq!(
            report.plio_map["c_q"],
            AiePlioReport {
                column: 24,
                direction: PlioDirection::Output,
            }
        );
        assert_eq!(
            report
                .per_kernel
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["tapa_aie_Add_0", "tapa_aie_Z_0"],
            "kernel names are sorted independently of instance IDs"
        );
        assert_eq!(
            report.per_kernel["tapa_aie_Add_0"],
            AieKernelReport {
                tile: AieTile { column: 23, row: 0 },
                program_bytes: 0,
                data_bytes: None,
            },
            "an explicit zero program size is preserved"
        );
        assert_eq!(
            report.per_kernel["tapa_aie_Z_0"].tile,
            AieTile { column: 25, row: 1 }
        );
        assert_eq!(report.per_kernel["tapa_aie_Z_0"].program_bytes, 11668);
        assert!(report
            .per_kernel
            .values()
            .all(|kernel| kernel.data_bytes.is_none()));
    }

    #[test]
    fn pl_control_report_excludes_only_its_reserved_plio() {
        let compiler = COMPILER
            .replace(
                "\"ps_i10\"",
                "\"ctrl\": {\"blockType\": \"in\", \"logicalName\": \"tapa_ctrl\"},\n        \"ps_i10\"",
            )
            .replace(
                "\"i7\": {\"plioInfo\": {\"column\": 26, \"direction\": \"pl2aie\"}}",
                "\"i7\": {\"plioInfo\": {\"column\": 26, \"direction\": \"pl2aie\"}},\n        \"ctrl\": {\"plioInfo\": {\"column\": 6, \"direction\": \"pl2aie\"}}",
            );
        let temp = write_reports(
            &compiler,
            &[("25_1", &linker_map(11668)), ("23_0", &linker_map(0))],
        );
        let report = read_report_with_control(graph_dir(&temp), true).expect("control report");
        assert!(!report.plio_map.contains_key("tapa_ctrl"));
        assert_eq!(report.plio_map.len(), 2);
        let native = read_report_with_control(graph_dir(&temp), false).expect("native report");
        assert!(native.plio_map.contains_key("tapa_ctrl"));
    }

    #[test]
    fn missing_and_malformed_reports_have_typed_path_aware_errors() {
        let temp = tempfile::tempdir().expect("temporary graph directory");
        let error = read_report_with_control(graph_dir(&temp), false).expect_err("missing compiler report");
        match error {
            XilinxError::AieReportIo { path, .. } => {
                assert!(
                    path.ends_with("Work/reports/compiler_report.json"),
                    "{path}"
                );
            }
            other => panic!("unexpected error: {other}"),
        }

        let temp = write_reports(
            COMPILER,
            &[
                ("25_1", &linker_map(11668).replace("8 bits", "16 bits")),
                ("23_0", &linker_map(0)),
            ],
        );
        let error = read_report_with_control(graph_dir(&temp), false).expect_err("wrong program-memory units");
        match error {
            XilinxError::AieReportParse { path, detail } => {
                assert!(path.ends_with("Work/aie/25_1/Release/25_1.map"), "{path}");
                assert!(detail.contains("not 8 bits"), "{detail}");
            }
            other => panic!("unexpected error: {other}"),
        }

        let temp = write_reports("{not JSON", &[]);
        let error = read_report_with_control(graph_dir(&temp), false).expect_err("malformed JSON");
        assert!(matches!(
            error,
            XilinxError::AieReportJson { path, .. }
                if path.ends_with("Work/reports/compiler_report.json")
        ));
    }
}
