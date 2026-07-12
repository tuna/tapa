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
use tapa_task_graph::Design;

use crate::error::{CliError, Result};
use crate::steps::version::VERSION as TAPA_VERSION;

use super::metrics::effective_total_area;

/// Typed report schema mirroring the on-disk format.
#[derive(Debug, Clone, Serialize)]
struct Report {
    schema: String,
    name: String,
    performance: Performance,
    area: Area,
}

#[derive(Debug, Clone, Serialize)]
struct Performance {
    source: String,
    clock_period: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    critical_path: Option<IndexMap<String, Self>>,
}

#[derive(Debug, Clone, Serialize)]
struct Area {
    source: String,
    total: IndexMap<String, Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    breakdown: Option<IndexMap<String, BreakdownEntry>>,
}

#[derive(Debug, Clone, Serialize)]
struct BreakdownEntry {
    count: usize,
    area: Area,
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
        .map_err(|e| CliError::InvalidArg(format!("report.json serialize: {e}")))?;
    fs::write(&json_path, json_str)?;

    let yaml_path = work_dir.join("report.yaml");
    let yaml_str = serde_yaml::to_string(&report)
        .map_err(|e| CliError::InvalidArg(format!("report.yaml serialize: {e}")))?;
    fs::write(&yaml_path, yaml_str)?;
    Ok(())
}

/// Recursively build a task report.
///
/// `TaskTopology::clock_period` and `total_area` contain the task-local HLS
/// estimate and an optional post-synthesis override, respectively. The
/// effective values are derived here in the same way as the original Python
/// task properties: an upper task's clock is the maximum of its own estimate
/// and all descendants, while an empty `total_area` is computed as
/// `self_area` plus every child instance's effective total.
fn build_task_report(design: &Design, task_name: &str, schema: &str) -> Result<Report> {
    let task = design.tasks.get(task_name).ok_or_else(|| {
        CliError::InvalidArg(format!("report: task `{task_name}` not found in design"))
    })?;

    let has_explicit_total = !task.total_area.is_empty();
    let area_source = if has_explicit_total { "synth" } else { "hls" };

    let mut child_reports = Vec::new();
    if task.level == "upper" {
        for (child_name, instances) in &task.tasks {
            let count = instances.as_array().map_or(0, Vec::len);
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

    let mut clock_period = task.clock_period.clone();
    let mut clock = parse_clock_period(task_name, &clock_period)?;
    for child in &child_reports {
        if child.clock.total_cmp(&clock).is_gt() {
            clock = child.clock;
            clock_period.clone_from(&child.report.performance.clock_period);
        }
    }

    let total_area = effective_total_area(design, task_name)?;

    let (performance, area_breakdown) = if task.level == "upper" {
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
        area: Area {
            source: area_source.to_string(),
            total: total_area,
            breakdown: area_breakdown,
        },
    })
}

fn parse_clock_period(task_name: &str, period: &str) -> Result<f64> {
    let parsed = period.parse::<f64>().map_err(|error| {
        CliError::InvalidArg(format!(
            "report: task `{task_name}` has invalid clock period `{period}`: {error}"
        ))
    })?;
    if !parsed.is_finite() {
        return Err(CliError::InvalidArg(format!(
            "report: task `{task_name}` has non-finite clock period `{period}`"
        )));
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use indexmap::IndexMap;
    use tapa_task_graph::TaskTopology;

    fn leaf(name: &str, clock: &str, area: Value) -> TaskTopology {
        TaskTopology {
            name: name.to_string(),
            level: "lower".to_string(),
            code: format!("void {name}() {{}}\n"),
            ports: Vec::new(),
            tasks: IndexMap::new(),
            fifos: IndexMap::new(),
            target: Some("hls".to_string()),
            is_slot: false,
            self_area: IndexMap::new(),
            total_area: area_to_map(area),
            clock_period: clock.to_string(),
        }
    }

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
        self_area: Value,
        tasks: IndexMap<String, Value>,
    ) -> TaskTopology {
        TaskTopology {
            name: name.to_string(),
            level: if tasks.is_empty() { "lower" } else { "upper" }.to_string(),
            code: format!("void {name}() {{}}\n"),
            ports: Vec::new(),
            tasks,
            fifos: IndexMap::new(),
            target: Some("hls".to_string()),
            is_slot: false,
            self_area: area_to_map(self_area),
            total_area: IndexMap::new(),
            clock_period: clock.to_string(),
        }
    }

    #[test]
    fn derives_recursive_metrics_when_total_area_is_empty() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut tasks = IndexMap::new();
        tasks.insert(
            "Leaf".to_string(),
            derived_task(
                "Leaf",
                "4.0",
                serde_json::json!({"LUT": 11, "FF": 1}),
                IndexMap::new(),
            ),
        );
        tasks.insert(
            "Middle".to_string(),
            derived_task(
                "Middle",
                "2.5",
                serde_json::json!({"LUT": 7, "FF": 2}),
                IndexMap::from([(
                    "Leaf".to_string(),
                    serde_json::json!([
                        {"args": {}, "step": 0},
                        {"args": {}, "step": 0},
                        {"args": {}, "step": 0}
                    ]),
                )]),
            ),
        );
        tasks.insert(
            "Top".to_string(),
            derived_task(
                "Top",
                "2.0",
                serde_json::json!({"LUT": 5, "FF": 3}),
                IndexMap::from([(
                    "Middle".to_string(),
                    serde_json::json!([
                        {"args": {}, "step": 0},
                        {"args": {}, "step": 0}
                    ]),
                )]),
            ),
        );
        let design = Design {
            top: "Top".to_string(),
            target: "xilinx-hls".to_string(),
            tasks,
            slot_task_name_to_fp_region: None,
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
        let mut tasks = IndexMap::new();
        tasks.insert(
            "VecAdd".to_string(),
            TaskTopology {
                name: "VecAdd".to_string(),
                level: "upper".to_string(),
                code: "void VecAdd() {}\n".to_string(),
                ports: Vec::new(),
                tasks: {
                    let mut m = IndexMap::new();
                    m.insert(
                        "Add".to_string(),
                        serde_json::json!([{"args": {}, "step": 0}, {"args": {}, "step": 0}]),
                    );
                    m
                },
                fifos: IndexMap::new(),
                target: Some("hls".to_string()),
                is_slot: false,
                self_area: IndexMap::new(),
                total_area: area_to_map(serde_json::json!({"LUT": 100})),
                clock_period: "3.33".to_string(),
            },
        );
        tasks.insert(
            "Add".to_string(),
            leaf("Add", "3.33", serde_json::json!({"LUT": 50})),
        );
        let design = Design {
            top: "VecAdd".to_string(),
            target: "xilinx-vitis".to_string(),
            tasks,
            slot_task_name_to_fp_region: None,
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
        let mut tasks = IndexMap::new();
        tasks.insert("T".to_string(), leaf("T", "3.33", serde_json::json!({})));
        let design = Design {
            top: "T".to_string(),
            target: "xilinx-hls".to_string(),
            tasks,
            slot_task_name_to_fp_region: None,
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
