//! The floorplan contract: the plain-data result the `tapa floorplan` planner
//! writes into [`WorkState`](crate::WorkState) and codegen reads back.
//!
//! `tapa-floorplan` (the planner) and `tapa-codegen` (the consumer) never
//! depend on each other; they meet only here, through these contract
//! types — serde records plus the shared [`Coor`] region-tag geometry.
//! The
//! planner assigns every flattened instance to a physical grid *region* and
//! records how each cross-region channel is pipelined; codegen turns those
//! stream records into Head/Body/Tail handshake pipelines and pblock
//! constraints.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::connectivity::MemoryBank;

/// Physical storage depth used for a shallow FIFO while floorplanning.
///
/// A shallow co-located FIFO uses an almost-full Tail with one cycle of
/// registered ready feedback.  One grace entry plus four safety entries keep
/// its logical capacity unchanged.  Deeper FIFOs retain their original
/// implementation and depth.
#[must_use]
pub const fn floorplanned_fifo_storage_depth(logical_depth: u32) -> u32 {
    if logical_depth <= 64 {
        logical_depth + 5
    } else {
        logical_depth
    }
}

/// Resource counts for the five classes TAPA floorplans on.
///
/// One count model with one spelling: the device tables, the floorplan
/// result, and the [`Task::self_area`](crate::Task::self_area) /
/// [`Task::total_area`](crate::Task::total_area) annotations all serialize
/// as these five fields. They are exactly the resources the HLS and
/// utilization report readers produce. A resource absent from a document is
/// zero of that resource; a value that is not a count fails to parse rather
/// than silently becoming zero.
///
/// Fields are stored in struct order `lut, ff, bram_18k, dsp, uram`; the
/// upstream `RESOURCES` iteration order is `FF, LUT, BRAM_18K, DSP, URAM`,
/// which consumers reproduce where solver parity matters.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Area {
    #[serde(default)]
    pub lut: u64,
    #[serde(default)]
    pub ff: u64,
    #[serde(default)]
    pub bram_18k: u64,
    #[serde(default)]
    pub dsp: u64,
    #[serde(default)]
    pub uram: u64,
}

impl Area {
    /// Add `count` instances' worth of `other`, refusing to wrap.
    ///
    /// Returns `None` on overflow so callers can name the task whose
    /// aggregate blew up.
    #[must_use]
    pub fn checked_add_scaled(self, other: Self, count: u64) -> Option<Self> {
        let add = |a: u64, b: u64| a.checked_add(b.checked_mul(count)?);
        Some(Self {
            lut: add(self.lut, other.lut)?,
            ff: add(self.ff, other.ff)?,
            bram_18k: add(self.bram_18k, other.bram_18k)?,
            dsp: add(self.dsp, other.dsp)?,
            uram: add(self.uram, other.uram)?,
        })
    }
}

/// An inclusive integer rectangle of grid slots: the region spanning
/// `[dl_x, ur_x] × [dl_y, ur_y]`. A single slot is `dl == ur`.
///
/// The geometry behind the region tags that are the wire format of
/// [`FloorplanResult::regions`] and the slot paths of [`PipelineRoute`]:
/// the planner encodes a rectangle with [`region_name`](Coor::region_name)
/// and codegen decodes it with the `from_*_name` parsers, so the two
/// engines agree on the tags without depending on each other. `Coor`
/// itself has no serde form — only its string tag crosses the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Coor {
    pub dl_x: u32,
    pub dl_y: u32,
    pub ur_x: u32,
    pub ur_y: u32,
}

impl Coor {
    /// The one-slot region at grid `(x, y)`.
    #[must_use]
    pub fn slot(x: u32, y: u32) -> Self {
        Self {
            dl_x: x,
            dl_y: y,
            ur_x: x,
            ur_y: y,
        }
    }

    /// The region spanning `(dl_x, dl_y)` to `(ur_x, ur_y)` inclusive.
    #[must_use]
    pub fn span(dl_x: u32, dl_y: u32, ur_x: u32, ur_y: u32) -> Self {
        Self {
            dl_x,
            dl_y,
            ur_x,
            ur_y,
        }
    }

