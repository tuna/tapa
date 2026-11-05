//! Top-task `report.json` / `report.yaml` emitter.
//!
//! Synth writes the report after RTL codegen so downstream pack flows
//! (xilinx-vitis `.xo` and xilinx-hls `.zip`) can include it as
//! `report.yaml` at archive root and `report.json` alongside.
//!
//! Schema:
//! ```yaml
//! schema: <tapa version>
//! name: <top task name>
//! performance:
//!   source: hls
//!   clock_period: "<nanoseconds>"   # canonical decimal, e.g. "3.33"
//!   critical_path:        # only when top is upper
//!     <child_task_name>: { ...child performance dict... }
//! area:
//!   source: hls            # "synth" when total_area has non-zero utilization
//!   total: { ...resource dict... }
//!   breakdown:             # only when top is upper
//!     <child_task_name>: { count: <n>, area: { ... } }
//! aie:                     # after a hardware AI Engine build; `tapa aiesim`
//!                          # creates the block for the section below
//!   tiles_used: <n>
//!   plio_map:
//!     <logical_plio_name>:
//!       direction: input   # PL→AIE; output means AIE→PL
//!       column: <interface column>
//!       width_bits: <32|64|128>
//!       channels: <64-bit channels consumed>
//!   per_kernel:
//!     <full_adf_kernel_function_name>:
//!       tile: { column: <n>, row: <n> }
//!       program_bytes: <n>
//!   aiesim:                # folded in by `tapa aiesim --cycle-accurate`
//!     streams:             # one entry per measured boundary FIFO
//!       <fifo>:
//!         beats: <n>
//!         first_beat_ns: <f>
//!         last_beat_ns: <f>
//!         mean_beat_spacing_ns: <f>   # omitted with fewer than two beats
//! ```

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use indexmap::IndexMap;
use serde::Serialize;
use serde_json::Value;
use tapa_ir::{AiePartition, AieStreamKind, ClockPeriod, Design, TaskLevel};
use tapa_xilinx::tools::aie::report::{AieReport, PlioDirection};

use crate::error::{CliError, Result};
use crate::steps::version::VERSION as TAPA_VERSION;

use super::metrics::{effective_total_area, instantiated_children};

/// Typed report schema mirroring the on-disk format.
#[derive(Debug, Clone, Serialize)]
struct Report {
    schema: String,
    name: String,
    performance: Performance,
    area: AreaSection,
    #[serde(skip_serializing_if = "Option::is_none")]
    aie: Option<AieSection>,
}

/// The hardware AI Engine mapping, enriched with the partition's PLIO widths.
#[derive(Debug, Clone, Serialize)]
struct AieSection {
    tiles_used: usize,
    plio_map: BTreeMap<String, AiePlioSection>,
    per_kernel: BTreeMap<String, tapa_xilinx::tools::aie::report::AieKernelReport>,
}

#[derive(Debug, Clone, Serialize)]
struct AiePlioSection {
    direction: PlioDirection,
    column: u32,
    width_bits: u32,
    channels: u64,
}

/// Where a published number came from. Serializes as the wire strings the
/// report schema has always used.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum MetricSource {
    /// The HLS estimate.
    Hls,
    /// A measured post-synthesis value.
    Synth,
}

#[derive(Debug, Clone, Serialize)]
struct Performance {
    source: MetricSource,
    /// Published as decimal nanoseconds; the schema is a document, so the
    /// reading stays the text a user recognizes.
    #[serde(serialize_with = "serialize_nanoseconds", rename = "clock_period")]
    clock: ClockPeriod,
    #[serde(skip_serializing_if = "Option::is_none")]
    critical_path: Option<IndexMap<String, Self>>,
}

#[allow(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde's serialize_with contract takes the field by reference"
)]
fn serialize_nanoseconds<S: serde::Serializer>(
    clock: &ClockPeriod,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error> {
    serializer.serialize_str(&clock.to_string())
}

/// The `area` section of a task's published report: where the numbers came
/// from, the totals, and the per-child breakdown. [`tapa_ir::Area`] is the
/// count model behind `total`.
#[derive(Debug, Clone, Serialize)]
struct AreaSection {
    source: MetricSource,
    total: IndexMap<String, Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    breakdown: Option<IndexMap<String, BreakdownEntry>>,
}

#[derive(Debug, Clone, Serialize)]
struct BreakdownEntry {
    count: usize,
    area: AreaSection,
}

struct ChildReport<'a> {
    name: &'a str,
    count: usize,
    clock: ClockPeriod,
    report: Report,
}

