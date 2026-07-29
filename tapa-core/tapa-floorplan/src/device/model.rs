//! The device model: a rows×cols grid of physical [`Slot`]s over a
//! shell-subtracted AMD FPGA, plus the integer-rectangle [`Coor`] algebra the
//! floorplan and routing ILPs run on.
//!
//! The model intentionally keeps only the coordinates and capacities needed by
//! the placement and routing formulations:
//!
//! * We model **grid** coordinates only — a slot lives at `(x, y)` with
//!   `x ∈ [0, cols)`, `y ∈ [0, rows)`, and a region is an inclusive rectangle
//!   of them. Separate *physical tile* coordinates (including "unset"
//!   sentinels) are not needed for the ILP, so [`Coor`] is `u32` and every
//!   conversion into the `i64` centroid space is a lossless `From`.
//! * Per-slot `area` is the resources **available** after the platform shell
//!   is subtracted; the usage limit derates it further at ILP time.

use serde::{Deserialize, Serialize};
use tapa_ir::Area;

/// Horizontal grid spacing between adjacent slot centroids.
pub const UNIT_DIST_X: i64 = 100;
/// Vertical grid spacing — larger than [`UNIT_DIST_X`] to price in the cost of
/// routing across an SLR (die) boundary.
pub const UNIT_DIST_Y: i64 = 150;
/// Physical distance one pipeline register hop spans.
pub const PP_DIST: i64 = 100;
/// Multiplier applied to vertical (SLR-crossing) distance in the floorplan
/// objective, so the ILP prefers to keep channels within a die.
pub const VERTICAL_DIST_PENALTY: i64 = 2;
/// Fraction of a boundary's raw wire capacity the router is allowed to use.
pub const USABLE_WIRE_RATIO: f64 = 0.7;
/// Default per-slot resource utilization target.
pub const DEFAULT_USAGE_LIMIT: f64 = 0.7;
/// Sentinel "no limit" wire capacity: a boundary with this cap never binds.
pub const WIRE_CAPACITY_INF: u64 = 100_000_000;

/// An inclusive integer rectangle of grid slots: the region spanning
/// `[dl_x, ur_x] × [dl_y, ur_y]`. A single slot is `dl == ur`.
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
    /// as a [`FloorplanResult`](tapa_ir::FloorplanResult) key and pblock name.
    #[must_use]
    pub fn region_name(&self) -> String {
        format!(
            "SLOT_X{}Y{}_TO_SLOT_X{}Y{}",
            self.dl_x, self.dl_y, self.ur_x, self.ur_y
        )
    }

    /// Parse a region tag produced by [`region_name`](Coor::region_name) back
    /// into a [`Coor`].
    #[must_use]
    pub fn from_region_name(name: &str) -> Option<Self> {
        let (lhs, rhs) = name.split_once("_TO_")?;
        let (dl_x, dl_y) = parse_slot_tag(lhs)?;
        let (ur_x, ur_y) = parse_slot_tag(rhs)?;
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

/// The five FPGA resource classes, in solver iteration order
/// (`FF, LUT, BRAM_18K, DSP, URAM`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Resource {
    Ff,
    Lut,
    Bram18k,
    Dsp,
    Uram,
}

impl Resource {
    /// All five classes, for iterating capacity constraints.
    pub const ALL: [Self; 5] = [Self::Ff, Self::Lut, Self::Bram18k, Self::Dsp, Self::Uram];

    /// This class's amount within an [`Area`].
    #[must_use]
    pub fn amount(self, area: &Area) -> u64 {
        match self {
            Self::Ff => area.ff,
            Self::Lut => area.lut,
            Self::Bram18k => area.bram_18k,
            Self::Dsp => area.dsp,
            Self::Uram => area.uram,
        }
    }

    /// The uppercase annotation-key name of this class.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Ff => "FF",
            Self::Lut => "LUT",
            Self::Bram18k => "BRAM_18K",
            Self::Dsp => "DSP",
            Self::Uram => "URAM",
        }
    }
}