    /// Slot count along x (inclusive).
    #[must_use]
    pub fn width(&self) -> u32 {
        self.ur_x.saturating_sub(self.dl_x) + 1
    }

    /// Slot count along y (inclusive).
    #[must_use]
    pub fn height(&self) -> u32 {
        self.ur_y.saturating_sub(self.dl_y) + 1
    }

    /// `self` sits directly *south* of `other` (shares `other`'s bottom edge)
    /// with overlapping x-extent.
    #[must_use]
    pub fn is_south_neighbor_of(&self, other: &Self) -> bool {
        self.ur_y + 1 == other.dl_y && self.dl_x.max(other.dl_x) <= self.ur_x.min(other.ur_x)
    }

    /// `self` sits directly *north* of `other`.
    #[must_use]
    pub fn is_north_neighbor_of(&self, other: &Self) -> bool {
        self.dl_y == other.ur_y + 1 && self.dl_x.max(other.dl_x) <= self.ur_x.min(other.ur_x)
    }

    /// `self` sits directly *east* of `other`.
    #[must_use]
    pub fn is_east_neighbor_of(&self, other: &Self) -> bool {
        self.dl_x == other.ur_x + 1 && self.dl_y.max(other.dl_y) <= self.ur_y.min(other.ur_y)
    }

    /// `self` sits directly *west* of `other`.
    #[must_use]
    pub fn is_west_neighbor_of(&self, other: &Self) -> bool {
        self.ur_x + 1 == other.dl_x && self.dl_y.max(other.dl_y) <= self.ur_y.min(other.ur_y)
    }

    /// The two regions share an edge (in any of the four directions).
    #[must_use]
    pub fn is_neighbor(&self, other: &Self) -> bool {
        self.is_north_neighbor_of(other)
            || self.is_south_neighbor_of(other)
            || self.is_east_neighbor_of(other)
            || self.is_west_neighbor_of(other)
    }

    /// `self` is contained within `other` (inclusive).
    #[must_use]
    pub fn is_inside(&self, other: &Self) -> bool {
        self.dl_x >= other.dl_x
            && self.dl_y >= other.dl_y
            && self.ur_x <= other.ur_x
            && self.ur_y <= other.ur_y
    }

    /// The two regions overlap, counting a shared boundary as overlap.
    #[must_use]
    pub fn has_overlap(&self, other: &Self) -> bool {
        !(other.dl_x > self.ur_x
            || other.ur_x < self.dl_x
            || other.dl_y > self.ur_y
            || other.ur_y < self.dl_y)
    }

    /// `self` is a (non-strict) superset of `other`.
    #[must_use]
    pub fn covers(&self, other: &Self) -> bool {
        self.dl_x <= other.dl_x
            && self.dl_y <= other.dl_y
            && self.ur_x >= other.ur_x
            && self.ur_y >= other.ur_y
    }

    /// Every atomic grid cell `(x, y)` the region covers, row-major.
    #[must_use]
    pub fn all_slot_coors(&self) -> Vec<(u32, u32)> {
        let mut cells = Vec::with_capacity((self.width() * self.height()) as usize);
        for y in self.dl_y..=self.ur_y {
            for x in self.dl_x..=self.ur_x {
                cells.push((x, y));
            }
        }
        cells
    }

    /// The region tag `SLOT_X{dl}Y{dl}_TO_SLOT_X{ur}Y{ur}` this region carries
    /// as a [`FloorplanResult`] key and pblock name.
    #[must_use]
    pub fn region_name(&self) -> String {
        format!(
            "SLOT_X{}Y{}_TO_SLOT_X{}Y{}",
            self.dl_x, self.dl_y, self.ur_x, self.ur_y
        )
    }

    /// Parse a region tag produced by [`region_name`](Coor::region_name) back
    /// into a [`Coor`].
    ///
    /// Reversed ranges (`dl > ur` on either axis) are rejected rather than
    /// silently reinterpreted as their down-left slot.
    #[must_use]
    pub fn from_region_name(name: &str) -> Option<Self> {
        let (lhs, rhs) = name.split_once("_TO_")?;
        let (dl_x, dl_y) = parse_slot_tag(lhs)?;
        let (ur_x, ur_y) = parse_slot_tag(rhs)?;
        if dl_x > ur_x || dl_y > ur_y {
            return None;
        }
        Some(Self::span(dl_x, dl_y, ur_x, ur_y))
    }

