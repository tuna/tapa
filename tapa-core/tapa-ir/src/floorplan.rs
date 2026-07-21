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

use crate::connectivity::MemoryBank;

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

/// Child-side identity of an M-AXI interface routed to external memory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AxiEndpoint {
    /// Canonical flattened child instance name.
    pub instance: String,
    /// M-AXI port name on the child instance.
    pub port: String,
    /// Corresponding top-level kernel port.
    pub top_port: String,
}

/// One independently pipelined AXI protocol channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AxiChannel {
    ReadAddress,
    ReadData,
    WriteAddress,
    WriteData,
    WriteResponse,
}

/// A distributed-control bundle with uniform routing semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlChannel {
    /// Start/release, scalar arguments, and mmap offsets sent to a child.
    Launch,
    /// Reset distribution, which requires reset-specific pipeline semantics.
    Reset,
    /// Completion returned from a child to the global controller.
    Completion,
}

/// Identity of one channel carried by a [`PipelineRoute`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RoutedChannel {
    /// One internal `tapa::stream`, identified by its flattened FIFO name.
    Stream { fifo: String },
    /// One AXI protocol channel between a child endpoint and a memory bank.
    Axi {
        endpoint: AxiEndpoint,
        bank: MemoryBank,
        channel: AxiChannel,
    },
    /// One control bundle associated with a flattened child instance.
    Control {
        instance: String,
        channel: ControlChannel,
    },
}

/// Physical route and register placement for one typed channel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PipelineRoute {
    /// Channel-specific identity; unrelated identities cannot be combined.
    pub channel: RoutedChannel,
    /// The slot path the channel takes, head to tail, e.g.
    /// `["SLOT_X0Y0", "SLOT_X0Y1"]`.
    pub route: Vec<String>,
    /// How registers are distributed across the route's hops.
    pub scheme: PipelineScheme,
    /// Ordered per-Body-cell slot assignment (one entry per Body cell).
    pub reg_regions: Vec<String>,
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
    pub routes: Vec<PipelineRoute>,
    /// Achieved per-slot resource usage, including generated stream pipeline
    /// storage and registers; keyed by region tag for reporting and DRCs.
    pub slot_usage: BTreeMap<String, Area>,
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::connectivity::MemoryKind;
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
            routes: vec![
                PipelineRoute {
                    channel: RoutedChannel::Stream {
                        fifo: "fifo_a_b".to_string(),
                    },
                    route: vec!["SLOT_X0Y0".to_string(), "SLOT_X1Y2".to_string()],
                    scheme: PipelineScheme::Double,
                    reg_regions: vec!["SLOT_X0Y1".to_string(), "SLOT_X1Y1".to_string()],
                },
                PipelineRoute {
                    channel: RoutedChannel::Axi {
                        endpoint: AxiEndpoint {
                            instance: "reader_0".to_string(),
                            port: "mem".to_string(),
                            top_port: "arg_mem".to_string(),
                        },
                        bank: MemoryBank {
                            kind: MemoryKind::Hbm,
                            index: 0,
                        },
                        channel: AxiChannel::ReadData,
                    },
                    route: vec!["SLOT_X1Y2".to_string(), "SLOT_X1Y0".to_string()],
                    scheme: PipelineScheme::SingleHDoubleV,
                    reg_regions: vec!["SLOT_X1Y1".to_string()],
                },
                PipelineRoute {
                    channel: RoutedChannel::Control {
                        instance: "reader_0".to_string(),
                        channel: ControlChannel::Launch,
                    },
                    route: vec!["SLOT_X0Y0".to_string(), "SLOT_X1Y2".to_string()],
                    scheme: PipelineScheme::Double,
                    reg_regions: vec!["SLOT_X0Y1".to_string()],
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
            serde_json::to_string(&AxiChannel::WriteAddress).unwrap(),
            r#""write_address""#
        );
        assert_eq!(
            serde_json::to_string(&ControlChannel::Completion).unwrap(),
            r#""completion""#
        );
        assert_eq!(
            serde_json::to_string(&PipelineScheme::SingleHDoubleV).unwrap(),
            r#""single_h_double_v""#,
        );
    }

    #[test]
    fn routed_channel_rejects_fields_from_another_variant() {
        let invalid = json!({
            "kind": "stream",
            "fifo": "q",
            "bank": {"kind": "hbm", "index": 0}
        });
        serde_json::from_value::<RoutedChannel>(invalid)
            .expect_err("a stream cannot carry AXI identity");
    }

    #[test]
    fn floorplan_result_rejects_unknown_fields() {
        let json = serde_json::to_string(&sample_result()).expect("serialize");
        let patched = json.replacen('{', r#"{"bogus":1,"#, 1);
        serde_json::from_str::<FloorplanResult>(&patched)
            .expect_err("unknown field must be rejected");
    }
}
