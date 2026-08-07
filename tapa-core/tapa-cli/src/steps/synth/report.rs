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
//!   clock_period: "<seconds>"
//!   critical_path:        # only when top is upper
//!     <child_task_name>: { ...child performance dict... }
//! area:
//!   source: hls            # "synth" when total_area has non-zero utilization
//!   total: { ...resource dict... }
//!   breakdown:             # only when top is upper
//!     <child_task_name>: { count: <n>, area: { ... } }
//! ```

use std::fs;
use std::path::Path;

use indexmap::IndexMap;
use serde::Serialize;
use serde_json::Value;
use tapa_ir::{Design, TaskLevel};

use crate::error::{CliError, Result};
use crate::steps::version::VERSION as TAPA_VERSION;

use super::metrics::effective_total_area;

/// Typed report schema mirroring the on-disk format.
#[derive(Debug, Clone, Serialize)]
struct Report {
    schema: String,
    name: String,
    performance: Performance,
    area: AreaSection,
}

#[derive(Debug, Clone, Serialize)]
struct Performance {
    source: String,
    clock_period: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    critical_path: Option<IndexMap<String, Self>>,
}

/// The `area` section of a task's published report: where the numbers came
/// from, the totals, and the per-child breakdown. [`tapa_ir::Area`] is the
/// count model behind `total`.
#[derive(Debug, Clone, Serialize)]
struct AreaSection {
    source: String,
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
    clock: f64,
    report: Report,
}

