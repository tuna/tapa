//! Effective synthesis metrics shared by reports and topology consumers.

use indexmap::IndexMap;
use serde_json::Value;
use tapa_ir::{Design, TaskLevel};

use crate::error::{CliError, Result};

/// Return a task's effective total area.
///
/// A stored `total_area` is an explicit post-synthesis override. When it is
/// empty, derive the total recursively from the task's HLS `self_area` and
/// every instantiated child.
pub(super) fn effective_total_area(
    design: &Design,
    task_name: &str,
) -> Result<IndexMap<String, Value>> {
    let task = design.tasks.get(task_name).ok_or_else(|| {
        CliError::InvalidArg(format!("metrics: task `{task_name}` not found in design"))
    })?;
    if !task.total_area.is_empty() {
        return Ok(task.total_area.clone());
    }

    let mut total = task.self_area.clone();
    if task.level == TaskLevel::Upper {
        for (child_name, instances) in &task.tasks {
            let count = instances.len();
            if count == 0 || !design.tasks.contains_key(child_name) {
                continue;
            }
            let child_total = effective_total_area(design, child_name)?;
            add_scaled_area(&mut total, &child_total, count, task_name)?;
        }
    }
    Ok(total)
}

fn add_scaled_area(
    total: &mut IndexMap<String, Value>,
    child: &IndexMap<String, Value>,
    count: usize,
    task_name: &str,
) -> Result<()> {
    let count = i64::try_from(count).map_err(|error| {
        CliError::InvalidArg(format!(
            "metrics: task `{task_name}` has too many child instances: {error}"
        ))
    })?;
    for (resource, child_value) in child {
        let child_value = area_value(child_value, task_name, resource)?;
        let increment = child_value.checked_mul(count).ok_or_else(|| {
            CliError::InvalidArg(format!(
                "metrics: `{resource}` area overflows while aggregating task `{task_name}`"
            ))
        })?;
        let current = total
            .get(resource)
            .map_or(Ok(0), |value| area_value(value, task_name, resource))?;
        let value = current.checked_add(increment).ok_or_else(|| {
            CliError::InvalidArg(format!(
                "metrics: `{resource}` area overflows while aggregating task `{task_name}`"
            ))
        })?;
        total.insert(resource.clone(), Value::from(value));
    }
    Ok(())
}

fn area_value(value: &Value, task_name: &str, resource: &str) -> Result<i64> {
    value.as_i64().ok_or_else(|| {
        CliError::InvalidArg(format!(
            "metrics: task `{task_name}` has non-integer `{resource}` area `{value}`"
        ))
    })
}
