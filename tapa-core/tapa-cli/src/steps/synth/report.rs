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
//!   source: hls            # "synth" once Vivado utilization populates total_area
//!   total: { ...resource dict... }
//!   breakdown:             # only when top is upper
//!     <child_task_name>: { count: <n>, area: { ... } }
//! ```

use std::fs;
use std::path::Path;

use indexmap::IndexMap;
use serde::Serialize;
use serde_json::Value;
use tapa_task_graph::{Design, TaskTopology};

use crate::error::{CliError, Result};
use crate::steps::version::VERSION as TAPA_VERSION;

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

/// Recursively build a task-report struct mirroring current
/// `Task.report`. Only recurses one level for `critical_path` /
/// `breakdown` (report itself recurses, but only top-level
/// is read by downstream consumers).
fn build_task_report(design: &Design, task_name: &str, schema: &str) -> Result<Report> {
    let task = design.tasks.get(task_name).ok_or_else(|| {
        CliError::InvalidArg(format!("report: task `{task_name}` not found in design"))
    })?;

    let area_source = if has_synth_area(task) { "synth" } else { "hls" };

    let (performance, area_breakdown) = if task.level == "upper" {
        let mut critical_path = IndexMap::new();
        let mut breakdown = IndexMap::new();
        for (child_name, instances) in &task.tasks {
            let Some(child_task) = design.tasks.get(child_name) else {
                continue;
            };
            let count = instances.as_array().map_or(0, Vec::len);
            let count = count.max(1);

            let child_report = build_task_report(design, child_name, schema)?;
            if task.clock_period == child_task.clock_period {
                critical_path.insert(child_name.clone(), child_report.performance);
            }
            let child_area = Area {
                source: if has_synth_area(child_task) {
                    "synth"
                } else {
                    "hls"
                }
                .to_string(),
                total: child_task.total_area.clone(),
                breakdown: None,
            };
            breakdown.insert(
                child_name.clone(),
                BreakdownEntry {
                    count,
                    area: child_area,
                },
            );
        }
        (
            Performance {
                source: "hls".to_string(),
                clock_period: task.clock_period.clone(),
                critical_path: Some(critical_path),
            },
            Some(breakdown),
        )
    } else {
        (
            Performance {
                source: "hls".to_string(),
                clock_period: task.clock_period.clone(),
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
            total: task.total_area.clone(),
            breakdown: area_breakdown,
        },
    })
}

fn has_synth_area(task: &TaskTopology) -> bool {
    task.total_area.values().any(|v| match v {
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::Null => false,
        Value::Bool(_) | Value::String(_) | Value::Array(_) | Value::Object(_) => true,
    })
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