/// Write `<work_dir>/report.{json,yaml}` for the design's top task.
/// `override_schema` (mirrors `--override-report-schema-version`) wins
/// over the baked `VERSION` constant when non-empty.
pub fn write_top_report(
    work_dir: &Path,
    design: &Design,
    override_schema: &str,
    aie: Option<(&AieReport, &AiePartition)>,
) -> Result<()> {
    let schema = if override_schema.is_empty() {
        TAPA_VERSION
    } else {
        override_schema
    };
    let mut report = build_task_report(design, &design.top, schema)?;
    if let Some((aie_report, partition)) = aie {
        let section = build_aie_section(aie_report, partition)?;
        check_aie1_channel_limits(&section)?;
        report.aie = Some(section);
    }

    let json_path = work_dir.join("report.json");
    let json_str = serde_json::to_string_pretty(&report)
        .map_err(|e| CliError::Report(format!("cannot serialize report.json: {e}")))?;
    fs::write(&json_path, json_str)?;

    let yaml_path = work_dir.join("report.yaml");
    let yaml_str = serde_yaml::to_string(&report)
        .map_err(|e| CliError::Report(format!("cannot serialize report.yaml: {e}")))?;
    fs::write(&yaml_path, yaml_str)?;
    Ok(())
}

/// Timing one output boundary stream as measured by `aiesimulator`,
/// folded into an existing root report after a fully matching
/// `tapa aiesim --cycle-accurate` run.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct AiesimStreamTiming {
    pub(crate) beats: usize,
    pub(crate) first_beat_ns: f64,
    pub(crate) last_beat_ns: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) mean_beat_spacing_ns: Option<f64>,
}

impl AiesimStreamTiming {
    /// Measure one output PLIO's parsed beat times. A stream the simulator
    /// never timestamped has nothing to report.
    pub fn from_beat_times(beat_times_ns: &[f64]) -> Option<Self> {
        let (&first, &last) = (beat_times_ns.first()?, beat_times_ns.last()?);
        let gaps = u32::try_from(beat_times_ns.len() - 1).unwrap_or(u32::MAX);
        Some(Self {
            beats: beat_times_ns.len(),
            first_beat_ns: first,
            last_beat_ns: last,
            // A lone beat has no interval to average; inventing 0.0 would
            // read as a measurement.
            mean_beat_spacing_ns: (gaps > 0).then(|| (last - first) / f64::from(gaps)),
        })
    }
}

/// The `aie.aiesim` report section: per-FIFO simulator timing, keyed by
/// boundary FIFO name in deterministic order.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct AiesimSection {
    pub streams: BTreeMap<String, AiesimStreamTiming>,
}

/// Fold AI Engine simulator stream timing into the root reports that exist.
///
/// `tapa aiesim` is documented to run after `analyze` alone, so a work dir
/// with no root report is a successful no-op: nothing is created and the
/// run's console output stands. Each of `report.json` / `report.yaml` that
/// does exist gains the section with every other field preserved — one
/// side may be absent without failing the run — while a malformed document
/// or a non-object `aie` block is an error naming the file and path rather
/// than a silent overwrite. Returns the file names updated.
pub fn fold_aiesim_timing(work_dir: &Path, timing: &AiesimSection) -> Result<Vec<String>> {
    let mut updated = Vec::new();
    let json_path = work_dir.join("report.json");
    if json_path.exists() {
        fold_report(&json_path, timing, ReportFormat::Json)?;
        updated.push("report.json".to_string());
    }
    let yaml_path = work_dir.join("report.yaml");
    if yaml_path.exists() {
        fold_report(&yaml_path, timing, ReportFormat::Yaml)?;
        updated.push("report.yaml".to_string());
    }
    Ok(updated)
}

fn not_foldable(path: &Path, reason: &str) -> CliError {
    CliError::Report(format!(
        "cannot fold aiesim timing into `{}` under `aie.aiesim`: {reason}",
        path.display()
    ))
}

/// The on-disk syntax of a root report.
#[derive(Clone, Copy)]
enum ReportFormat {
    Json,
    Yaml,
}

/// Fold `timing` under `aie.aiesim` in the report at `path`, preserving
/// every other field, in one read → parse → insert → serialize → write
/// flow for both syntaxes.
///
/// YAML folds through the order-preserving `serde_json::Value` too, so the
/// insert logic exists once; a YAML mapping with non-string keys fails the
/// conversion and is reported as a parse failure of the file.
fn fold_report(path: &Path, timing: &AiesimSection, format: ReportFormat) -> Result<()> {
    let text = fs::read_to_string(path)
        .map_err(|e| CliError::Report(format!("cannot read `{}`: {e}", path.display())))?;
    let mut root: Value = parse_report(&text, format)
        .map_err(|e| CliError::Report(format!("cannot parse `{}`: {e}", path.display())))?;
    let section = serde_json::to_value(timing)
        .map_err(|e| CliError::Report(format!("cannot serialize the aiesim section: {e}")))?;
    set_aiesim(&mut root, path, section)?;
    let rewritten = serialize_report(&root, format)
        .map_err(|e| CliError::Report(format!("cannot serialize `{}`: {e}", path.display())))?;
    fs::write(path, rewritten)?;
    Ok(())
}

fn parse_report(text: &str, format: ReportFormat) -> std::result::Result<Value, String> {
    match format {
        ReportFormat::Json => serde_json::from_str(text).map_err(|e| e.to_string()),
        ReportFormat::Yaml => serde_yaml::from_str::<serde_yaml::Value>(text)
            .map_err(|e| e.to_string())
            .and_then(|yaml| serde_json::to_value(&yaml).map_err(|e| e.to_string())),
    }
}

