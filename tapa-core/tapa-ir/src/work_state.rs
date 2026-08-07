//! The persisted pipeline state: [`WorkState`], the root of `tapa.json`.
//!
//! This is the *schema* half of the work-directory state file; the file I/O
//! (atomic write, path helpers, the stale-work-dir error) belongs to
//! `tapa-cli`, which owns the work directory. The types live here because
//! `tapa.json` has two readers in two Cargo workspaces:
//!
//! * `tapa-cli` writes `<work_dir>/tapa.json` (`analyze`), annotates it in
//!   place (`synth`), and reads it back (`pack`); and
//! * `frt-cosim` reads the `tapa.json` entry `tapa pack` copies verbatim into
//!   the `.zip` archive, to recover the kernel's argument list.
//!
//! Both parse *these* types, so a field rename is a compile error on both
//! sides of the workspace boundary rather than a runtime surprise in cosim.

use serde::{Deserialize, Serialize};

use crate::clock::ClockPeriod;
use crate::error::ParseError;
use crate::floorplan::FloorplanResult;
use crate::graph::TaskGraph;

/// Name of the single state file: `<work_dir>/tapa.json`, copied under the
/// same name to the root of the `.zip` archive `tapa pack` emits.
pub const FILE_NAME: &str = "tapa.json";

/// Schema version stamped into every [`WorkState`] this tapa writes.
///
/// Bump on any backward-incompatible change to the state shape — including
/// the nested [`TaskGraph`] wire form. Work dirs outlive tool versions, so a
/// mismatch must surface as a clear "re-run analyze" error, not as a
/// confusing field-level parse failure. Purely *additive* fields carried with
/// `#[serde(default, skip_serializing_if = "Option::is_none")]` are
/// backward-compatible — old files parse unchanged and new files omit the
/// field until it is populated — so they do not require a bump (the cosim
/// port metadata on [`crate::port::Port`] landed that way).
///
/// v2 added the optional [`WorkState::floorplan`] contract; v3 made routed
/// channel identities variant-specific.
pub const VERSION: u32 = 3;

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
    /// The floorplan the `floorplan` step computed, when it has run. Its
    /// presence switches codegen and pack into the floorplanned path; absent,
    /// the flow is byte-for-byte as before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub floorplan: Option<FloorplanResult>,
}

impl WorkState {
    /// Wrap `graph` with the current [`VERSION`] and pre-synth (default)
    /// flow settings.
    #[must_use]
    pub fn new(graph: TaskGraph) -> Self {
        Self {
            version: VERSION,
            graph,
            flow: FlowSettings::default(),
            floorplan: None,
        }
    }

