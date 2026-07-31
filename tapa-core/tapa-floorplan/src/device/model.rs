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

pub use tapa_ir::floorplan::Coor;

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
/// Default per-slot resource utilization target.
pub const DEFAULT_USAGE_LIMIT: f64 = 0.7;
/// Sentinel "no limit" wire capacity: a boundary with this cap never binds.
pub const WIRE_CAPACITY_INF: u64 = 100_000_000;

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
    /// The grid must have at least one row and one column.
    #[error("device grid must have at least one row and one column")]
    EmptyGrid,
    /// A slot's coordinates are outside the declared grid.
    #[error("slot ({x},{y}) is outside the {cols}x{rows} grid")]
    SlotOutOfBounds {
        /// Out-of-bounds column.
        x: u32,
        /// Out-of-bounds row.
        y: u32,
        /// Declared grid columns.
        cols: u32,
        /// Declared grid rows.
        rows: u32,
    },
    /// Two slots claim the same grid cell.
    #[error("grid cell ({x},{y}) has more than one slot")]
    DuplicateSlot {
        /// Column of the duplicated cell.
        x: u32,
        /// Row of the duplicated cell.
        y: u32,
    },
    /// A grid cell has no slot. Sparse grids are not supported: real SSI
    /// devices are complete rectangles of full-width SLR dies, and the
    /// multilevel row pass and route candidate generation assume coverage.
    #[error("grid cell ({x},{y}) has no slot; device grids must be complete rectangles")]
    MissingSlot {
        /// Column of the missing cell.
        x: u32,
        /// Row of the missing cell.
        y: u32,
    },
    /// A slot centroid is off the unit-distance grid the placement objective
    /// assumes.
    #[error("slot ({x},{y}) centroid ({centroid_x},{centroid_y}) is off the unit grid")]
    CentroidOffGrid {
        /// Slot column.
        x: u32,
        /// Slot row.
        y: u32,
        /// Declared centroid x.
        centroid_x: i64,
        /// Declared centroid y.
        centroid_y: i64,
    },
    /// The table models exact external banks but records no platform, so the
    /// planner's platform check would silently no-op.
    #[error("slot ({x},{y}) carries bank tag `{tag}` but the table has no platform_name")]
    BankTagWithoutPlatform {
        /// Slot column.
        x: u32,
        /// Slot row.
        y: u32,
        /// The bank tag requiring a platform.
        tag: String,
    },
}