/// Per-direction wire crossing capacities of a slot boundary.
///
/// An omitted direction defaults to [`WIRE_CAPACITY_INF`] — a boundary with no
/// declared cap does not constrain routing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirCaps {
    #[serde(default = "wire_cap_inf")]
    pub north: u64,
    #[serde(default = "wire_cap_inf")]
    pub south: u64,
    #[serde(default = "wire_cap_inf")]
    pub east: u64,
    #[serde(default = "wire_cap_inf")]
    pub west: u64,
}

fn wire_cap_inf() -> u64 {
    WIRE_CAPACITY_INF
}

impl Default for DirCaps {
    fn default() -> Self {
        Self {
            north: WIRE_CAPACITY_INF,
            south: WIRE_CAPACITY_INF,
            east: WIRE_CAPACITY_INF,
            west: WIRE_CAPACITY_INF,
        }
    }
}

/// Per-direction anchor pblock ranges: where a boundary-crossing pipeline
/// register may be physically placed on each side of the slot.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirRegions {
    #[serde(default)]
    pub north: Vec<String>,
    #[serde(default)]
    pub south: Vec<String>,
    #[serde(default)]
    pub east: Vec<String>,
    #[serde(default)]
    pub west: Vec<String>,
}

/// One physical slot on the device grid.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Slot {
    /// Grid column, `0..cols`.
    pub x: u32,
    /// Grid row, `0..rows` (row = SLR).
    pub y: u32,
    /// Resources available in this slot after the platform shell.
    pub area: Area,
    /// Centroid abscissa in the `(UNIT_DIST_X, UNIT_DIST_Y)` metric space.
    pub centroid_x: i64,
    /// Centroid ordinate.
    pub centroid_y: i64,
    /// Vivado pblock range operations. A bare range is an implicit `-add` for
    /// compatibility with simple device tables; shell-shaped slots may use
    /// explicit `-add` and `-remove` clauses.
    #[serde(default)]
    pub pblock_ranges: Vec<String>,
    /// Wire-crossing budget on each boundary.
    #[serde(default)]
    pub wire_cap: DirCaps,
    /// Anchor pblock ranges for pipeline registers on each boundary.
    #[serde(default)]
    pub anchor: DirRegions,
    /// Slot tags such as `HBM[0]`, `DDR[0]`, `CLK_RST`.
    #[serde(default)]
    pub tags: Vec<String>,
}

impl Slot {
    /// This slot's one-cell [`Coor`].
    #[must_use]
    pub fn coor(&self) -> Coor {
        Coor::slot(self.x, self.y)
    }
}

/// A precollected FPGA device: its grid, per-slot resources, and geometry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Device {
    /// Short key, e.g. `"u280"`.
    pub key: String,
    /// Full Vivado part number, e.g. `"xcu280-fsvh2892-2L-e"`.
    pub part_num: String,
    /// Vitis platform this table targets, when it is platform-specific.
    #[serde(default)]
    pub platform_name: Option<String>,
    /// Number of grid rows (= SLRs).
    pub rows: u32,
    /// Number of grid columns.
    pub cols: u32,
    /// Physical distance one pipeline register hop spans.
    pub pp_dist: i64,
    /// Whether this is a Versal part (grid-only modeling, no NoC physics).
    #[serde(default)]
    pub is_versal: bool,
    /// User pblock name the floorplan XDC scopes cells under, if any.
    #[serde(default)]
    pub user_pblock_name: Option<String>,
    /// All slots, one per grid cell.
    pub slots: Vec<Slot>,
}

/// A semantic error in a parsed device table.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DeviceValidationError {
    /// An implementation pblock needs at least one physical range.
    #[error("slot ({x},{y}) has no pblock ranges")]
    MissingPblockRanges { x: u32, y: u32 },
    /// A range operation cannot be passed safely and unambiguously to Vivado.
    #[error("slot ({x},{y}) has invalid pblock range `{range}`")]
    InvalidPblockRange { x: u32, y: u32, range: String },
    /// The platform parent name is interpolated into generated Tcl.
    #[error("invalid user pblock name `{0}`")]
    InvalidUserPblockName(String),
}

