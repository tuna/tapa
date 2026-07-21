//! The floorplan contract: the plain-data result the `tapa floorplan` planner
//! writes into [`WorkState`](crate::WorkState) and codegen reads back.
//!
//! `tapa-floorplan` (the planner) and `tapa-codegen` (the consumer) never
//! depend on each other; they meet only here, through these serde types. The
//! planner assigns every flattened instance to a physical grid *region* and
//! records how each cross-region channel is pipelined; codegen turns those
//! stream records into Head/Body/Tail handshake pipelines and pblock
//! constraints.

use std::collections::BTreeMap;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Resource counts for the five classes TAPA floorplans on.
///
/// Fields are stored in struct order `lut, ff, bram_18k, dsp, uram`; the
/// upstream `RESOURCES` iteration order is `FF, LUT, BRAM_18K, DSP, URAM`,
/// which consumers reproduce where solver parity matters. The keys in the
/// untyped [`Task::self_area`](crate::Task::self_area) /
/// [`Task::total_area`](crate::Task::total_area) maps are the uppercase
/// `LUT`/`FF`/`BRAM_18K`/`DSP`/`URAM` that synth writes;
/// [`Area::from_annotations`] reads exactly those.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Area {
    pub lut: u64,
    pub ff: u64,
    pub bram_18k: u64,
    pub dsp: u64,
    pub uram: u64,
}

impl Area {
    /// Read a typed [`Area`] out of the untyped `LUT`/`FF`/`BRAM_18K`/`DSP`/
    /// `URAM` annotation map synth writes into a task's area fields.
    ///
    /// A missing or non-integer entry counts as zero: an unannotated task
    /// occupies no area, rather than poisoning the whole conversion.
    #[must_use]
    pub fn from_annotations(map: &IndexMap<String, Value>) -> Self {
        let get = |key: &str| map.get(key).and_then(Value::as_u64).unwrap_or(0);
        Self {
            lut: get("LUT"),
            ff: get("FF"),
            bram_18k: get("BRAM_18K"),
            dsp: get("DSP"),
            uram: get("URAM"),
        }
    }
}

/// One pipelined channel that crosses a slot boundary.
///
/// Stream crossings are rendered as named Head/Body/Tail handshake pipelines.
/// The contract also reserves an M-AXI (`mmap`) kind for future codegen
/// support. The planner records the slot [`route`](Crossing::route) and exact
/// Body-cell regions; codegen and XDC emission replay supported records.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Crossing {
    /// Whether this crossing is a stream or an M-AXI link.
    pub kind: CrossingKind,
    /// The channel key. For [`CrossingKind::Stream`] it is the
    /// [`Task::fifos`](crate::Task::fifos) map key (the interconnect name);
    /// for [`CrossingKind::Axi`] it is the mmap argument name.
    pub link: String,
    /// The slot path the channel takes, head to tail, e.g.
    /// `["SLOT_X0Y0", "SLOT_X0Y1"]`.
    pub route: Vec<String>,
    /// Number of Body pipeline cells inserted along the route. Head and Tail
    /// are endpoint cells and are not included (`BODY_LEVEL` convention).
    pub level: u32,
    /// How registers are distributed across the route's hops.
    pub scheme: PipelineScheme,
    /// Ordered per-Body-cell slot assignment (one entry per Body cell).
    pub reg_regions: Vec<String>,
}

/// What kind of net a [`Crossing`] pipelines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CrossingKind {
    /// A `tapa::stream` channel — pipelined with Head/Body/Tail handshake cells.
    Stream,
    /// A reserved M-AXI (`mmap`) link kind; codegen support is not implemented.
    Axi,
}

/// How pipeline registers are distributed across a route's hops.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PipelineScheme {
    /// One register per intermediate slot.
    Single,
    /// Two registers per hop.
    Double,
    /// One register per horizontal hop, two per vertical (SLR) hop.
    SingleHDoubleV,
}