/// Write `<work_dir>/report.{json,yaml}` for the design's top task.
/// `override_schema` (mirrors `--override-report-schema-version`) wins
/// over the baked `VERSION` constant when non-empty.
pub fn write_top_report(work_dir: &Path, design: &Design, override_schema: &str) -> Result<()> {
    let schema = if override_schema.is_empty() {
        TAPA_VERSION
    } else {
        override_schema
    };
    let report = build_task_report(design, &design.top, schema)?;

    let json_path = work_dir.join("report.json");
    let json_str = serde_json::to_string_pretty(&report)
        .map_err(|e| CliError::Report(format!("report.json serialize: {e}")))?;
    fs::write(&json_path, json_str)?;

    let yaml_path = work_dir.join("report.yaml");
    let yaml_str = serde_yaml::to_string(&report)
        .map_err(|e| CliError::Report(format!("report.yaml serialize: {e}")))?;
    fs::write(&yaml_path, yaml_str)?;
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
    let task = design.tasks.get(task_name).ok_or_else(|| {
        CliError::Report(format!("report: task `{task_name}` not found in design"))
    })?;

    let has_explicit_total = task.total_area.is_some();
    let area_source = if has_explicit_total { "synth" } else { "hls" };

    let mut child_reports = Vec::new();
    if task.level == TaskLevel::Upper {
        for (child_name, instances) in &task.tasks {
            let count = instances.len();
            if count == 0 || !design.tasks.contains_key(child_name) {
                continue;
            }
            let report = build_task_report(design, child_name, schema)?;
            let clock = parse_clock_period(child_name, &report.performance.clock_period)?;
            child_reports.push(ChildReport {
                name: child_name,
                count,
                clock,
                report,
            });
        }
    }

    // A task with no HLS estimate carries an empty `clock_period`: either
    // `synth` has not run yet, or it is a `synth: ignore` task that HLS
    // skips entirely. Such a task contributes zero to the critical path
    // rather than failing the whole report.
    let mut clock_period = if task.clock_period.is_empty() {
        "0".to_string()
    } else {
        task.clock_period.clone()
    };
    let mut clock = parse_clock_period(task_name, &clock_period)?;
    for child in &child_reports {
        if child.clock.total_cmp(&clock).is_gt() {
            clock = child.clock;
            clock_period.clone_from(&child.report.performance.clock_period);
        }
    }

    let total_area = published_area(effective_total_area(design, task_name)?);

    let (performance, area_breakdown) = if task.level == TaskLevel::Upper {
        let mut critical_path = IndexMap::new();
        let mut breakdown = IndexMap::new();
        for child in child_reports {
            if child.clock.total_cmp(&clock).is_eq() {
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
                source: "hls".to_string(),
                clock_period,
                critical_path: Some(critical_path),
            },
            Some(breakdown),
        )
    } else {
        (
            Performance {
                source: "hls".to_string(),
                clock_period,
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
            source: area_source.to_string(),
            total: total_area,
            breakdown: area_breakdown,
        },
    })
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

fn parse_clock_period(task_name: &str, period: &str) -> Result<f64> {
    let parsed = period.parse::<f64>().map_err(|error| {
        CliError::Report(format!(
            "report: task `{task_name}` has invalid clock period `{period}`: {error}"
        ))
    })?;
    if !parsed.is_finite() {
        return Err(CliError::Report(format!(
            "report: task `{task_name}` has non-finite clock period `{period}`"
        )));
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    use indexmap::IndexMap;
    use tapa_ir::{SynthTarget, Task, TaskInstance, TaskLevel};

    /// `count` child instances with empty args and step 0, mirroring
    /// the `[{"args": {}, "step": 0}, ...]` JSON the untyped fixtures
    /// used.
    fn instances(count: usize) -> Vec<TaskInstance> {
        vec![
            TaskInstance {
                name: None,
                args: BTreeMap::new(),
                step: 0,
            };
            count
        ]
    }

    fn leaf(name: &str, clock: &str, area: Option<tapa_ir::Area>) -> Task {
        Task {
            level: TaskLevel::Lower,
            code: format!("void {name}() {{}}\n"),
            ports: Vec::new(),
            tasks: BTreeMap::new(),
            fifos: BTreeMap::new(),
            readable_name: String::new(),
            synth: SynthTarget::Hls,
            self_area: None,
            total_area: area,
            clock_period: clock.to_string(),
        }
    }

    #[allow(dead_code, reason = "kept for fixtures that still build maps")]
    fn area_to_map(v: Value) -> IndexMap<String, Value> {
        match v {
            Value::Object(o) => o.into_iter().collect(),
            Value::Null
            | Value::Bool(_)
            | Value::Number(_)
            | Value::String(_)
            | Value::Array(_) => IndexMap::new(),
        }
    }

    fn derived_task(
        name: &str,
        clock: &str,
        self_area: tapa_ir::Area,
        tasks: BTreeMap<String, Vec<TaskInstance>>,
    ) -> Task {
        Task {
            level: if tasks.is_empty() {
                TaskLevel::Lower
            } else {
                TaskLevel::Upper
            },
            code: format!("void {name}() {{}}\n"),
            ports: Vec::new(),
            tasks,
            fifos: BTreeMap::new(),
            readable_name: String::new(),
            synth: SynthTarget::Hls,
            self_area: Some(self_area),
            total_area: None,
            clock_period: clock.to_string(),
        }
    }

    /// `synth: ignore` tasks are skipped by HLS, so they reach the report
    /// with an empty `clock_period` annotation. That must read as zero (the
    /// value `analyze` used to seed) instead of failing the whole report.
    #[test]
    fn unsynthesized_task_clock_period_reads_as_zero() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut custom_rtl = leaf(
            "Custom",
            "",
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
                    code: "void Top() {}\n".to_string(),
                    ports: Vec::new(),
                    tasks: BTreeMap::from([("Custom".to_string(), instances(1))]),
                    fifos: BTreeMap::new(),
                    readable_name: String::new(),
                    synth: SynthTarget::Hls,
                    self_area: None,
                    total_area: None,
                    clock_period: "2.5".to_string(),
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

        write_top_report(dir.path(), &design, "").expect("report must not fail on empty clock");
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
                "4.0",
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
                "2.5",
                tapa_ir::Area {
                    lut: 7,
                    ff: 2,
                    ..tapa_ir::Area::default()
                },
                BTreeMap::from([("Leaf".to_string(), instances(3))]),
            ),
        );
        tasks.insert(
            "Top".to_string(),
            derived_task(
                "Top",
                "2.0",
                tapa_ir::Area {
                    lut: 5,
                    ff: 3,
                    ..tapa_ir::Area::default()
                },
                BTreeMap::from([("Middle".to_string(), instances(2))]),
            ),
        );
        let design = Design {
            schema_version: tapa_ir::graph::SCHEMA_VERSION,
            top: "Top".to_string(),
            target: tapa_ir::Target::XilinxHls,
            tasks,
            cflags: Vec::new(),
        };

        write_top_report(dir.path(), &design, "").expect("write report");
        let parsed: Value = serde_json::from_str(
            &fs::read_to_string(dir.path().join("report.json")).expect("read report"),
        )
        .expect("valid report json");

        assert_eq!(parsed["performance"]["clock_period"], "4.0");
        assert_eq!(
            parsed["performance"]["critical_path"]["Middle"]["clock_period"],
            "4.0"
        );
        assert_eq!(
            parsed["performance"]["critical_path"]["Middle"]["critical_path"]["Leaf"]
                ["clock_period"],
            "4.0"
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
                code: "void VecAdd() {}\n".to_string(),
                ports: Vec::new(),
                tasks: BTreeMap::from([("Add".to_string(), instances(2))]),
                fifos: BTreeMap::new(),
                readable_name: String::new(),
                synth: SynthTarget::Hls,
                self_area: None,
                total_area: Some(tapa_ir::Area {
                    lut: 100,
                    ..tapa_ir::Area::default()
                }),
                clock_period: "3.33".to_string(),
            },
        );
        tasks.insert(
            "Add".to_string(),
            leaf(
                "Add",
                "3.33",
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
        write_top_report(dir.path(), &design, "").expect("write report");
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
    }

    #[test]
    fn override_schema_wins() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut tasks = BTreeMap::new();
        tasks.insert("T".to_string(), leaf("T", "3.33", None));
        let design = Design {
            schema_version: tapa_ir::graph::SCHEMA_VERSION,
            top: "T".to_string(),
            target: tapa_ir::Target::XilinxHls,
            tasks,
            cflags: Vec::new(),
        };
        write_top_report(dir.path(), &design, "9.9.9-override").expect("write");
        let parsed: Value = serde_json::from_str(
            &fs::read_to_string(dir.path().join("report.json")).expect("read"),
        )
        .expect("json");
        assert_eq!(parsed["schema"], "9.9.9-override");
        assert_eq!(parsed["area"]["source"], "hls"); // empty total_area
    }
}