fn serialize_report(root: &Value, format: ReportFormat) -> std::result::Result<String, String> {
    match format {
        ReportFormat::Json => serde_json::to_string_pretty(root).map_err(|e| e.to_string()),
        ReportFormat::Yaml => serde_yaml::to_string(root).map_err(|e| e.to_string()),
    }
}

/// Place `section` at `aie.aiesim`, creating `aie` when the report has none.
fn set_aiesim(root: &mut Value, path: &Path, section: Value) -> Result<()> {
    let Value::Object(root_fields) = root else {
        return Err(not_foldable(path, "the document root is not an object"));
    };
    let aie = root_fields
        .entry("aie")
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
    let Value::Object(aie_fields) = aie else {
        return Err(not_foldable(path, "`aie` is not an object"));
    };
    aie_fields.insert("aiesim".to_string(), section);
    Ok(())
}

/// Recursively build a task report.
///
/// `Task::clock_period` and `total_area` contain the task-local HLS
/// estimate and an optional post-synthesis override, respectively. The
/// effective values are derived here: an upper task's clock is the maximum
/// of its own estimate and all descendants, while an empty `total_area` is
/// computed as `self_area` plus every child instance's effective total.
fn build_task_report(design: &Design, task_name: &str, schema: &str) -> Result<Report> {
    let task = design
        .tasks
        .get(task_name)
        .ok_or_else(|| CliError::Report(format!("task `{task_name}` not found in the design")))?;

    let area_source = if task.total_area.is_some() {
        MetricSource::Synth
    } else {
        MetricSource::Hls
    };

    let mut child_reports = Vec::new();
    for (child_name, count) in instantiated_children(design, task) {
        let report = build_task_report(design, child_name, schema)?;
        child_reports.push(ChildReport {
            name: child_name,
            count,
            clock: report.performance.clock,
            report,
        });
    }

    // A task with no HLS estimate has no measured period: either `synth`
    // has not run yet, or it is a `synth: ignore` task that HLS skips
    // entirely. Such a task contributes zero to the critical path rather
    // than failing the whole report.
    let clock = child_reports
        .iter()
        .map(|child| child.clock)
        .chain(task.clock_period)
        .max()
        .unwrap_or(ClockPeriod::ZERO);

    let total_area = published_area(effective_total_area(design, task_name)?);

    let (performance, area_breakdown) = if task.level == TaskLevel::Upper {
        let mut critical_path = IndexMap::new();
        let mut breakdown = IndexMap::new();
        for child in child_reports {
            if child.clock == clock {
                critical_path.insert(child.name.to_string(), child.report.performance);
            }
            breakdown.insert(
                child.name.to_string(),
                BreakdownEntry {
                    count: child.count,
                    area: child.report.area,
                },
            );
        }
        (
            Performance {
                source: MetricSource::Hls,
                clock,
                critical_path: Some(critical_path),
            },
            Some(breakdown),
        )
    } else {
        (
            Performance {
                source: MetricSource::Hls,
                clock,
                critical_path: None,
            },
            None,
        )
    };

    Ok(Report {
        schema: schema.to_string(),
        name: task_name.to_string(),
        performance,
        area: AreaSection {
            source: area_source,
            total: total_area,
            breakdown: area_breakdown,
        },
        aie: None,
    })
}

/// Join the compiler's logical PLIO placements to the typed partition's
/// widths and directions. Neither side is allowed to have unmatched PLIOs.
fn build_aie_section(report: &AieReport, partition: &AiePartition) -> Result<AieSection> {
    let mut partition_plios = BTreeMap::new();
    for stream in &partition.streams {
        let AieStreamKind::Plio { input, width } = stream.kind else {
            continue;
        };
        if partition_plios
            .insert(stream.name.as_str(), (input, width))
            .is_some()
        {
            return Err(CliError::Report(format!(
                "duplicate logical PLIO `{}` in the AI Engine partition",
                stream.name
            )));
        }
    }
    for name in partition_plios.keys() {
        if !report.plio_map.contains_key(*name) {
            return Err(CliError::Report(format!(
                "AI Engine mapping report is missing logical PLIO `{name}`"
            )));
        }
    }

    let mut plio_map = BTreeMap::new();
    for (name, plio) in &report.plio_map {
        let Some(&(input, width_bits)) = partition_plios.get(name.as_str()) else {
            return Err(CliError::Report(format!(
                "AI Engine mapping report has logical PLIO `{name}`, but the partition does not"
            )));
        };
        let direction = if input {
            PlioDirection::Input
        } else {
            PlioDirection::Output
        };
        if plio.direction != direction {
            return Err(CliError::Report(format!(
                "logical PLIO `{name}` direction mismatch: partition has {}, mapping report has {}",
                direction_name(direction),
                direction_name(plio.direction)
            )));
        }
        let channels = match width_bits {
            32 | 64 => 1,
            128 => 2,
            width => {
                return Err(CliError::Report(format!(
                "logical PLIO `{name}` has unsupported width {width} bits; expected 32, 64, or 128"
            )))
            }
        };
        plio_map.insert(
            name.clone(),
            AiePlioSection {
                direction,
                column: plio.column,
                width_bits,
                channels,
            },
        );
    }
    Ok(AieSection {
        tiles_used: report.tiles_used,
        plio_map,
        per_kernel: report.per_kernel.clone(),
    })
}