/// The planner's output for one design: where every instance lands and how
/// every cross-region channel is pipelined.
///
/// Stored in [`WorkState`](crate::WorkState) so it is resumable and
/// inspectable in `tapa.json`; read by codegen and by XDC emission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FloorplanResult {
    /// Resolved device key, e.g. `"u280"`.
    pub device: String,
    /// Floorplan grid as `(cols, rows)`.
    pub grid: (u32, u32),
    /// Flattened instance canonical name → region tag, e.g.
    /// `"SLOT_X0Y0_TO_SLOT_X1Y0"`.
    pub regions: BTreeMap<String, String>,
    /// One entry per pipelined channel.
    pub crossings: Vec<Crossing>,
    /// Achieved per-slot resource usage, keyed by region tag; for reporting
    /// and the empty-pblock / over-capacity DRCs.
    pub slot_usage: BTreeMap<String, Area>,
}

#[cfg(test)]
mod tests {
    use super::*;

    use serde_json::json;

    #[test]
    fn area_reads_the_uppercase_annotation_keys() {
        let map: IndexMap<String, Value> = IndexMap::from([
            ("LUT".to_string(), json!(10)),
            ("FF".to_string(), json!(20)),
            ("BRAM_18K".to_string(), json!(3)),
            ("DSP".to_string(), json!(4)),
            ("URAM".to_string(), json!(5)),
        ]);
        assert_eq!(
            Area::from_annotations(&map),
            Area {
                lut: 10,
                ff: 20,
                bram_18k: 3,
                dsp: 4,
                uram: 5
            },
        );
    }

    #[test]
    fn area_missing_entries_are_zero() {
        let map: IndexMap<String, Value> = IndexMap::from([("LUT".to_string(), json!(7))]);
        assert_eq!(
            Area::from_annotations(&map),
            Area {
                lut: 7,
                ..Area::default()
            },
            "an unannotated resource occupies nothing",
        );
    }

    fn sample_result() -> FloorplanResult {
        FloorplanResult {
            device: "u280".to_string(),
            grid: (2, 3),
            regions: BTreeMap::from([
                ("Top/a".to_string(), "SLOT_X0Y0_TO_SLOT_X0Y0".to_string()),
                ("Top/b".to_string(), "SLOT_X1Y2_TO_SLOT_X1Y2".to_string()),
            ]),
            crossings: vec![
                Crossing {
                    kind: CrossingKind::Stream,
                    link: "fifo_a_b".to_string(),
                    route: vec!["SLOT_X0Y0".to_string(), "SLOT_X1Y2".to_string()],
                    level: 4,
                    scheme: PipelineScheme::Double,
                    reg_regions: vec!["SLOT_X0Y1".to_string(), "SLOT_X1Y1".to_string()],
                },
                Crossing {
                    kind: CrossingKind::Axi,
                    link: "arg_mem".to_string(),
                    route: vec!["SLOT_X1Y2".to_string(), "SLOT_X1Y0".to_string()],
                    level: 3,
                    scheme: PipelineScheme::SingleHDoubleV,
                    reg_regions: vec!["SLOT_X1Y1".to_string()],
                },
            ],
            slot_usage: BTreeMap::from([(
                "SLOT_X0Y0_TO_SLOT_X0Y0".to_string(),
                Area {
                    lut: 100,
                    ff: 200,
                    bram_18k: 1,
                    dsp: 2,
                    uram: 0,
                },
            )]),
        }
    }

    #[test]
    fn floorplan_result_round_trips() {
        let result = sample_result();
        let json = serde_json::to_string(&result).expect("serialize");
        let back: FloorplanResult = serde_json::from_str(&json).expect("parse");
        assert_eq!(back, result, "the contract must survive a JSON round trip");
    }

    #[test]
    fn enum_tags_are_snake_case() {
        // The JSON tags are the CLI's `--pp-scheme` spellings; pin them.
        assert_eq!(
            serde_json::to_string(&CrossingKind::Axi).unwrap(),
            r#""axi""#
        );
        assert_eq!(
            serde_json::to_string(&PipelineScheme::SingleHDoubleV).unwrap(),
            r#""single_h_double_v""#,
        );
    }

    #[test]
    fn floorplan_result_rejects_unknown_fields() {
        let json = serde_json::to_string(&sample_result()).expect("serialize");
        let patched = json.replacen('{', r#"{"bogus":1,"#, 1);
        serde_json::from_str::<FloorplanResult>(&patched)
            .expect_err("unknown field must be rejected");
    }
}
