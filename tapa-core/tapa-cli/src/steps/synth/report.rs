//! Top-task `report.json` / `report.yaml` emitter.
//!
//! Ports `tapa/codegen/program_rtl.py::generate_top_rtl`'s report
//! step + `tapa/task.py::Task.report`. Synth writes the report after
//! RTL codegen so downstream pack flows (xilinx-vitis `.xo` and
//! xilinx-hls `.zip`) can include it as `report.yaml` at archive
//! root and `report.json` alongside.
//!
//! Schema (matches Python):
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

use serde_json::{json, Value};
use tapa_task_graph::{Design, TaskTopology};

use crate::error::{CliError, Result};
use crate::steps::version::VERSION as TAPA_VERSION;

/// Write `<work_dir>/report.{json,yaml}` for the design's top task.
/// `override_schema` (mirrors `--override-report-schema-version`) wins
/// over the baked `VERSION` constant when non-empty.
pub fn write_top_report(
    work_dir: &Path,
    design: &Design,
    override_schema: &str,
) -> Result<()> {
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

/// Recursively build a task-report dict mirroring Python's
/// `Task.report`. Only recurses one level for `critical_path` /
/// `breakdown` (Python's report itself recurses, but only top-level
/// is read by downstream consumers).
fn build_task_report(design: &Design, task_name: &str, schema: &str) -> Result<Value> {
    let task = design.tasks.get(task_name).ok_or_else(|| {
        CliError::InvalidArg(format!(
            "report: task `{task_name}` not found in design",
        ))
    })?;
    let mut performance = serde_json::Map::new();
    performance.insert("source".to_string(), json!("hls"));
    performance.insert("clock_period".to_string(), json!(task.clock_period));

    let mut area = serde_json::Map::new();
    // Python: `source = "synth" if self._total_area else "hls"`. The
    // Rust port records explicit per-task `total_area` only after the
    // post-Vivado utilization pass runs (`emit_post_synth_util`); use
    // the presence of any non-empty `total_area` value as the proxy
    // (matches Python's behavior — `_total_area` is set at the same
    // point).
    let area_source = if has_synth_area(task) { "synth" } else { "hls" };
    area.insert("source".to_string(), json!(area_source));
    area.insert("total".to_string(), json!(task.total_area));

    if task.level == "upper" {
        let mut critical_path = serde_json::Map::new();
        let mut breakdown = serde_json::Map::new();
        for (child_name, instances) in &task.tasks {
            let Some(child_task) = design.tasks.get(child_name) else {
                continue;
            };
            let count = instances.as_array().map_or(0, Vec::len);
            // Python keys breakdown by `instance.task.name` (the child
            // task's own name, which matches the IndexMap key here),
            // dedup'd via `setdefault`. Always emit at least 1.
            let count = count.max(1);

            let child_report = build_task_report(design, child_name, schema)?;
            if task.clock_period == child_task.clock_period {
                if let Some(child_performance) = child_report.get("performance") {
                    critical_path
                        .entry(child_name.clone())
                        .or_insert_with(|| child_performance.clone());
                }
            }
            let child_area = child_report
                .get("area")
                .cloned()
                .unwrap_or_else(|| json!({}));
            breakdown.insert(
                child_name.clone(),
                json!({"count": count, "area": child_area}),
            );
        }
        performance.insert("critical_path".to_string(), Value::Object(critical_path));
        area.insert("breakdown".to_string(), Value::Object(breakdown));
    }

    Ok(json!({
        "schema": schema,
        "name": task_name,
        "performance": Value::Object(performance),
        "area": Value::Object(area),
    }))
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
                        json!([{"args": {}, "step": 0}, {"args": {}, "step": 0}]),
                    );
                    m
                },
                fifos: IndexMap::new(),
                target: Some("hls".to_string()),
                is_slot: false,
                self_area: IndexMap::new(),
                total_area: area_to_map(json!({"LUT": 100})),
                clock_period: "3.33".to_string(),
            },
        );
        tasks.insert(
            "Add".to_string(),
            leaf("Add", "3.33", json!({"LUT": 50})),
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
        assert!(yaml.contains("count: 2"), "report missing breakdown count: {yaml}");
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
        tasks.insert("T".to_string(), leaf("T", "3.33", json!({})));
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
