//! The single work-directory state file, `<work_dir>/tapa.json`.
//!
//! `analyze` writes it, `synth` annotates it in place, `pack` consumes it.
//! It is the **only** file the pipeline reads back: the unified
//! [`TaskGraph`] plus the small typed [`FlowSettings`] block, behind a
//! [`VERSION`] stamp so a work dir written by a different tapa fails with a
//! clear message instead of a field-level serde diagnostic.
//!
//! `analyze` also drops the verbatim `tapacc` stdout at
//! `<work_dir>/tapacc.json` for provenance, but nothing reads it back — it
//! is a debug artifact, not state, so there is exactly one schema-bearing
//! file in the work directory.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tapa_ir::{ParseError, TaskGraph};

use crate::error::{CliError, Result};
use crate::state::json::write_bytes_atomic;

/// Name of the single work-directory state file.
pub const FILE_NAME: &str = "tapa.json";

/// Schema version stamped into every [`WorkState`] this tapa writes.
///
/// Bump on any backward-incompatible change to the state shape — including
/// the nested [`TaskGraph`] wire form. Work dirs outlive tool versions, so a
/// mismatch must surface as [`CliError::StaleWorkState`], not as a confusing
/// parse failure.
pub const VERSION: u32 = 1;

/// Everything the pipeline persists between steps.
#[allow(
    clippy::derive_partial_eq_without_eq,
    reason = "transitively holds serde_json::Value through TaskGraph"
)]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WorkState {
    /// Schema version; always [`VERSION`] for state this tapa writes.
    pub version: u32,
    /// The unified task graph: the structure `analyze` derives from
    /// `tapacc`, plus the post-synthesis annotations `synth` writes in
    /// place.
    pub graph: TaskGraph,
    /// Flow-level settings resolved by the pipeline steps.
    pub flow: FlowSettings,
}

impl WorkState {
    /// Wrap `graph` with the current [`VERSION`] and pre-synth (default)
    /// flow settings.
    pub fn new(graph: TaskGraph) -> Self {
        Self {
            version: VERSION,
            graph,
            flow: FlowSettings::default(),
        }
    }
}

/// Flow-level settings shared across pipeline steps.
///
/// The compilation flow target is deliberately **absent**: [`TaskGraph::target`]
/// is its single home, so there is nothing to reconcile between two copies.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FlowSettings {
    /// Target part number resolved by `synth` from `--part-num` or
    /// `--platform`. `pack` reads it back to build the `.xo`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub part_num: Option<String>,
    /// Vitis platform `synth --platform` ran against, when given. Read by
    /// `pack --bitstream-script` to render the `v++` invocation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
    /// The **requested** target clock period.
    ///
    /// Distinct from [`tapa_ir::Task::clock_period`], which is the per-task
    /// *achieved* estimate HLS reports back: one is an input to synthesis,
    /// the other a result of it. Both are kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clock_period: Option<String>,
    /// `v++` connectivity config forwarded to `pack --bitstream-script`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connectivity: Option<PathBuf>,
    /// Set once `synth` has completed for this work dir.
    #[serde(default)]
    pub synthed: bool,
}

/// Path of the state file inside `work_dir`.
pub fn path_in(work_dir: &Path) -> PathBuf {
    work_dir.join(FILE_NAME)
}

/// Load `<work_dir>/tapa.json`.
///
/// A missing file surfaces as [`CliError::MissingState`] and a foreign schema
/// version as [`CliError::StaleWorkState`] — both checked before serde gets a
/// chance to report whichever field it happens to trip over first.
pub fn load(work_dir: &Path) -> Result<WorkState> {
    let path = path_in(work_dir);
    if !path.exists() {
        return Err(CliError::MissingState {
            name: FILE_NAME.to_string(),
            path,
        });
    }
    let text = fs_err::read_to_string(&path)?;
    check_version(&text, &path)?;
    let de = &mut serde_json::Deserializer::from_str(&text);
    serde_path_to_error::deserialize(de).map_err(|e| {
        CliError::Schema(ParseError::Schema {
            path: e.path().to_string(),
            message: e.inner().to_string(),
        })
    })
}

/// Persist `state` to `<work_dir>/tapa.json`.
///
/// Pretty-printed (work dirs are meant to be read and diffed by humans) and
/// swapped in atomically, so a reader never observes a half-written file.
pub fn store(work_dir: &Path, state: &WorkState) -> Result<()> {
    let mut bytes = serde_json::to_vec_pretty(state)?;
    bytes.push(b'\n');
    write_bytes_atomic(work_dir, FILE_NAME, &bytes)
}

/// The `version` stamp alone, read without committing to the rest of the
/// schema. Unknown fields are ignored (no `deny_unknown_fields`) precisely so
/// this still parses when the surrounding shape is one this tapa cannot read.
#[derive(Deserialize)]
struct VersionProbe {
    #[serde(default)]
    version: Option<u32>,
}