    /// Parse a bare single-slot tag `SLOT_X{x}Y{y}` into a [`Coor`].
    #[must_use]
    pub fn from_slot_name(name: &str) -> Option<Self> {
        let (x, y) = parse_slot_tag(name)?;
        Some(Self::slot(x, y))
    }

    /// Parse either a region tag ([`from_region_name`](Coor::from_region_name))
    /// or a bare single-slot tag `SLOT_X{x}Y{y}` into a [`Coor`].
    #[must_use]
    pub fn from_region_or_slot_name(name: &str) -> Option<Self> {
        if let Some(coor) = Self::from_region_name(name) {
            return Some(coor);
        }
        Self::from_slot_name(name)
    }

    /// Parse a region or slot tag that denotes exactly one slot into that
    /// slot. A region tag is atomic only when its endpoints are identical;
    /// reversed ranges are rejected.
    #[must_use]
    pub fn from_atomic_region_name(name: &str) -> Option<Self> {
        let coor = Self::from_region_or_slot_name(name)?;
        (coor.dl_x == coor.ur_x && coor.dl_y == coor.ur_y).then_some(coor)
    }
}

/// Parse a single-slot tag `SLOT_X{x}Y{y}` into its grid coordinates.
fn parse_slot_tag(tag: &str) -> Option<(u32, u32)> {
    let rest = tag.strip_prefix("SLOT_X")?;
    let (x, y) = rest.split_once('Y')?;
    Some((x.parse().ok()?, y.parse().ok()?))
}

/// Child-side identity of an M-AXI interface routed to external memory.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AxiChannel {
    ReadAddress,
    ReadData,
    WriteAddress,
    WriteData,
    WriteResponse,
}

impl AxiChannel {
    /// Stable short name used in generated AXI pipeline hierarchy.
    #[must_use]
    pub const fn rtl_name(self) -> &'static str {
        match self {
            Self::ReadAddress => "ar",
            Self::ReadData => "r",
            Self::WriteAddress => "aw",
            Self::WriteData => "w",
            Self::WriteResponse => "b",
        }
    }
}

/// Deterministic generated instance name for one direct AXI channel pipeline.
#[must_use]
pub fn axi_pipeline_instance_name(endpoint: &AxiEndpoint, channel: AxiChannel) -> String {
    format!(
        "__tapa_axi_{}_{}",
        crate::port::sanitize_identifier_name(&endpoint.top_port),
        channel.rtl_name(),
    )
}

/// Deterministic generated instance name for an async-mmap-to-AXI bridge.
///
/// The name is derived from the external top port rather than an internal
/// signal prefix, so inserting an AXI pipeline does not change RTL hierarchy.
#[must_use]
pub fn async_mmap_bridge_instance_name(top_port: &str) -> String {
    format!("{}__m_axi", crate::port::sanitize_identifier_name(top_port))
}

/// Deterministic generated instance name for the global task controller.
#[must_use]
pub const fn global_controller_instance_name() -> &'static str {
    "__tapa_global_controller"
}

/// Deterministic generated instance name for a flattened task's local
/// controller.
#[must_use]
pub fn local_controller_instance_name(instance: &str) -> String {
    format!(
        "__tapa_local_controller_{}",
        crate::port::sanitize_identifier_name(instance),
    )
}

/// Physical bit widths of the five independent AXI ready/valid channels.
///
/// Each enabled value includes the channel payload plus its `VALID` and
/// `READY` bits. A zero value disables that channel; this represents the
/// read-only or write-only half pruned by an async mmap bridge. Plain mmap
/// interfaces enable all five channels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AxiChannelWidths {
    pub read_address: u32,
    pub read_data: u32,
    pub write_address: u32,
    pub write_data: u32,
    pub write_response: u32,
}

impl AxiChannelWidths {
    /// Return the physical width of `channel`, including handshake bits.
    #[must_use]
    pub const fn physical_width(self, channel: AxiChannel) -> u32 {
        match channel {
            AxiChannel::ReadAddress => self.read_address,
            AxiChannel::ReadData => self.read_data,
            AxiChannel::WriteAddress => self.write_address,
            AxiChannel::WriteData => self.write_data,
            AxiChannel::WriteResponse => self.write_response,
        }
    }

