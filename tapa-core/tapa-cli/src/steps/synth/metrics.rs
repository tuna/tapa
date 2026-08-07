//! Effective synthesis metrics shared by reports and topology consumers.

use tapa_ir::{Area, Design, TaskLevel};

use crate::error::{CliError, Result};

/// Return a task's effective total area.
///
/// A stored `total_area` is an explicit post-synthesis override. Without one,
/// derive the total from the task's HLS `self_area` and every instantiated
/// child. A task with neither is unannotated, and reports say so rather than
/// counting it as free.
pub(super) fn effective_total_area(design: &Design, task_name: &str) -> Result<Option<Area>> {
    let task = design.tasks.get(task_name).ok_or_else(|| {
        CliError::Report(format!("metrics: task `{task_name}` not found in design"))
    })?;
    if let Some(total) = task.total_area {
        return Ok(Some(total));
    }
    let Some(mut total) = task.self_area else {
        return Ok(None);
    };

    if task.level == TaskLevel::Upper {
        for (child_name, instances) in &task.tasks {
            let count = instances.len();
            if count == 0 || !design.tasks.contains_key(child_name) {
                continue;
            }
            let Some(child_total) = effective_total_area(design, child_name)? else {
                continue;
            };
            let count = u64::try_from(count).map_err(|error| {
                CliError::Report(format!(
                    "metrics: task `{task_name}` has too many child instances: {error}"
                ))
            })?;
            total = total
                .checked_add_scaled(child_total, count)
                .ok_or_else(|| {
                    CliError::Report(format!(
                        "metrics: area overflows while aggregating task `{task_name}`"
                    ))
                })?;
        }
    }
    Ok(Some(total))
}
