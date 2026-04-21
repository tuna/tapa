use serde_json::Value as JsonValue;
use std::fs::File;
use std::path::Path;
use zip::ZipArchive;

use crate::common::{archive_text, Result};

const REPORT_FILES: &[&str] = &[
    "report.json",
    "report.yaml",
    "report/Add/Add_csynth.xml",
    "report/Mmap2Stream/Impl_csynth.xml",
    "report/Mmap2Stream/Mmap2Stream_csynth.xml",
    "report/Stream2Mmap/Impl_csynth.xml",
    "report/Stream2Mmap/Stream2Mmap_csynth.xml",
    "report/VecAdd/VecAdd_csynth.xml",
];

const REPORT_MODULES: &[&str] = &["Add", "Mmap2Stream", "Stream2Mmap"];

pub fn check_xo_reports(path: &Path) -> Result<()> {
    let file =
        File::open(path).map_err(|error| format!("failed to open {}: {error}", path.display()))?;
    let mut archive = ZipArchive::new(file)
        .map_err(|error| format!("failed to read {} as zip: {error}", path.display()))?;

    for report_file in REPORT_FILES {
        archive
            .by_name(report_file)
            .map_err(|_| format!("{} missing {report_file}", path.display()))?;
    }

    let report_json = archive_text(&mut archive, "report.json")?;
    let report_json: JsonValue = serde_json::from_str(&report_json)
        .map_err(|error| format!("invalid report.json: {error}"))?;
    for module in REPORT_MODULES {
        require_synth_source_json(&report_json, module, "report.json")?;
    }

    let report_yaml = archive_text(&mut archive, "report.yaml")?;
    let report_yaml: serde_yaml::Value = serde_yaml::from_str(&report_yaml)
        .map_err(|error| format!("invalid report.yaml: {error}"))?;
    for module in REPORT_MODULES {
        require_synth_source_yaml(&report_yaml, module, "report.yaml")?;
    }

    Ok(())
}

fn require_synth_source_json(report: &JsonValue, module: &str, name: &str) -> Result<()> {
    let source = report
        .get("area")
        .and_then(|value| value.get("breakdown"))
        .and_then(|value| value.get(module))
        .and_then(|value| value.get("area"))
        .and_then(|value| value.get("source"))
        .and_then(JsonValue::as_str);
    if source != Some("synth") {
        return Err(format!("{name}: {module} area source is not synth"));
    }
    Ok(())
}

fn require_synth_source_yaml(report: &serde_yaml::Value, module: &str, name: &str) -> Result<()> {
    let source = report
        .get("area")
        .and_then(|value| value.get("breakdown"))
        .and_then(|value| value.get(module))
        .and_then(|value| value.get("area"))
        .and_then(|value| value.get("source"))
        .and_then(serde_yaml::Value::as_str);
    if source != Some("synth") {
        return Err(format!("{name}: {module} area source is not synth"));
    }
    Ok(())
}
