//! `design.json` read / write — wraps `tapa_task_graph::Design` with
//! work-directory path conventions.

use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

use tapa_task_graph::Design;

use crate::error::{CliError, Result};

const FILE_NAME: &str = "design.json";

pub fn path_in(work_dir: &Path) -> std::path::PathBuf {
    work_dir.join(FILE_NAME)
}

/// Load `<work_dir>/design.json`. Missing file surfaces as
/// [`CliError::MissingState`].
pub fn load_design(work_dir: &Path) -> Result<Design> {
    let path = path_in(work_dir);
    if !path.exists() {
        return Err(CliError::MissingState {
            name: FILE_NAME.to_string(),
            path,
        });
    }
    let file = File::open(&path)?;
    Ok(Design::from_reader(file)?)
}

/// Persist `design` to `<work_dir>/design.json` using the
/// Python-compatible JSON formatter.
pub fn store_design(work_dir: &Path, design: &Design) -> Result<()> {
    std::fs::create_dir_all(work_dir)?;
    let path = path_in(work_dir);
    let mut writer = BufWriter::new(File::create(&path)?);
    design.to_writer(&mut writer)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use indexmap::IndexMap;
    use tapa_task_graph::TaskTopology;

    fn sample_design() -> Design {
        let mut tasks = IndexMap::new();
        tasks.insert(
            "Top".to_string(),
            TaskTopology {
                name: "Top".to_string(),
                level: "lower".to_string(),
                code: "void Top() {}".to_string(),
                ports: Vec::new(),
                tasks: IndexMap::new(),
                fifos: IndexMap::new(),
                target: Some("hls".to_string()),
                is_slot: false,
                self_area: IndexMap::new(),
                total_area: IndexMap::new(),
                clock_period: "0".to_string(),
            },
        );
        Design {
            top: "Top".to_string(),
            target: "xilinx-hls".to_string(),
            tasks,
            slot_task_name_to_fp_region: None,
        }
    }

    #[test]
    fn round_trip_via_disk() {
        let dir = tempfile::tempdir().expect("tempdir");
        let design = sample_design();
        store_design(dir.path(), &design).expect("store");
        let loaded = load_design(dir.path()).expect("load");
        assert_eq!(loaded, design);
    }

    #[test]
    fn missing_design_is_typed_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let err = load_design(dir.path()).expect_err("must fail");
        assert!(matches!(err, CliError::MissingState { .. }));
    }
}