/// Reject a work dir stamped with any version but [`VERSION`].
///
/// A payload that is not even a JSON object falls through to the full parse,
/// which reports the real syntax error rather than a bogus version complaint.
fn check_version(text: &str, path: &Path) -> Result<()> {
    let Ok(probe) = serde_json::from_str::<VersionProbe>(text) else {
        return Ok(());
    };
    match probe.version {
        Some(VERSION) => Ok(()),
        found => Err(CliError::StaleWorkState {
            path: path.to_path_buf(),
            found: found.map_or_else(|| "unversioned".to_string(), |v| format!("v{v}")),
            expected: VERSION,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    use tapa_ir::{SynthTarget, Target, Task, TaskLevel};

    fn sample_state() -> WorkState {
        let mut tasks = BTreeMap::new();
        tasks.insert(
            "Top".to_string(),
            Task {
                level: TaskLevel::Lower,
                code: "void Top() {}".to_string(),
                ports: Vec::new(),
                tasks: BTreeMap::new(),
                fifos: BTreeMap::new(),
                readable_name: "Top".to_string(),
                synth: SynthTarget::Hls,
                self_area: indexmap::IndexMap::new(),
                total_area: indexmap::IndexMap::new(),
                clock_period: "0".to_string(),
            },
        );
        WorkState::new(TaskGraph {
            top: "Top".to_string(),
            target: Target::XilinxHls,
            tasks,
            cflags: Vec::new(),
        })
    }

    #[test]
    fn round_trip_via_disk() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut state = sample_state();
        state.flow.part_num = Some("xcvu37p".to_string());
        state.flow.clock_period = Some("3.33".to_string());
        state.flow.synthed = true;
        store(dir.path(), &state).expect("store");
        let loaded = load(dir.path()).expect("load");
        assert_eq!(loaded, state);
    }

    #[test]
    fn missing_state_is_typed_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let err = load(dir.path()).expect_err("must fail");
        assert!(
            matches!(err, CliError::MissingState { ref name, .. } if name == FILE_NAME),
            "missing tapa.json must surface as MissingState; got {err}",
        );
    }

    #[test]
    fn stored_state_is_pretty_printed() {
        let dir = tempfile::tempdir().expect("tempdir");
        store(dir.path(), &sample_state()).expect("store");
        let raw = fs_err::read_to_string(path_in(dir.path())).expect("read");
        assert!(
            raw.starts_with("{\n  \"version\": 1,\n"),
            "state file must be pretty-printed for human diffing; got {raw}",
        );
        assert!(raw.ends_with("}\n"), "state file must end with a newline");
    }

    #[test]
    fn foreign_version_is_typed_error_not_serde_noise() {
        let dir = tempfile::tempdir().expect("tempdir");
        store(dir.path(), &sample_state()).expect("store");
        // Restamp with a future version and drop the graph the way a
        // schema change would, so a plain parse would fail on a field
        // long before it noticed the version.
        fs_err::write(
            path_in(dir.path()),
            br#"{"version": 2, "graph": {"whatever": true}}"#,
        )
        .expect("write v2 state");
        let err = load(dir.path()).expect_err("foreign version must fail");
        let CliError::StaleWorkState {
            found, expected, ..
        } = &err
        else {
            panic!("expected StaleWorkState, got {err}");
        };
        assert_eq!(found, "v2", "reported version");
        assert_eq!(*expected, VERSION, "expected version");
        assert!(
            err.to_string().contains("tapa analyze"),
            "error must tell the user how to recover; got {err}",
        );
    }

    #[test]
    fn unversioned_state_is_rejected() {
        // Pre-`version` work dirs (and hand-written files that forget the
        // stamp) must get the same clear message, not `missing field`.
        let dir = tempfile::tempdir().expect("tempdir");
        fs_err::write(path_in(dir.path()), br#"{"graph": {}, "flow": {}}"#).expect("write");
        let err = load(dir.path()).expect_err("unversioned state must fail");
        assert!(
            matches!(err, CliError::StaleWorkState { ref found, .. } if found == "unversioned"),
            "got {err}",
        );
    }

    #[test]
    fn malformed_json_reports_the_syntax_error() {
        // The version probe must not swallow a real syntax error.
        let dir = tempfile::tempdir().expect("tempdir");
        fs_err::write(path_in(dir.path()), b"not json at all").expect("write");
        let err = load(dir.path()).expect_err("malformed state must fail");
        assert!(
            matches!(err, CliError::Schema(..)),
            "malformed JSON must surface as a schema error; got {err}",
        );
    }

    #[test]
    fn unknown_field_is_rejected() {
        // `deny_unknown_fields` is what keeps the state schema honest; pin it.
        let dir = tempfile::tempdir().expect("tempdir");
        store(dir.path(), &sample_state()).expect("store");
        let text = fs_err::read_to_string(path_in(dir.path())).expect("read");
        let patched = text.replace("\"version\": 1,", "\"version\": 1,\n  \"bogus\": 1,");
        fs_err::write(path_in(dir.path()), patched).expect("write");
        let err = load(dir.path()).expect_err("unknown field must fail");
        assert!(
            matches!(err, CliError::Schema(..)),
            "unknown state field must be rejected; got {err}",
        );
    }

    #[test]
    fn flow_settings_omit_absent_values() {
        // Absent optional settings must not materialize as `null`s that a
        // reader could mistake for "resolved to nothing".
        let dir = tempfile::tempdir().expect("tempdir");
        store(dir.path(), &sample_state()).expect("store");
        let raw = fs_err::read_to_string(path_in(dir.path())).expect("read");
        assert!(
            raw.contains("\"flow\": {\n    \"synthed\": false\n  }"),
            "unresolved flow settings must be omitted, not null; got {raw}",
        );
    }
}