impl Device {
    /// Validate the table values interpolated into implementation Tcl.
    pub fn validate(&self) -> Result<(), DeviceValidationError> {
        for slot in &self.slots {
            if slot.pblock_ranges.is_empty() {
                return Err(DeviceValidationError::MissingPblockRanges {
                    x: slot.x,
                    y: slot.y,
                });
            }
            for range in &slot.pblock_ranges {
                if !valid_pblock_range(range) {
                    return Err(DeviceValidationError::InvalidPblockRange {
                        x: slot.x,
                        y: slot.y,
                        range: range.clone(),
                    });
                }
            }
        }

        if let Some(name) = &self.user_pblock_name {
            let mut characters = name.chars();
            let valid_first = characters
                .next()
                .is_some_and(|character| character.is_ascii_alphabetic() || character == '_');
            if !valid_first
                || name.starts_with("__")
                || !characters
                    .all(|character| character.is_ascii_alphanumeric() || character == '_')
            {
                return Err(DeviceValidationError::InvalidUserPblockName(name.clone()));
            }
        }

        Ok(())
    }

    /// The slot at grid `(x, y)`, if it exists.
    #[must_use]
    pub fn slot(&self, x: u32, y: u32) -> Option<&Slot> {
        self.slots.iter().find(|s| s.x == x && s.y == y)
    }

    /// All slots carrying an exact device tag such as `HBM[7]` or
    /// `S_AXI_CONTROL`.
    pub fn slots_with_tag<'a>(&'a self, tag: &'a str) -> impl Iterator<Item = &'a Slot> + 'a {
        self.slots
            .iter()
            .filter(move |slot| slot.tags.iter().any(|candidate| candidate == tag))
    }

    /// The centroid of a region: the midpoint of its down-left and up-right
    /// slot centroids. `None` if either corner slot is missing.
    #[must_use]
    pub fn island_centroid(&self, region: &Coor) -> Option<(i64, i64)> {
        let dl = self.slot(region.dl_x, region.dl_y)?;
        let ur = self.slot(region.ur_x, region.ur_y)?;
        Some((
            i64::midpoint(dl.centroid_x, ur.centroid_x),
            i64::midpoint(dl.centroid_y, ur.centroid_y),
        ))
    }

    /// The summed resources of every slot in a region. `None` if any covered
    /// cell has no slot.
    #[must_use]
    pub fn island_area(&self, region: &Coor) -> Option<Area> {
        let mut total = Area::default();
        for (x, y) in region.all_slot_coors() {
            let slot = self.slot(x, y)?;
            total = add_area(total, slot.area);
        }
        Some(total)
    }
}

fn valid_pblock_range(range: &str) -> bool {
    if range.trim() != range
        || range.is_empty()
        || range
            .chars()
            .any(|character| character.is_control() || matches!(character, ';' | '$' | '[' | ']'))
    {
        return false;
    }

    let payload = if let Some(payload) = range.strip_prefix("-add ") {
        payload
    } else if let Some(payload) = range.strip_prefix("-remove ") {
        payload
    } else if range.starts_with('-') {
        return false;
    } else {
        range
    };
    if payload.is_empty() {
        return false;
    }

    let payload = if let Some(payload) = payload.strip_prefix('{') {
        let Some(payload) = payload.strip_suffix('}') else {
            return false;
        };
        payload
    } else if payload.ends_with('}') {
        return false;
    } else {
        payload
    };
    !payload.is_empty()
        && payload.split_ascii_whitespace().all(|item| {
            item.contains("_X")
                && item.contains('Y')
                && item.chars().all(|character| {
                    character.is_ascii_alphanumeric() || matches!(character, '_' | ':')
                })
        })
}

/// Component-wise sum of two [`Area`]s. A free function because `Area` is
/// defined in `tapa-ir`, so the orphan rule forbids an `impl Add` here.
#[must_use]
pub fn add_area(a: Area, b: Area) -> Area {
    Area {
        lut: a.lut + b.lut,
        ff: a.ff + b.ff,
        bram_18k: a.bram_18k + b.bram_18k,
        dsp: a.dsp + b.dsp,
        uram: a.uram + b.uram,
    }
}

/// The floorplan objective's distance between two centroids:
/// `|Δx| + penalty·|Δy|`, penalizing SLR-crossing (vertical) distance.
#[must_use]
pub fn penalized_distance(a: (i64, i64), b: (i64, i64), vertical_penalty: i64) -> i64 {
    (a.0 - b.0).abs() + vertical_penalty * (a.1 - b.1).abs()
}