impl Device {
    /// Validate the table values interpolated into implementation Tcl and the
    /// structural grid invariants the planner relies on.
    pub fn validate(&self) -> Result<(), DeviceValidationError> {
        // The grid must be a complete rectangle: exactly one in-bounds slot
        // per (x, y).
        if self.rows == 0 || self.cols == 0 {
            return Err(DeviceValidationError::EmptyGrid);
        }
        let mut seen = std::collections::BTreeSet::new();
        for slot in &self.slots {
            if slot.x >= self.cols || slot.y >= self.rows {
                return Err(DeviceValidationError::SlotOutOfBounds {
                    x: slot.x,
                    y: slot.y,
                    cols: self.cols,
                    rows: self.rows,
                });
            }
            if !seen.insert((slot.x, slot.y)) {
                return Err(DeviceValidationError::DuplicateSlot {
                    x: slot.x,
                    y: slot.y,
                });
            }
        }
        let expected = usize::try_from(u64::from(self.rows) * u64::from(self.cols))
            .expect("grid size fits usize");
        if seen.len() != expected {
            let (x, y) = (0..self.rows)
                .flat_map(|y| (0..self.cols).map(move |x| (x, y)))
                .find(|cell| !seen.contains(cell))
                .expect("a shorter seen set implies a missing cell");
            return Err(DeviceValidationError::MissingSlot { x, y });
        }

        // Centroids must follow the unit grid the placement objective prices
        // distances in.
        for slot in &self.slots {
            let expected = (
                UNIT_DIST_X * i64::from(slot.x),
                UNIT_DIST_Y * i64::from(slot.y),
            );
            if (slot.centroid_x, slot.centroid_y) != expected {
                return Err(DeviceValidationError::CentroidOffGrid {
                    x: slot.x,
                    y: slot.y,
                    centroid_x: slot.centroid_x,
                    centroid_y: slot.centroid_y,
                });
            }
        }

        // Exact bank anchors are platform-specific: a table that models them
        // must record its platform or the planner's platform check no-ops.
        if self.platform_name.is_none() {
            for slot in &self.slots {
                for tag in &slot.tags {
                    if tag.parse::<tapa_ir::MemoryBank>().is_ok() {
                        return Err(DeviceValidationError::BankTagWithoutPlatform {
                            x: slot.x,
                            y: slot.y,
                            tag: tag.clone(),
                        });
                    }
                }
            }
        }

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

/// Return 70% of the raw wire capacity, rounded to the nearest integer (ties up).
///
/// Division-first: `raw / 10 * 7 + ((raw % 10) * 7 + 5) / 10` is the exact
/// round-half-up of `(raw * 7 + 5) / 10` (with `raw = 10q + r`, both forms
/// equal `7q + (7r + 5) / 10`), but never overflows `u64` for huge inputs.
#[must_use]
pub(crate) fn usable_wire_capacity(raw: u64) -> u64 {
    raw / 10 * 7 + ((raw % 10) * 7 + 5) / 10
}

/// The usable capacity of the shared border between two facing slot sides.
///
/// The *smaller* of the two facing declarations is taken first — either side
/// of the physical boundary may be the binding one — and the result is
/// derated to its usable share. Placement cuts and per-boundary routing
/// constraints both use this helper so the two stages always model the same
/// budget for a given boundary.
pub(crate) fn effective_border_capacity(lhs: u64, rhs: u64) -> u64 {
    usable_wire_capacity(lhs.min(rhs))
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
    fn usable_wire_capacity_rounds_half_up() {
        assert_eq!(usable_wire_capacity(45), 32, "31.5 rounds up to 32");
        assert_eq!(usable_wire_capacity(44), 31, "30.8 rounds to 31");
        assert_eq!(usable_wire_capacity(15), 11, "10.5 rounds up to 11");
        assert_eq!(usable_wire_capacity(0), 0, "0 stays 0");
    }

    /// The multiply-first form `(raw * 7 + 5) / 10` panics on overflow for
    /// huge raw capacities in debug builds; the division-first form must not.
    #[test]
    fn usable_wire_capacity_does_not_overflow_near_u64_max() {
        let raw = u64::MAX;
        // raw = 10q + 5 with q = 1_844_674_407_370_955_161, so the exact
        // round-half-up is 7q + (7*5 + 5)/10 = 7q + 4.
        assert_eq!(usable_wire_capacity(raw), 7 * 1_844_674_407_370_955_161 + 4,);
        assert_eq!(
            usable_wire_capacity(u64::MAX - 1),
            7 * 1_844_674_407_370_955_161 + 3
        );
    }

    #[test]
    fn effective_border_capacity_takes_the_minimum_then_derates() {
        assert_eq!(effective_border_capacity(100, 1), 1, "min(100, 1) * 0.7");
        assert_eq!(effective_border_capacity(1, 100), 1);
        assert_eq!(effective_border_capacity(10, 10), 7);
        assert_eq!(
            effective_border_capacity(WIRE_CAPACITY_INF, 0),
            0,
            "one unconstrained side does not lift the other side's zero",
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
    fn device_validation_requires_a_complete_rectangle_grid() {
        grid_2x2().validate().expect("the 2x2 toy is complete");

        // One cell missing: sparse grids are not supported.
        let mut sparse = grid_2x2();
        sparse.slots.retain(|slot| (slot.x, slot.y) != (1, 1));
        assert_eq!(
            sparse.validate(),
            Err(DeviceValidationError::MissingSlot { x: 1, y: 1 })
        );

        // Two slots on one cell.
        let mut duplicated = grid_2x2();
        let extra = duplicated.slots[0].clone();
        duplicated.slots.push(extra);
        assert_eq!(
            duplicated.validate(),
            Err(DeviceValidationError::DuplicateSlot { x: 0, y: 0 })
        );

        // A slot outside the declared grid.
        let mut out_of_bounds = grid_2x2();
        out_of_bounds.slots[3].x = 2;
        assert_eq!(
            out_of_bounds.validate(),
            Err(DeviceValidationError::SlotOutOfBounds {
                x: 2,
                y: 1,
                cols: 2,
                rows: 2,
            })
        );

        // Degenerate dimensions.
        let mut empty = grid_2x2();
        empty.cols = 0;
        assert_eq!(empty.validate(), Err(DeviceValidationError::EmptyGrid));
    }

    #[test]
    fn device_validation_requires_unit_grid_centroids() {
        let mut device = grid_2x2();
        device.slots[1].centroid_y = UNIT_DIST_Y;
        assert!(matches!(
            device.validate(),
            Err(DeviceValidationError::CentroidOffGrid { x: 1, y: 0, .. })
        ));
    }

    #[test]
    fn device_validation_requires_a_platform_for_bank_tags() {
        let mut device = grid_2x2();
        device.slots[0].tags.push("DDR[0]".to_string());
        assert_eq!(
            device.validate(),
            Err(DeviceValidationError::BankTagWithoutPlatform {
                x: 0,
                y: 0,
                tag: "DDR[0]".to_string(),
            })
        );

        device.platform_name = Some("shell".to_string());
        device
            .validate()
            .expect("a recorded platform satisfies the rule");

        // Non-bank tags never require a platform.
        let mut device = grid_2x2();
        device.slots[0].tags.push("CLK_RST".to_string());
        device.slots[0].tags.push("HBM".to_string());
        device
            .validate()
            .expect("SLR markers and control tags are not banks");
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
