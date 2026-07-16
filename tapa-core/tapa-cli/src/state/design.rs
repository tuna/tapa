//! `design.json` read / write — wraps `tapa_ir::Design` with
//! work-directory path conventions.

use std::path::Path;

use tapa_ir::Design;

use crate::error::{CliError, Result};
use crate::state::json::write_json_atomic;

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
    let file = fs_err::File::open(&path)?;
    Ok(Design::from_reader(file)?)
}

/// Persist `design` to `<work_dir>/design.json` using the shared compact JSON
/// formatter ([`write_json_atomic`]).
pub fn store_design(work_dir: &Path, design: &Design) -> Result<()> {
    write_json_atomic(work_dir, FILE_NAME, design)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    use indexmap::IndexMap;
    use tapa_ir::{SynthTarget, Task, TaskLevel};

    fn sample_design() -> Design {
        let mut tasks = BTreeMap::new();
        tasks.insert(
            "Top".to_string(),
            Task {
                level: TaskLevel::Lower,
                code: "void Top() {}".to_string(),
                ports: Vec::new(),
                tasks: BTreeMap::new(),
                fifos: BTreeMap::new(),
                readable_name: String::new(),
                synth: SynthTarget::Hls,
                self_area: IndexMap::new(),
                total_area: IndexMap::new(),
                clock_period: "0".to_string(),
            },
        );
        Design {
            top: "Top".to_string(),
            target: tapa_ir::Target::XilinxHls,
            tasks,
            cflags: Vec::new(),
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