#[cfg(test)]
mod tests {

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
    use super::*;

    /// A 2×2 device with centroids on the `(100, 150)` grid, used for the
    /// geometry tests.
    fn grid_2x2() -> Device {
        let slot = |x: u32, y: u32, lut: u64| Slot {
            x,
            y,
            area: Area {
                lut,
                ff: 0,
                bram_18k: 0,
                dsp: 0,
                uram: 0,
            },
            centroid_x: UNIT_DIST_X * i64::from(x),
            centroid_y: UNIT_DIST_Y * i64::from(y),
            pblock_ranges: vec!["CLOCKREGION_X0Y0:CLOCKREGION_X0Y0".to_string()],
            wire_cap: DirCaps::default(),
            anchor: DirRegions::default(),
            tags: Vec::new(),
        };
        Device {
            key: "toy".to_string(),
            part_num: "xctoy".to_string(),
            platform_name: None,
            rows: 2,
            cols: 2,
            pp_dist: PP_DIST,
            is_versal: false,
            user_pblock_name: None,
            slots: vec![
                slot(0, 0, 10),
                slot(1, 0, 20),
                slot(0, 1, 30),
                slot(1, 1, 40),
            ],
        }
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
    fn island_centroid_is_the_corner_midpoint() {
        let device = grid_2x2();
        // Single slot: centroid is the slot's own.
        assert_eq!(device.island_centroid(&Coor::slot(1, 1)), Some((100, 150)));
        // Whole device: midpoint of (0,0)=(0,0) and (1,1)=(100,150).
        assert_eq!(
            device.island_centroid(&Coor::span(0, 0, 1, 1)),
            Some((50, 75)),
        );
        assert_eq!(device.island_centroid(&Coor::slot(9, 9)), None);
    }

    #[test]
    fn island_area_sums_covered_slots() {
        let device = grid_2x2();
        // 10 + 20 + 30 + 40 across the whole grid.
        assert_eq!(
            device.island_area(&Coor::span(0, 0, 1, 1)).map(|a| a.lut),
            Some(100),
        );
        assert_eq!(
            device.island_area(&Coor::slot(0, 1)).map(|a| a.lut),
            Some(30),
        );
    }

    #[test]
    fn penalized_distance_weights_vertical() {
        // Horizontal hop of 100 costs 100; vertical hop of 150 costs 300.
        assert_eq!(
            penalized_distance((0, 0), (100, 0), VERTICAL_DIST_PENALTY),
            100
        );
        assert_eq!(
            penalized_distance((0, 0), (0, 150), VERTICAL_DIST_PENALTY),
            300
        );
    }

    #[test]
    fn wire_caps_default_to_infinite() {
        assert_eq!(DirCaps::default().north, WIRE_CAPACITY_INF);
        let partial: DirCaps = serde_json::from_str(r#"{"north": 11520}"#).expect("parse");
        assert_eq!(partial.north, 11520);
        assert_eq!(
            partial.south, WIRE_CAPACITY_INF,
            "omitted caps are infinite"
        );
    }

    #[test]
    fn device_validation_accepts_explicit_range_operations() {
        let mut device = grid_2x2();
        device.user_pblock_name = Some("pblock_dynamic_region".to_string());
        device.slots[0].pblock_ranges = vec![
            "-add {CLOCKREGION_X0Y0:CLOCKREGION_X3Y3}".to_string(),
            "-remove CLOCKREGION_X2Y0:CLOCKREGION_X3Y3".to_string(),
        ];
        device.validate().expect("valid device table");
    }

    #[test]
    fn device_validation_rejects_unsafe_range_and_parent_tokens() {
        let mut device = grid_2x2();
        device.slots[0].pblock_ranges =
            vec!["-add CLOCKREGION_X0Y0:CLOCKREGION_X3Y3; puts bad".to_string()];
        assert!(matches!(
            device.validate(),
            Err(DeviceValidationError::InvalidPblockRange { x: 0, y: 0, .. })
        ));

        let mut device = grid_2x2();
        device.user_pblock_name = Some("bad parent".to_string());
        assert_eq!(
            device.validate(),
            Err(DeviceValidationError::InvalidUserPblockName(
                "bad parent".to_string()
            ))
        );
    }
}