fn direction_name(direction: PlioDirection) -> &'static str {
    match direction {
        PlioDirection::Input => "input (PL→AIE)",
        PlioDirection::Output => "output (AIE→PL)",
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
struct Aie1ChannelUsage {
    input: u64,
    output: u64,
}

/// Aggregate 64-bit interface channels by column, in deterministic column
/// order. A 128-bit PLIO occupies two channels.
fn aie1_channel_usage(section: &AieSection) -> BTreeMap<u32, Aie1ChannelUsage> {
    let mut columns = BTreeMap::new();
    for plio in section.plio_map.values() {
        let usage: &mut Aie1ChannelUsage = columns.entry(plio.column).or_default();
        match plio.direction {
            PlioDirection::Input => usage.input += plio.channels,
            PlioDirection::Output => usage.output += plio.channels,
        }
    }
    columns
}

/// AIE1 interface tiles provide 8 PL→AIE and 6 AIE→PL 64-bit channels.
fn check_aie1_channel_limits(section: &AieSection) -> Result<()> {
    const INPUT_LIMIT: u64 = 8;
    const OUTPUT_LIMIT: u64 = 6;
    for (column, usage) in aie1_channel_usage(section) {
        log::info!(
            "AI Engine interface column {column}: {}/{} PL→AIE and {}/{} AIE→PL 64-bit channels",
            usage.input,
            INPUT_LIMIT,
            usage.output,
            OUTPUT_LIMIT
        );
        if usage.input > INPUT_LIMIT {
            return Err(CliError::Report(format!(
                "AI Engine interface column {column} uses {} PL→AIE (input) 64-bit channels, exceeding the AIE1 limit of {INPUT_LIMIT}",
                usage.input
            )));
        }
        if usage.output > OUTPUT_LIMIT {
            return Err(CliError::Report(format!(
                "AI Engine interface column {column} uses {} AIE→PL (output) 64-bit channels, exceeding the AIE1 limit of {OUTPUT_LIMIT}",
                usage.output
            )));
        }
    }
    Ok(())
}

/// Render a typed area into the report's published resource map.
///
/// The report schema is a user-facing document, so the keys and their order
/// stay what they have always been; the typed [`tapa_ir::Area`] is the
/// single internal model behind it. An unannotated task publishes an empty
/// map, as it did when the annotation was missing.
fn published_area(area: Option<tapa_ir::Area>) -> IndexMap<String, Value> {
    let Some(area) = area else {
        return IndexMap::new();
    };
    IndexMap::from([
        ("BRAM_18K".to_string(), Value::from(area.bram_18k)),
        ("DSP".to_string(), Value::from(area.dsp)),
        ("FF".to_string(), Value::from(area.ff)),
        ("LUT".to_string(), Value::from(area.lut)),
        ("URAM".to_string(), Value::from(area.uram)),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    use tapa_ir::{AieStream, SynthTarget, Task, TaskInstance, TaskLevel};
    use tapa_xilinx::tools::aie::report::{AieKernelReport, AiePlioReport, AieTile};

    use crate::steps::synth::fixtures;
    use crate::testutil::task;

    fn leaf(name: &str, clock: Option<ClockPeriod>, area: Option<tapa_ir::Area>) -> Task {
        Task {
            total_area: area,
            clock_period: clock,
            ..task(name)
        }
    }

    fn derived_task(
        name: &str,
        clock: Option<ClockPeriod>,
        self_area: tapa_ir::Area,
        tasks: BTreeMap<String, Vec<TaskInstance>>,
    ) -> Task {
        Task {
            level: if tasks.is_empty() {
                TaskLevel::Lower
            } else {
                TaskLevel::Upper
            },
            tasks,
            self_area: Some(self_area),
            clock_period: clock,
            ..task(name)
        }
    }

    fn plio_stream(name: &str, input: bool, width: u32) -> AieStream {
        AieStream {
            name: name.to_string(),
            fifo: name.to_string(),
            words: 1,
            frame: None,
            kind: AieStreamKind::Plio { input, width },
        }
    }

    /// Three PLIOs share a column with both directions and all channel
    /// counts: 32-bit input (1), 128-bit input (2), and 64-bit output (1).
    fn aie_fixture() -> (AieReport, AiePartition) {
        let report = AieReport {
            tiles_used: 1,
            plio_map: BTreeMap::from([
                (
                    "a_q".to_string(),
                    AiePlioReport {
                        column: 7,
                        direction: PlioDirection::Input,
                    },
                ),
                (
                    "b_q".to_string(),
                    AiePlioReport {
                        column: 7,
                        direction: PlioDirection::Input,
                    },
                ),
                (
                    "c_q".to_string(),
                    AiePlioReport {
                        column: 7,
                        direction: PlioDirection::Output,
                    },
                ),
            ]),
            per_kernel: BTreeMap::from([(
                "tapa_aie_Add_0".to_string(),
                AieKernelReport {
                    tile: AieTile { column: 23, row: 2 },
                    program_bytes: 4096,
                },
            )]),
        };
        let partition = AiePartition {
            kernels: Vec::new(),
            streams: vec![
                plio_stream("a_q", true, 32),
                plio_stream("b_q", true, 128),
                plio_stream("c_q", false, 64),
            ],
            run_forever: false,
        };
        (report, partition)
    }

    #[test]
    fn aie_json_has_widths_and_channels_only_at_root() {
        let dir = tempfile::tempdir().expect("tempdir");
        let design = Design {
            schema_version: tapa_ir::graph::SCHEMA_VERSION,
            top: "Top".to_string(),
            target: tapa_ir::Target::XilinxVitis,
            tasks: BTreeMap::from([
                (
                    "Top".to_string(),
                    derived_task(
                        "Top",
                        Some(ClockPeriod::from_picoseconds(2000)),
                        tapa_ir::Area::default(),
                        BTreeMap::from([("Add".to_string(), fixtures::instances(1))]),
                    ),
                ),
                (
                    "Add".to_string(),
                    leaf("Add", Some(ClockPeriod::from_picoseconds(2000)), None),
                ),
            ]),
            cflags: Vec::new(),
        };
        let (aie_report, partition) = aie_fixture();

        write_top_report(dir.path(), &design, "", Some((&aie_report, &partition)))
            .expect("write AIE report");
        let parsed: Value = serde_json::from_str(
            &fs::read_to_string(dir.path().join("report.json")).expect("read report"),
        )
        .expect("valid report JSON");
        assert_eq!(
            parsed["aie"],
            serde_json::json!({
                "tiles_used": 1,
                "plio_map": {
                    "a_q": {
                        "direction": "input",
                        "column": 7,
                        "width_bits": 32,
                        "channels": 1
                    },
                    "b_q": {
                        "direction": "input",
                        "column": 7,
                        "width_bits": 128,
                        "channels": 2
                    },
                    "c_q": {
                        "direction": "output",
                        "column": 7,
                        "width_bits": 64,
                        "channels": 1
                    }
                },
                "per_kernel": {
                    "tapa_aie_Add_0": {
                        "tile": { "column": 23, "row": 2 },
                        "program_bytes": 4096
                    }
                }
            })
        );
        assert!(parsed["performance"]["critical_path"]["Add"]
            .get("aie")
            .is_none());
        assert!(parsed["area"]["breakdown"]["Add"]["area"]
            .get("aie")
            .is_none());

        let yaml: serde_yaml::Value = serde_yaml::from_str(
            &fs::read_to_string(dir.path().join("report.yaml")).expect("read YAML"),
        )
        .expect("valid report YAML");
        assert_eq!(yaml["aie"]["tiles_used"], 1);
        assert_eq!(yaml["aie"]["plio_map"]["b_q"]["channels"], 2);
    }

    #[test]
    fn plio_width_join_and_mixed_column_accounting_use_64_bit_channels() {
        let (report, partition) = aie_fixture();
        let section = build_aie_section(&report, &partition).expect("join PLIOs");
        assert_eq!(section.plio_map["a_q"].width_bits, 32);
        assert_eq!(section.plio_map["a_q"].channels, 1);
        assert_eq!(section.plio_map["b_q"].width_bits, 128);
        assert_eq!(section.plio_map["b_q"].channels, 2);
        assert_eq!(section.plio_map["c_q"].width_bits, 64);
        assert_eq!(section.plio_map["c_q"].channels, 1);
        assert_eq!(
            aie1_channel_usage(&section),
            BTreeMap::from([(
                7,
                Aie1ChannelUsage {
                    input: 3,
                    output: 1,
                },
            )])
        );
        check_aie1_channel_limits(&section).expect("usage is within AIE1 limits");
    }

    #[test]
    fn aie_join_rejects_missing_duplicate_and_mismatched_plios() {
        let (mut report, partition) = aie_fixture();
        report.plio_map.remove("a_q");
        let error = build_aie_section(&report, &partition).expect_err("missing report PLIO");
        assert!(error.to_string().contains("missing logical PLIO `a_q`"));

        let (report, mut partition) = aie_fixture();
        partition.streams.push(partition.streams[0].clone());
        let error = build_aie_section(&report, &partition).expect_err("duplicate partition PLIO");
        assert!(error.to_string().contains("duplicate logical PLIO `a_q`"));

        let (mut report, partition) = aie_fixture();
        report.plio_map.get_mut("a_q").expect("a_q").direction = PlioDirection::Output;
        let error = build_aie_section(&report, &partition).expect_err("direction mismatch");
        assert!(error
            .to_string()
            .contains("logical PLIO `a_q` direction mismatch"));
    }

    #[test]
    fn aie1_channel_overflow_identifies_column_direction_and_usage() {
        let section = AieSection {
            tiles_used: 0,
            plio_map: (0..7)
                .map(|index| {
                    (
                        format!("out_{index}"),
                        AiePlioSection {
                            direction: PlioDirection::Output,
                            column: 24,
                            width_bits: 64,
                            channels: 1,
                        },
                    )
                })
                .collect(),
            per_kernel: BTreeMap::new(),
        };
        let error = check_aie1_channel_limits(&section).expect_err("seven outputs exceed six");
        let message = error.to_string();
        assert!(message.contains("interface column 24"), "{message}");
        assert!(message.contains("AIE→PL (output)"), "{message}");
        assert!(message.contains("uses 7"), "{message}");
        assert!(message.contains("limit of 6"), "{message}");
    }

    /// `synth: ignore` tasks are skipped by HLS, so they reach the report
    /// with an empty `clock_period` annotation. That must read as zero (the
    /// value `analyze` used to seed) instead of failing the whole report.
    #[test]
    fn unsynthesized_task_clock_period_reads_as_zero() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut custom_rtl = leaf(
            "Custom",
            None,
            Some(tapa_ir::Area {
                lut: 7,
                ..tapa_ir::Area::default()
            }),
        );
        custom_rtl.synth = SynthTarget::Ignore;
        let tasks = BTreeMap::from([
            ("Custom".to_string(), custom_rtl),
            (
                "Top".to_string(),
                Task {
                    level: TaskLevel::Upper,
                    tasks: BTreeMap::from([("Custom".to_string(), fixtures::instances(1))]),
                    clock_period: Some(ClockPeriod::from_picoseconds(2500)),
                    ..task("Top")
                },
            ),
        ]);
        let design = Design {
            schema_version: tapa_ir::graph::SCHEMA_VERSION,
            top: "Top".to_string(),
            target: tapa_ir::Target::XilinxHls,
            tasks,
            cflags: Vec::new(),
        };

        write_top_report(dir.path(), &design, "", None)
            .expect("report must not fail on empty clock");
        let parsed: Value = serde_json::from_str(
            &fs::read_to_string(dir.path().join("report.json")).expect("read report"),
        )
        .expect("valid report json");

        // The ignored child contributes 0, so the top's own 2.5 wins.
        assert_eq!(parsed["performance"]["clock_period"], "2.5");
        assert_eq!(
            parsed["area"]["breakdown"]["Custom"]["area"]["total"]["LUT"], 7,
            "an unsynthesized task still reports its area"
        );
    }

    #[test]
    fn derives_recursive_metrics_when_total_area_is_empty() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut tasks = BTreeMap::new();
        tasks.insert(
            "Leaf".to_string(),
            derived_task(
                "Leaf",
                Some(ClockPeriod::from_picoseconds(4000)),
                tapa_ir::Area {
                    lut: 11,
                    ff: 1,
                    ..tapa_ir::Area::default()
                },
                BTreeMap::new(),
            ),
        );
        tasks.insert(
            "Middle".to_string(),
            derived_task(
                "Middle",
                Some(ClockPeriod::from_picoseconds(2500)),
                tapa_ir::Area {
                    lut: 7,
                    ff: 2,
                    ..tapa_ir::Area::default()
                },
                BTreeMap::from([("Leaf".to_string(), fixtures::instances(3))]),
            ),
        );
        tasks.insert(
            "Top".to_string(),
            derived_task(
                "Top",
                Some(ClockPeriod::from_picoseconds(2000)),
                tapa_ir::Area {
                    lut: 5,
                    ff: 3,
                    ..tapa_ir::Area::default()
                },
                BTreeMap::from([("Middle".to_string(), fixtures::instances(2))]),
            ),
        );
        let design = Design {
            schema_version: tapa_ir::graph::SCHEMA_VERSION,
            top: "Top".to_string(),
            target: tapa_ir::Target::XilinxHls,
            tasks,
            cflags: Vec::new(),
        };

        write_top_report(dir.path(), &design, "", None).expect("write report");
        let parsed: Value = serde_json::from_str(
            &fs::read_to_string(dir.path().join("report.json")).expect("read report"),
        )
        .expect("valid report json");

        // Published in canonical nanoseconds: `4`, not the `4.0` a
        // tool happened to print.
        assert_eq!(parsed["performance"]["clock_period"], "4");
        assert_eq!(
            parsed["performance"]["critical_path"]["Middle"]["clock_period"],
            "4"
        );
        assert_eq!(
            parsed["performance"]["critical_path"]["Middle"]["critical_path"]["Leaf"]
                ["clock_period"],
            "4"
        );
        assert_eq!(parsed["area"]["source"], "hls");
        assert_eq!(parsed["area"]["total"]["LUT"], 85);
        assert_eq!(parsed["area"]["total"]["FF"], 13);
        assert_eq!(parsed["area"]["breakdown"]["Middle"]["count"], 2);
        assert_eq!(
            parsed["area"]["breakdown"]["Middle"]["area"]["total"]["LUT"],
            40
        );
        assert_eq!(
            parsed["area"]["breakdown"]["Middle"]["area"]["breakdown"]["Leaf"]["count"],
            3
        );
        assert_eq!(
            parsed["area"]["breakdown"]["Middle"]["area"]["breakdown"]["Leaf"]["area"]["total"]
                ["LUT"],
            11
        );
    }

    #[test]
    fn writes_report_for_upper_top_with_breakdown() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut tasks = BTreeMap::new();
        tasks.insert(
            "VecAdd".to_string(),
            Task {
                level: TaskLevel::Upper,
                tasks: BTreeMap::from([("Add".to_string(), fixtures::instances(2))]),
                total_area: Some(tapa_ir::Area {
                    lut: 100,
                    ..tapa_ir::Area::default()
                }),
                clock_period: Some(ClockPeriod::from_picoseconds(3330)),
                ..task("VecAdd")
            },
        );
        tasks.insert(
            "Add".to_string(),
            leaf(
                "Add",
                Some(ClockPeriod::from_picoseconds(3330)),
                Some(tapa_ir::Area {
                    lut: 50,
                    ..tapa_ir::Area::default()
                }),
            ),
        );
        let design = Design {
            schema_version: tapa_ir::graph::SCHEMA_VERSION,
            top: "VecAdd".to_string(),
            target: tapa_ir::Target::XilinxVitis,
            tasks,
            cflags: Vec::new(),
        };
        write_top_report(dir.path(), &design, "", None).expect("write report");
        let yaml = fs::read_to_string(dir.path().join("report.yaml")).expect("read yaml");
        assert!(yaml.contains("name: VecAdd"));
        assert!(yaml.contains("breakdown:"));
        assert!(yaml.contains("Add:"));
        assert!(
            yaml.contains("count: 2"),
            "report missing breakdown count: {yaml}"
        );
        let json_str = fs::read_to_string(dir.path().join("report.json")).expect("read json");
        let parsed: Value = serde_json::from_str(&json_str).expect("valid json");
        assert_eq!(parsed["name"], "VecAdd");
        assert_eq!(parsed["area"]["source"], "synth"); // total_area populated
        assert_eq!(parsed["performance"]["clock_period"], "3.33");
        assert_eq!(parsed["area"]["breakdown"]["Add"]["count"], 2);
        assert!(
            parsed.get("aie").is_none(),
            "PL-only report has no AIE block"
        );
        assert!(!yaml.contains("\naie:"), "PL-only YAML has no AIE block");
    }

    #[test]
    fn override_schema_wins() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut tasks = BTreeMap::new();
        tasks.insert(
            "T".to_string(),
            leaf("T", Some(ClockPeriod::from_picoseconds(3330)), None),
        );
        let design = Design {
            schema_version: tapa_ir::graph::SCHEMA_VERSION,
            top: "T".to_string(),
            target: tapa_ir::Target::XilinxHls,
            tasks,
            cflags: Vec::new(),
        };
        write_top_report(dir.path(), &design, "9.9.9-override", None).expect("write");
        let parsed: Value = serde_json::from_str(
            &fs::read_to_string(dir.path().join("report.json")).expect("read"),
        )
        .expect("json");
        assert_eq!(parsed["schema"], "9.9.9-override");
        assert_eq!(parsed["area"]["source"], "hls"); // empty total_area
    }

    /// Two output streams: one with three beats, one with a single beat
    /// that must publish no spacing.
    fn aiesim_timing_fixture() -> AiesimSection {
        AiesimSection {
            streams: BTreeMap::from([
                (
                    "c_q".to_string(),
                    AiesimStreamTiming::from_beat_times(&[240.0, 256.0, 272.0])
                        .expect("three beats"),
                ),
                (
                    "d_q".to_string(),
                    AiesimStreamTiming::from_beat_times(&[100.0]).expect("one beat"),
                ),
            ]),
        }
    }

    #[test]
    fn fold_preserves_existing_fields_and_matches_across_formats() {
        let dir = tempfile::tempdir().expect("tempdir");
        let report = r#"{
  "schema": "9.9.9-override",
  "name": "Top",
  "extra": { "keep": true },
  "aie": {
    "tiles_used": 1,
    "plio_map": {
      "c_q": { "direction": "output", "column": 7, "width_bits": 64, "channels": 1 }
    },
    "per_kernel": {},
    "aie_note": "keep me too"
  }
}"#;
        fs::write(dir.path().join("report.json"), report).expect("write json");
        fs::write(
            dir.path().join("report.yaml"),
            "schema: 9.9.9-override\naie:\n  tiles_used: 1\n  aie_note: keep me too\n",
        )
        .expect("write yaml");

        let updated =
            fold_aiesim_timing(dir.path(), &aiesim_timing_fixture()).expect("fold reports");
        assert_eq!(updated, ["report.json", "report.yaml"]);

        let json: Value = serde_json::from_str(
            &fs::read_to_string(dir.path().join("report.json")).expect("read json"),
        )
        .expect("valid json");
        // Everything that was there survives, including the schema override
        // and the hardware mapping the fold nests under.
        assert_eq!(json["schema"], "9.9.9-override");
        assert_eq!(json["name"], "Top");
        assert_eq!(json["extra"]["keep"], true);
        assert_eq!(json["aie"]["tiles_used"], 1);
        assert_eq!(json["aie"]["plio_map"]["c_q"]["direction"], "output");
        assert_eq!(json["aie"]["aie_note"], "keep me too");
        assert_eq!(
            json["aie"]["aiesim"]["streams"]["c_q"],
            serde_json::json!({
                "beats": 3,
                "first_beat_ns": 240.0,
                "last_beat_ns": 272.0,
                "mean_beat_spacing_ns": 16.0,
            }),
            "spacing is (last - first) / (beats - 1)"
        );
        assert_eq!(json["aie"]["aiesim"]["streams"]["d_q"]["beats"], 1);
        assert!(
            json["aie"]["aiesim"]["streams"]["d_q"]
                .get("mean_beat_spacing_ns")
                .is_none(),
            "a single beat has no spacing to report"
        );

        let yaml: serde_yaml::Value = serde_yaml::from_str(
            &fs::read_to_string(dir.path().join("report.yaml")).expect("read yaml"),
        )
        .expect("valid yaml");
        assert_eq!(yaml["schema"], "9.9.9-override");
        assert_eq!(yaml["aie"]["tiles_used"], 1);
        assert_eq!(
            serde_json::to_value(&yaml["aie"]["aiesim"]).expect("yaml measurements"),
            json["aie"]["aiesim"],
            "JSON and YAML gain the same measurements"
        );
    }

    #[test]
    fn fold_creates_a_missing_aie_block_and_tolerates_a_missing_side() {
        let timing = aiesim_timing_fixture();

        let json_only = tempfile::tempdir().expect("tempdir");
        fs::write(
            json_only.path().join("report.json"),
            r#"{"schema": "1", "name": "T"}"#,
        )
        .expect("write json");
        let updated = fold_aiesim_timing(json_only.path(), &timing).expect("fold");
        assert_eq!(updated, ["report.json"], "no YAML side is not an error");
        let json: Value = serde_json::from_str(
            &fs::read_to_string(json_only.path().join("report.json")).expect("read json"),
        )
        .expect("valid json");
        assert_eq!(json["schema"], "1");
        assert_eq!(json["aie"]["aiesim"]["streams"]["c_q"]["beats"], 3);

        let yaml_only = tempfile::tempdir().expect("tempdir");
        fs::write(yaml_only.path().join("report.yaml"), "schema: 1\n").expect("write yaml");
        let updated = fold_aiesim_timing(yaml_only.path(), &timing).expect("fold");
        assert_eq!(updated, ["report.yaml"], "no JSON side is not an error");
        assert!(!yaml_only.path().join("report.json").exists());
    }

    #[test]
    fn fold_without_root_reports_is_a_no_op() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("tapa.json"), "{}").expect("write state");
        let updated =
            fold_aiesim_timing(dir.path(), &aiesim_timing_fixture()).expect("fold is a no-op");
        assert!(updated.is_empty(), "analyze-only work dir folds nothing");
        assert!(!dir.path().join("report.json").exists());
        assert!(!dir.path().join("report.yaml").exists());
        assert_eq!(fs::read_dir(dir.path()).expect("read dir").count(), 1);
    }

    #[test]
    fn stream_timing_skips_unmeasured_streams_and_lone_beat_spacing() {
        assert!(
            AiesimStreamTiming::from_beat_times(&[]).is_none(),
            "a stream the simulator never timestamped has no entry"
        );
        let lone = AiesimStreamTiming::from_beat_times(&[100.0]).expect("one beat");
        assert_eq!(
            serde_json::to_value(&lone).expect("serialize"),
            serde_json::json!({ "beats": 1, "first_beat_ns": 100.0, "last_beat_ns": 100.0 })
        );
    }

    #[test]
    fn fold_rejects_malformed_and_non_object_documents_without_overwriting() {
        fn fold_fails(file: &str, content: &str) -> String {
            let dir = tempfile::tempdir().expect("tempdir");
            fs::write(dir.path().join(file), content).expect("write report");
            let error = fold_aiesim_timing(dir.path(), &aiesim_timing_fixture())
                .expect_err("an unfoldable report must fail loudly");
            assert_eq!(
                fs::read_to_string(dir.path().join(file)).expect("read back"),
                content,
                "a rejected document is never overwritten"
            );
            error.to_string()
        }

        let message = fold_fails("report.json", "{ not json");
        assert!(message.contains("cannot parse"), "{message}");
        assert!(message.contains("report.json"), "{message}");
        let message = fold_fails("report.json", r#"{"aie": 3}"#);
        assert!(message.contains("`aie` is not an object"), "{message}");
        let message = fold_fails("report.json", "[1]");
        assert!(message.contains("root is not an object"), "{message}");
        let message = fold_fails("report.yaml", "aie: [unclosed");
        assert!(message.contains("cannot parse"), "{message}");
        assert!(message.contains("report.yaml"), "{message}");
        let message = fold_fails("report.yaml", "aie: 3");
        assert!(message.contains("`aie` is not an object"), "{message}");
    }
}