    /// Iterate in stable protocol order.
    #[must_use]
    pub const fn channels(self) -> [(AxiChannel, u32); 5] {
        [
            (AxiChannel::ReadAddress, self.read_address),
            (AxiChannel::ReadData, self.read_data),
            (AxiChannel::WriteAddress, self.write_address),
            (AxiChannel::WriteData, self.write_data),
            (AxiChannel::WriteResponse, self.write_response),
        ]
    }

    /// Iterate over channels that exist in generated RTL.
    pub fn enabled_channels(self) -> impl Iterator<Item = (AxiChannel, u32)> {
        self.channels().into_iter().filter(|(_, width)| *width != 0)
    }
}

/// A distributed-control bundle with uniform routing semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlChannel {
    /// Start/release, scalar arguments, and mmap offsets sent to a child.
    Launch,
    /// Reset distribution, which requires reset-specific pipeline semantics.
    Reset,
    /// Completion returned from a child to the global controller.
    Completion,
}

impl ControlChannel {
    /// Stable short name used in generated control-pipeline hierarchy.
    #[must_use]
    pub const fn rtl_name(self) -> &'static str {
        match self {
            Self::Launch => "launch",
            Self::Reset => "reset",
            Self::Completion => "completion",
        }
    }
}

/// Deterministic generated instance name for one flattened task's control
/// pipeline.
#[must_use]
pub fn control_pipeline_instance_name(instance: &str, channel: ControlChannel) -> String {
    format!(
        "__tapa_control_{}_{}",
        crate::port::sanitize_identifier_name(instance),
        channel.rtl_name(),
    )
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
    /// Horizontal hops behave like `Single` (one register per intermediate
    /// slot); each vertical (SLR) hop adds one register in each of its two
    /// endpoint slots, i.e. two per vertical hop.
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
    fn parses_region_slot_and_atomic_names() {
        assert_eq!(
            Coor::from_region_or_slot_name("SLOT_X1Y2"),
            Some(Coor::span(1, 2, 1, 2))
        );
        assert_eq!(
            Coor::from_region_or_slot_name("SLOT_X1Y2_TO_SLOT_X2Y3"),
            Some(Coor::span(1, 2, 2, 3))
        );
        assert_eq!(
            Coor::from_atomic_region_name("SLOT_X1Y2"),
            Some(Coor::span(1, 2, 1, 2))
        );
        assert_eq!(
            Coor::from_atomic_region_name("SLOT_X1Y2_TO_SLOT_X1Y2"),
            Some(Coor::span(1, 2, 1, 2))
        );
        assert_eq!(
            Coor::from_atomic_region_name("SLOT_X1Y2_TO_SLOT_X2Y2"),
            None
        );
    }

    #[test]
    fn reversed_region_tags_are_rejected() {
        assert_eq!(
            Coor::from_region_name("SLOT_X2Y0_TO_SLOT_X1Y0"),
            None,
            "dl_x > ur_x is malformed, not a one-slot region",
        );
        assert_eq!(
            Coor::from_region_name("SLOT_X0Y2_TO_SLOT_X0Y1"),
            None,
            "dl_y > ur_y is malformed, not a one-slot region",
        );
        assert_eq!(
            Coor::from_region_or_slot_name("SLOT_X2Y2_TO_SLOT_X1Y1"),
            None
        );
        // Forward ranges with one reversed axis are rejected as a whole.
        assert_eq!(Coor::from_region_name("SLOT_X0Y1_TO_SLOT_X1Y0"), None);
    }

    #[test]
    fn neighbor_tests_are_directional_and_edge_sharing() {
        let a = Coor::slot(0, 0);
        let b = Coor::slot(0, 1); // directly north of a
        let c = Coor::slot(1, 0); // directly east of a
        assert!(b.is_north_neighbor_of(&a), "b is north of a");
        assert!(a.is_south_neighbor_of(&b), "a is south of b");
        assert!(c.is_east_neighbor_of(&a), "c is east of a");
        assert!(a.is_west_neighbor_of(&c), "a is west of c");
        assert!(a.is_neighbor(&b) && a.is_neighbor(&c));
        // Diagonal slots share only a corner, so they are not neighbors.
        assert!(!Coor::slot(0, 0).is_neighbor(&Coor::slot(1, 1)));
    }

    #[test]
    fn overlap_covers_inside() {
        let region = Coor::span(0, 0, 1, 1);
        let cell = Coor::slot(1, 0);
        assert!(cell.is_inside(&region), "cell inside the 2x2 region");
        assert!(region.covers(&cell), "region covers the cell");
        assert!(region.has_overlap(&cell), "they overlap");
        // Adjacent-but-disjoint cells still count as overlapping on the shared
        // edge, matching the upstream convention.
        assert!(Coor::slot(0, 0).has_overlap(&Coor::slot(0, 0)));
        assert!(!Coor::slot(0, 0).is_inside(&Coor::slot(1, 0)));
    }

    #[test]
    fn width_height_and_cells() {
        let region = Coor::span(0, 0, 1, 1);
        assert_eq!((region.width(), region.height()), (2, 2));
        assert_eq!(
            region.all_slot_coors(),
            vec![(0, 0), (1, 0), (0, 1), (1, 1)],
            "cells are enumerated row-major",
        );
    }

    #[test]
    fn area_round_trips_its_five_counts() {
        let area: Area = serde_json::from_value(
            json!({"lut": 10, "ff": 20, "bram_18k": 3, "dsp": 4, "uram": 5}),
        )
        .expect("parse area");
        assert_eq!(
            area,
            Area {
                lut: 10,
                ff: 20,
                bram_18k: 3,
                dsp: 4,
                uram: 5
            },
        );
        assert_eq!(
            serde_json::to_value(area).expect("serialize area"),
            json!({"lut": 10, "ff": 20, "bram_18k": 3, "dsp": 4, "uram": 5}),
            "one spelling for every document that carries counts",
        );
    }

    #[test]
    fn area_rejects_a_value_that_is_not_a_count() {
        // The old reader turned this into a silent zero on one path and an
        // error on another.
        serde_json::from_value::<Area>(json!({"lut": "lots"})).expect_err("non-count lut");
    }

    #[test]
    fn scaled_accumulation_refuses_to_wrap() {
        let one = Area {
            lut: 1,
            ..Area::default()
        };
        assert_eq!(
            Area::default().checked_add_scaled(one, 3),
            Some(Area {
                lut: 3,
                ..Area::default()
            })
        );
        let huge = Area {
            lut: u64::MAX,
            ..Area::default()
        };
        assert_eq!(huge.checked_add_scaled(one, 1), None);
    }

    #[test]
    fn async_mmap_bridge_name_is_stable_and_sanitized() {
        assert_eq!(async_mmap_bridge_instance_name("mem[2]"), "mem_2__m_axi");
        assert_eq!(
            async_mmap_bridge_instance_name("mem-data"),
            "mem_data__m_axi"
        );
    }

    #[test]
    fn area_missing_entries_are_zero() {
        let area: Area = serde_json::from_value(json!({"lut": 7})).expect("parse area");
        assert_eq!(
            area,
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
    fn axi_pipeline_name_uses_the_unique_emitted_top_port() {
        let endpoint = AxiEndpoint {
            instance: "Module1Func#1".to_string(),
            port: "a_b".to_string(),
            top_port: "mem[3]".to_string(),
        };

        assert_eq!(
            axi_pipeline_instance_name(&endpoint, AxiChannel::ReadData),
            "__tapa_axi_mem_3_r",
        );
    }

    #[test]
    fn distributed_control_names_share_identifier_sanitization() {
        assert_eq!(
            global_controller_instance_name(),
            "__tapa_global_controller"
        );
        assert_eq!(
            local_controller_instance_name("Module1Func#1"),
            "__tapa_local_controller_Module1Func_1",
        );
        assert_eq!(
            control_pipeline_instance_name("worker[3]", ControlChannel::Completion),
            "__tapa_control_worker_3_completion",
        );
        assert_eq!(ControlChannel::Launch.rtl_name(), "launch");
        assert_eq!(ControlChannel::Reset.rtl_name(), "reset");
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