    /// Parse a state payload with field-path error diagnostics.
    ///
    /// Strict: [`deny_unknown_fields`](WorkState) applies all the way down,
    /// so any key the model does not know is an error rather than something
    /// silently carried along.
    pub fn from_json(json: &str) -> Result<Self, ParseError> {
        let de = &mut serde_json::Deserializer::from_str(json);
        serde_path_to_error::deserialize(de).map_err(|e| ParseError::Schema {
            path: e.path().to_string(),
            message: e.inner().to_string(),
        })
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
    /// `--platform`. `pack` reads it back to build the `.xo`, and `frt-cosim`
    /// reads it out of the packed archive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub part_num: Option<String>,
    /// Vitis platform `synth --platform` ran against, when given. Read by
    /// `pack --bitstream-script` to render the `v++` invocation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
    /// The **requested** target clock period.
    ///
    /// Distinct from [`crate::Task::clock_period`], which is the per-task
    /// *achieved* estimate HLS reports back: one is an input to synthesis,
    /// the other a result of it. Both are kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clock_period: Option<ClockPeriod>,
    /// Set once `synth` has completed for this work dir.
    #[serde(default)]
    pub synthed: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::BTreeMap;

    use crate::{SynthTarget, Target, Task, TaskLevel};

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
                self_area: None,
                total_area: None,
                clock_period: None,
            },
        );
        WorkState::new(TaskGraph {
            schema_version: crate::graph::SCHEMA_VERSION,
            top: "Top".to_string(),
            target: Target::XilinxHls,
            tasks,
            cflags: Vec::new(),
        })
    }

    #[test]
    fn new_stamps_the_current_version() {
        assert_eq!(
            sample_state().version,
            VERSION,
            "`new` must stamp the version readers check",
        );
    }

    #[test]
    fn round_trips_through_json() {
        let mut state = sample_state();
        state.flow.part_num = Some("xcvu37p".to_string());
        state.flow.platform = Some("xilinx_u250_gen3x16_xdma_4_1_202210_1".to_string());
        state.flow.clock_period = Some(ClockPeriod::from_picoseconds(3330));
        state.flow.synthed = true;
        let json = serde_json::to_string(&state).expect("serialize");
        let back = WorkState::from_json(&json).expect("parse");
        assert_eq!(back, state, "state must survive a JSON round trip");
    }

    #[test]
    fn floorplan_is_absent_by_default_and_omitted_from_json() {
        let state = sample_state();
        assert!(
            state.floorplan.is_none(),
            "a fresh state is not floorplanned"
        );
        let json = serde_json::to_string(&state).expect("serialize");
        assert!(
            !json.contains("floorplan"),
            "an absent floorplan must not materialize a key; got {json}",
        );
    }

    #[test]
    fn floorplan_round_trips_when_present() {
        use crate::floorplan::{Area, FloorplanResult};

        let mut state = sample_state();
        state.floorplan = Some(FloorplanResult {
            device: "u280".to_string(),
            grid: (2, 3),
            regions: BTreeMap::from([("Top".to_string(), "SLOT_X0Y0_TO_SLOT_X0Y0".to_string())]),
            routes: Vec::new(),
            slot_usage: BTreeMap::from([(
                "SLOT_X0Y0_TO_SLOT_X0Y0".to_string(),
                Area {
                    lut: 1,
                    ff: 2,
                    bram_18k: 0,
                    dsp: 0,
                    uram: 0,
                },
            )]),
        });
        let json = serde_json::to_string(&state).expect("serialize");
        let back = WorkState::from_json(&json).expect("parse");
        assert_eq!(
            back, state,
            "a floorplanned state must survive a round trip"
        );
    }

    #[test]
    fn version_is_v3_for_typed_pipeline_routes() {
        assert_eq!(VERSION, 3, "typed pipeline routes require state v3");
    }

    #[test]
    fn unknown_state_field_is_rejected() {
        // `deny_unknown_fields` is what keeps the state schema honest across
        // the two workspaces that parse it; pin it.
        let json = serde_json::to_string(&sample_state()).expect("serialize");
        let patched = json.replacen('{', r#"{"bogus":1,"#, 1);
        let err = WorkState::from_json(&patched).expect_err("unknown field must fail");
        assert!(
            err.to_string().contains("bogus"),
            "error must name the unknown field; got {err}",
        );
    }

    #[test]
    fn unknown_flow_field_is_rejected() {
        let json = serde_json::to_string(&sample_state()).expect("serialize");
        let patched = json.replace(r#""flow":{"#, r#""flow":{"bogus":1,"#);
        let err = WorkState::from_json(&patched).expect_err("unknown flow field must fail");
        assert!(
            err.to_string().contains("bogus"),
            "error must name the unknown field; got {err}",
        );
    }

    #[test]
    fn parse_error_carries_the_field_path() {
        // The reader that trips over a bad value must be told *where*, not
        // just that something somewhere failed to parse.
        let json = serde_json::to_string(&sample_state())
            .expect("serialize")
            .replace(r#""target":"xilinx-hls""#, r#""target":"cpu-sim""#);
        let err = WorkState::from_json(&json).expect_err("bad target must fail");
        let text = err.to_string();
        assert!(
            text.contains("graph.target"),
            "error must point at the offending field path; got {text}",
        );
    }

    #[test]
    fn unresolved_flow_settings_are_omitted_not_null() {
        // Absent optional settings must not materialize as `null`s that a
        // reader could mistake for "resolved to nothing".
        let json = serde_json::to_string(&sample_state().flow).expect("serialize");
        assert_eq!(
            json, r#"{"synthed":false}"#,
            "unresolved flow settings must be omitted entirely",
        );
    }

    #[test]
    fn flow_settings_default_to_pre_synth() {
        let flow = FlowSettings::default();
        assert!(!flow.synthed, "a fresh work dir has not been synthesized");
        assert!(flow.part_num.is_none(), "part number is resolved by synth");
    }
}
