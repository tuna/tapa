//! Selecting a [`Device`] table from a part number.
//!
//! The per-part tables are checked-in JSON, embedded with `include_str!` so
//! selection never depends on the source tree being present at runtime. Adding
//! a device is a new `*.json` plus one [`TABLES`] entry.

use crate::device::model::{Device, DeviceValidationError};

/// Every embedded device table, as `(key, json)`.
///
/// Each table's per-slot area is precollected, so selection never shells out to
/// Vivado. u50 is the one characterized device still missing; it needs the same
/// per-clock-region census of its platform's `pblock_dynamic_region` that the
/// others were built from.
const TABLES: &[(&str, &str)] = &[
    ("u250", include_str!("tables/u250.json")),
    ("u280", include_str!("tables/u280.json")),
    ("u55c", include_str!("tables/u55c.json")),
    ("vck190", include_str!("tables/vck190.json")),
];

/// Why [`select_device`] could not return a device.
#[derive(Debug, thiserror::Error)]
pub enum SelectError {
    /// No table's key, part number, or part family matched the query.
    #[error("no floorplan device table matches `{query}` (known: {known})")]
    UnknownPart {
        /// The part number or alias the caller asked for.
        query: String,
        /// The comma-separated keys that are available.
        known: String,
    },
    /// An embedded table failed to parse — a bug in the checked-in JSON.
    #[error("embedded device table `{key}` is malformed: {source}")]
    MalformedTable {
        /// The offending table's key.
        key: String,
        /// The underlying serde error.
        source: serde_json::Error,
    },
    /// An embedded table parsed but violated a device-model invariant.
    #[error("embedded device table `{key}` is invalid: {source}")]
    InvalidTable {
        /// The offending table's key.
        key: String,
        /// The semantic validation error.
        source: DeviceValidationError,
    },
}

/// A device source: resolves part numbers against a set of device tables.
///
/// The registry is the single seam between device *selection* and device
/// *storage*: today [`DeviceRegistry::embedded`] wraps the compiled-in
/// [`TABLES`], and future external device files extend the set of tables a
/// registry value carries rather than patching the selection path.
pub(crate) struct DeviceRegistry {
    tables: &'static [(&'static str, &'static str)],
}

impl DeviceRegistry {
    /// The registry over the compiled-in per-part tables.
    pub(crate) const fn embedded() -> Self {
        Self { tables: TABLES }
    }

    /// The keys of every table in this registry.
    fn keys(&self) -> Vec<&'static str> {
        self.tables.iter().map(|(key, _)| *key).collect()
    }

    /// Parse every table in this registry, surfacing a malformed one as an
    /// error.
    fn all_devices(&self) -> Result<Vec<Device>, SelectError> {
        self.tables
            .iter()
            .map(|(key, json)| {
                let device = serde_json::from_str::<Device>(json).map_err(|source| {
                    SelectError::MalformedTable {
                        key: (*key).to_string(),
                        source,
                    }
                })?;
                device
                    .validate()
                    .map_err(|source| SelectError::InvalidTable {
                        key: (*key).to_string(),
                        source,
                    })?;
                Ok(device)
            })
            .collect()
    }

    /// Resolve a part number to its device table; see [`select_device`]
    /// for the matching rules.
    pub(crate) fn select(&self, part_num: &str) -> Result<Device, SelectError> {
        let query = part_num.trim().to_ascii_lowercase();
        let query_family = part_family(&query);

        for device in self.all_devices()? {
            let key = device.key.to_ascii_lowercase();
            let part = device.part_num.to_ascii_lowercase();
            if query == key || query == part || query_family == part_family(&part) {
                return Ok(device);
            }
        }

        Err(SelectError::UnknownPart {
            query: part_num.to_string(),
            known: self.keys().join(", "),
        })
    }
}

/// The leading token of a part string, e.g. `xcu280` from
/// `xcu280-fsvh2892-2L-e`. This is the "part family" aliases match on.
fn part_family(part: &str) -> &str {
    part.split('-').next().unwrap_or(part)
}

/// Resolve a part number (or a short alias like `u280` or `xcu280-…`) to its
/// device table.
///
/// Matching is case-insensitive and accepts the table key (`u280`), the full
/// part number (`xcu280-fsvh2892-2L-e`), or any part string in the same family
/// (`xcu280-…`).
pub fn select_device(part_num: &str) -> Result<Device, SelectError> {
    DeviceRegistry::embedded().select(part_num)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_table_parses_and_covers_its_grid() {
        for device in DeviceRegistry::embedded()
            .all_devices()
            .expect("all tables must parse")
        {
            assert_eq!(
                u32::try_from(device.slots.len()).expect("slot count fits u32"),
                device.rows * device.cols,
                "{}: one slot per grid cell",
                device.key,
            );
            // Exactly the full grid, each cell once.
            for y in 0..device.rows {
                for x in 0..device.cols {
                    assert!(
                        device.slot(x, y).is_some(),
                        "{}: missing slot ({x},{y})",
                        device.key,
                    );
                }
            }
        }
    }

    #[test]
    fn centroids_follow_the_unit_grid() {
        use crate::device::model::{UNIT_DIST_X, UNIT_DIST_Y};
        for device in DeviceRegistry::embedded().all_devices().expect("parse") {
            for slot in &device.slots {
                assert_eq!(
                    (slot.centroid_x, slot.centroid_y),
                    (
                        UNIT_DIST_X * i64::from(slot.x),
                        UNIT_DIST_Y * i64::from(slot.y)
                    ),
                    "{}: slot ({},{}) centroid off the unit grid",
                    device.key,
                    slot.x,
                    slot.y,
                );
            }
        }
    }

    #[test]
    fn u280_reference_slot_matches_upstream() {
        let device = select_device("u280").expect("u280 resolves");
        assert!(!device.is_versal);
        assert_eq!((device.cols, device.rows), (2, 3));
        let slot = device.slot(0, 0).expect("slot (0,0)");
        assert_eq!(slot.area.lut, 220_800);
        assert_eq!(slot.area.ff, 441_600);
        assert_eq!(slot.area.bram_18k, 768);
        assert_eq!(slot.area.dsp, 1440);
        assert_eq!(slot.area.uram, 128);
        assert_eq!(slot.wire_cap.north, 11_520);
        assert_eq!(slot.wire_cap.east, 40_320);
    }

    #[test]
    fn u280_external_interfaces_have_exact_unique_slots() {
        let device = select_device("u280").expect("u280 resolves");
        assert_eq!(
            device.platform_name.as_deref(),
            Some("xilinx_u280_gen3x16_xdma_1_202211_1")
        );
        assert_eq!(
            device.user_pblock_name.as_deref(),
            Some("pblock_dynamic_region")
        );
        for index in 0..32 {
            let tag = format!("HBM[{index}]");
            let slots = device.slots_with_tag(&tag).collect::<Vec<_>>();
            assert_eq!(slots.len(), 1, "{tag} must identify exactly one slot");
            assert_eq!(slots[0].y, 0, "{tag} belongs in the memory-facing row");
            assert_eq!(slots[0].x, u32::from(index >= 16));
        }
        for (tag, expected) in [
            ("DDR[0]", (1, 0)),
            ("DDR[1]", (1, 1)),
            ("CLK_RST", (1, 0)),
            ("S_AXI_CONTROL", (1, 1)),
        ] {
            let slots = device.slots_with_tag(tag).collect::<Vec<_>>();
            assert_eq!(slots.len(), 1, "{tag} must identify exactly one slot");
            assert_eq!((slots[0].x, slots[0].y), expected);
        }
    }

    /// `xcu55c` shares `xcu280`'s die and package, so the grid is the same
    /// 3 SLRs × 2 columns over CR X0-X7 / Y0-Y11, and the 32
    /// `BLI_HBM_AXI_INTF` sites all sit in CR row Y0 — X0-X15 under CR
    /// columns X0-X3 and X16-X31 under X4-X7, which is what splits the banks
    /// across the bottom row's two slots.
    ///
    /// The shell is not the same. The U55C platform hands the ULP one
    /// `BLP_S_AXI_CTRL_USER_*` interface per SLR and every one of them is
    /// placed in the right-hand column, so the control anchor is a choice of
    /// row rather than of column; SLR1 is taken because it is equidistant from
    /// the other two.
    #[test]
    fn u55c_external_interfaces_have_exact_unique_slots() {
        let device = select_device("u55c").expect("u55c resolves");
        assert_eq!(device.part_num, "xcu55c-fsvh2892-2L-e");
        assert_eq!(
            device.platform_name.as_deref(),
            Some("xilinx_u55c_gen3x16_xdma_3_202210_1")
        );
        assert_eq!(
            device.user_pblock_name.as_deref(),
            Some("pblock_dynamic_region")
        );
        assert!(!device.is_versal);
        for index in 0..32 {
            let tag = format!("HBM[{index}]");
            let slots = device.slots_with_tag(&tag).collect::<Vec<_>>();
            assert_eq!(slots.len(), 1, "{tag} must identify exactly one slot");
            assert_eq!(slots[0].y, 0, "{tag} belongs in the memory-facing row");
            assert_eq!(slots[0].x, u32::from(index >= 16));
        }
        for tag in ["CLK_RST", "S_AXI_CONTROL"] {
            let slots = device.slots_with_tag(tag).collect::<Vec<_>>();
            assert_eq!(slots.len(), 1, "{tag} must identify exactly one slot");
            assert_eq!((slots[0].x, slots[0].y), (1, 1));
        }
        // The U55C carries no DDR, so a `sp=...:DDR[0]` binding must fail to
        // resolve rather than land somewhere plausible.
        assert_eq!(device.slots_with_tag("DDR[0]").count(), 0);
    }

    /// Per-slot areas are a census of the sites the platform's
    /// `pblock_dynamic_region` leaves in each slot, measured on the shell
    /// checkpoint shipped in the platform's `hw.xsa`. Two properties make the
    /// numbers checkable: the shell occupies only the right-hand column, so
    /// every left-column slot must equal the bare device's fabric for that
    /// quarter; and the six slots must tile the dynamic region without
    /// overlapping.
    #[test]
    fn u55c_slots_carry_the_platform_dynamic_region() {
        let device = select_device("u55c").expect("u55c resolves");
        for (x, y, lut, dsp, bram_18k, uram) in [
            // Left column: untouched by the shell, equal to the raw device.
            (0, 0, 220_800, 1440, 768, 128),
            (0, 1, 216_960, 1536, 768, 128),
            (0, 2, 216_960, 1536, 768, 128),
            // Right column: what the static region leaves behind.
            (1, 0, 168_000, 1224, 432, 192),
            (1, 1, 147_840, 1248, 384, 192),
            (1, 2, 178_080, 1392, 432, 192),
        ] {
            let slot = device.slot(x, y).expect("slot");
            assert_eq!(slot.area.lut, lut, "({x},{y}) lut");
            assert_eq!(slot.area.ff, lut * 2, "({x},{y}) ff");
            assert_eq!(slot.area.dsp, dsp, "({x},{y}) dsp");
            assert_eq!(slot.area.bram_18k, bram_18k, "({x},{y}) bram");
            assert_eq!(slot.area.uram, uram, "({x},{y}) uram");
            assert!(!slot.pblock_ranges.is_empty(), "({x},{y}) ranges");
        }
        // SLL budget is 6 registers per LAGUNA site, and the shell takes a
        // quarter of the right column's, so the two columns cross at
        // different capacities.
        assert_eq!(device.slot(0, 0).expect("slot").wire_cap.north, 11_520);
        assert_eq!(device.slot(1, 0).expect("slot").wire_cap.north, 8_640);
        // Every column is cut at the same clock-region boundary in every row,
        // so a vertical hop always joins slots that physically overlap — the
        // invariant `border_capacity` and the SLL pipelining both rest on.
        for slot in &device.slots {
            let (lo, hi) = if slot.x == 0 { (0, 3) } else { (4, 7) };
            for range in &slot.pblock_ranges {
                for column in range.match_indices("CLOCKREGION_X").filter_map(|(at, _)| {
                    range[at + "CLOCKREGION_X".len()..]
                        .split('Y')
                        .next()
                        .and_then(|digits| digits.parse::<u32>().ok())
                }) {
                    assert!(
                        (lo..=hi).contains(&column),
                        "slot ({},{}) reaches CR column X{column}, outside X{lo}-X{hi}: {range}",
                        slot.x,
                        slot.y,
                    );
                }
            }
        }
    }

    /// Every HP I/O bank on `xcu250` lives in clock-region column X4 (banks
    /// 61-74), so every DDR4 controller is in the right-hand slot column, and
    /// `platforminfo` puts DDR segment n on SLR n. The shell — where the
    /// PCIe/XDMA control interface enters — occupies SLR1's right half.
    #[test]
    fn u250_external_interfaces_have_exact_unique_slots() {
        let device = select_device("u250").expect("u250 resolves");
        assert_eq!(
            device.platform_name.as_deref(),
            Some("xilinx_u250_gen3x16_xdma_4_1_202210_1")
        );
        assert_eq!(
            device.user_pblock_name.as_deref(),
            Some("pblock_dynamic_region")
        );
        for (tag, expected) in [
            ("DDR[0]", (1, 0)),
            // SLR1's right half is entirely shell, so the interfaces that
            // enter there anchor to the only slot in SLR1 that has fabric.
            ("DDR[1]", (0, 1)),
            ("DDR[2]", (1, 2)),
            ("DDR[3]", (1, 3)),
            ("CLK_RST", (0, 1)),
            ("S_AXI_CONTROL", (0, 1)),
        ] {
            let slots = device.slots_with_tag(tag).collect::<Vec<_>>();
            assert_eq!(slots.len(), 1, "{tag} must identify exactly one slot");
            assert_eq!((slots[0].x, slots[0].y), expected, "{tag}");
        }
    }

    /// Every SLR is cut at the same column boundary, and each slot's capacity
    /// is the fabric the platform's `pblock_dynamic_region` actually leaves
    /// it, measured per clock region. The four slots the shell does not touch
    /// come out exactly equal to the device-level numbers, which is the
    /// cross-check that the measurement is right.
    ///
    /// SLR1 is the interesting row: the shell owns all four of its right-hand
    /// columns, so slot (1,1) is empty and the interfaces that enter through
    /// it anchor to (0,1) instead. Re-cutting that row to balance it
    /// (`X0-X1` / `X2-X3`, 1.8% apart against 4.7% for the aligned cut) was
    /// tried and rejected: a row-specific cut would put logical column 1 over
    /// physical `X4-X7` in SLR0 and `X2-X3` in SLR1, asserting an SLL border
    /// between pblocks that share no physical column.
    #[test]
    fn u250_slots_carry_the_platform_dynamic_region() {
        let device = select_device("u250").expect("u250 resolves");
        for (x, y, pblock, lut) in [
            (0, 0, "CLOCKREGION_X0Y0:CLOCKREGION_X3Y3", 216_960),
            (1, 0, "CLOCKREGION_X4Y0:CLOCKREGION_X7Y3", 206_880),
            (0, 1, "CLOCKREGION_X0Y4:CLOCKREGION_X3Y7", 213_120),
            // The shell: no user fabric at all, hence no tag either.
            (1, 1, "CLOCKREGION_X4Y4:CLOCKREGION_X7Y7", 0),
            (0, 2, "CLOCKREGION_X0Y8:CLOCKREGION_X3Y11", 216_960),
            (1, 2, "CLOCKREGION_X4Y8:CLOCKREGION_X7Y11", 202_200),
            (0, 3, "CLOCKREGION_X0Y12:CLOCKREGION_X3Y15", 216_960),
            (1, 3, "CLOCKREGION_X4Y12:CLOCKREGION_X7Y15", 215_040),
        ] {
            let slot = device.slot(x, y).expect("slot");
            assert_eq!(slot.pblock_ranges, vec![pblock.to_string()], "({x},{y})");
            assert_eq!(slot.area.lut, lut, "({x},{y})");
            assert_eq!(slot.area.ff, lut * 2, "({x},{y})");
        }
        // Every column is cut at the same place in every row, so a vertical
        // hop always joins slots that physically overlap. `border_capacity`,
        // the pipeline planner's SLR-hop register budget, and the XDC's
        // `USER_SLL_REG` guidance all depend on that.
        for slot in &device.slots {
            let columns: Vec<&str> = slot.pblock_ranges[0]
                .split(':')
                .map(|end| end.split('Y').next().expect("CLOCKREGION_X<n>Y<m>"))
                .collect();
            let expected: [&str; 2] = if slot.x == 0 {
                ["CLOCKREGION_X0", "CLOCKREGION_X3"]
            } else {
                ["CLOCKREGION_X4", "CLOCKREGION_X7"]
            };
            assert_eq!(
                columns.as_slice(),
                expected.as_slice(),
                "slot ({},{}) breaks column alignment",
                slot.x,
                slot.y,
            );
        }
    }

    /// VC1902's four NoC memory controllers sit in the bottom row at
    /// clock regions X0Y0, X3Y0, X6Y0 and X10Y0. The table's memory-facing
    /// row is y=0, whose left slot spans CR columns X0-X4 and whose right
    /// slot spans X5-X9, so the first two controllers face the left slot.
    #[test]
    fn vck190_external_interfaces_have_exact_unique_slots() {
        let device = select_device("vck190").expect("vck190 resolves");
        assert!(device.is_versal);
        assert_eq!(
            device.platform_name.as_deref(),
            Some("xilinx_vck190_base_202410_1")
        );
        for (tag, expected) in [
            ("DDR[0]", (0, 0)),
            ("DDR[1]", (0, 0)),
            ("DDR[2]", (1, 0)),
            ("DDR[3]", (1, 0)),
            ("CLK_RST", (0, 0)),
            ("S_AXI_CONTROL", (0, 0)),
        ] {
            let slots = device.slots_with_tag(tag).collect::<Vec<_>>();
            assert_eq!(slots.len(), 1, "{tag} must identify exactly one slot");
            assert_eq!((slots[0].x, slots[0].y), expected, "{tag}");
        }
    }

    /// Anchoring only works when a tag names one slot, so no table may
    /// repeat an exact bank or interface tag. A copy-paste slip in a
    /// hand-authored table would otherwise surface as a confusing plan-time
    /// error. Family tags like the bare `HBM` are deliberately shared.
    #[test]
    fn no_device_table_repeats_an_exact_tag() {
        for (key, _) in TABLES {
            let device = select_device(key).expect("known device resolves");
            let mut seen = std::collections::BTreeSet::new();
            for slot in &device.slots {
                for tag in &slot.tags {
                    if !tag.ends_with(']') && !matches!(tag.as_str(), "CLK_RST" | "S_AXI_CONTROL") {
                        continue;
                    }
                    assert!(
                        seen.insert(tag.clone()),
                        "{key}: tag {tag} appears on more than one slot"
                    );
                }
            }
        }
    }

    #[test]
    fn u280_platform_slot_capacities_are_precollected() {
        let device = select_device("u280").expect("u280 resolves");
        let expected = [
            ((0, 0), (220_800, 441_600, 768, 1_440, 128)),
            ((0, 1), (220_800, 441_600, 768, 1_536, 128)),
            ((0, 2), (220_800, 441_600, 768, 1_536, 128)),
            ((1, 0), (164_160, 328_320, 432, 1_224, 192)),
            ((1, 1), (142_080, 284_160, 384, 1_248, 192)),
            ((1, 2), (161_760, 323_520, 432, 1_320, 192)),
        ];
        for ((x, y), (lut, ff, bram_18k, dsp, uram)) in expected {
            let slot = device.slot(x, y).expect("precollected slot");
            assert_eq!(
                (
                    slot.area.lut,
                    slot.area.ff,
                    slot.area.bram_18k,
                    slot.area.dsp,
                    slot.area.uram,
                ),
                (lut, ff, bram_18k, dsp, uram),
                "slot ({x},{y})",
            );
        }

        assert_eq!(device.slot(0, 0).expect("slot").wire_cap.north, 11_520);
        assert_eq!(device.slot(0, 1).expect("slot").wire_cap.south, 11_520);
        assert_eq!(device.slot(0, 1).expect("slot").wire_cap.north, 11_520);
        assert_eq!(device.slot(0, 2).expect("slot").wire_cap.south, 11_520);
        assert_eq!(device.slot(1, 0).expect("slot").wire_cap.north, 5_277);
        assert_eq!(device.slot(1, 1).expect("slot").wire_cap.south, 5_277);
        assert_eq!(device.slot(1, 1).expect("slot").wire_cap.north, 7_253);
        assert_eq!(device.slot(1, 2).expect("slot").wire_cap.south, 7_253);
    }

    #[test]
    fn u280_platform_pblock_operations_are_exact() {
        let device = select_device("u280").expect("u280 resolves");
        let row_ranges: [&[&str]; 3] = [
            &[
                "-add {SLICE_X206Y0:SLICE_X232Y59 SLICE_X176Y60:SLICE_X196Y239 SLICE_X117Y180:SLICE_X145Y239}",
                "-add {DSP48E2_X25Y18:DSP48E2_X28Y89 DSP48E2_X16Y66:DSP48E2_X19Y89 DSP48E2_X30Y0:DSP48E2_X31Y17}",
                "-add {LAGUNA_X24Y0:LAGUNA_X27Y119 LAGUNA_X16Y0:LAGUNA_X19Y119}",
                "-add {RAMB18_X11Y24:RAMB18_X11Y95 RAMB18_X8Y72:RAMB18_X9Y95 RAMB18_X12Y0:RAMB18_X13Y23}",
                "-add {RAMB36_X11Y12:RAMB36_X11Y47 RAMB36_X8Y36:RAMB36_X9Y47 RAMB36_X12Y0:RAMB36_X13Y11}",
                "-add {URAM288_X4Y16:URAM288_X4Y63 URAM288_X2Y48:URAM288_X2Y63}",
                "-add {CLOCKREGION_X5Y3:CLOCKREGION_X5Y3 CLOCKREGION_X0Y3:CLOCKREGION_X3Y3 CLOCKREGION_X0Y1:CLOCKREGION_X5Y2 CLOCKREGION_X0Y0:CLOCKREGION_X6Y0}",
            ],
            &[
                "-add {SLICE_X176Y240:SLICE_X196Y479}",
                "-add {DSP48E2_X25Y90:DSP48E2_X28Y185}",
                "-add {LAGUNA_X24Y120:LAGUNA_X27Y359}",
                "-add {RAMB18_X11Y96:RAMB18_X11Y191}",
                "-add {RAMB36_X11Y48:RAMB36_X11Y95}",
                "-add {URAM288_X4Y64:URAM288_X4Y127}",
                "-add {CLOCKREGION_X0Y4:CLOCKREGION_X5Y7}",
            ],
            &[
                "-add {SLICE_X117Y660:SLICE_X145Y719 SLICE_X176Y480:SLICE_X196Y659 SLICE_X220Y540:SLICE_X221Y599}",
                "-add {DSP48E2_X16Y258:DSP48E2_X19Y281 DSP48E2_X25Y186:DSP48E2_X28Y257}",
                "-add {LAGUNA_X16Y480:LAGUNA_X19Y599 LAGUNA_X24Y360:LAGUNA_X27Y479}",
                "-add {RAMB18_X8Y264:RAMB18_X9Y287 RAMB18_X11Y192:RAMB18_X11Y263}",
                "-add {RAMB36_X8Y132:RAMB36_X9Y143 RAMB36_X11Y96:RAMB36_X11Y131}",
                "-add {URAM288_X2Y176:URAM288_X2Y191 URAM288_X4Y128:URAM288_X4Y175}",
                "-add {CLOCKREGION_X5Y11:CLOCKREGION_X7Y11 CLOCKREGION_X0Y11:CLOCKREGION_X3Y11 CLOCKREGION_X0Y8:CLOCKREGION_X5Y10}",
                "-add {CONFIG_SITE_X0Y2:CONFIG_SITE_X0Y2}",
            ],
        ];
        let half_ranges = [
            [
                "CLOCKREGION_X4Y0:CLOCKREGION_X7Y3",
                "CLOCKREGION_X4Y4:CLOCKREGION_X7Y7",
                "CLOCKREGION_X4Y8:CLOCKREGION_X7Y11",
            ],
            [
                "CLOCKREGION_X0Y0:CLOCKREGION_X3Y3",
                "CLOCKREGION_X0Y4:CLOCKREGION_X3Y7",
                "CLOCKREGION_X0Y8:CLOCKREGION_X3Y11",
            ],
        ];

        for x in 0..2 {
            for y in 0..3 {
                let slot = device.slot(x, y).expect("platform slot");
                let mut expected = row_ranges[y as usize]
                    .iter()
                    .map(|range| (*range).to_string())
                    .collect::<Vec<_>>();
                expected.push(format!("-remove {}", half_ranges[x as usize][y as usize]));
                assert_eq!(slot.pblock_ranges, expected, "slot ({x},{y})");
            }
        }
    }

    #[test]
    fn selection_accepts_key_full_part_and_family() {
        for query in [
            "u280",
            "xcu280-fsvh2892-2L-e",
            "xcu280-anything",
            "XCU280-FOO",
        ] {
            assert_eq!(
                select_device(query).expect("resolves").key,
                "u280",
                "`{query}` must resolve to u280",
            );
        }
        assert_eq!(select_device("vck190").expect("resolves").key, "vck190");
        assert_eq!(
            select_device("xcvc1902-vsva2197-2MP-e-S")
                .expect("resolves")
                .key,
            "vck190",
        );
    }

    #[test]
    fn unknown_part_is_a_typed_error() {
        let err = select_device("xcvu9p-nonsense").expect_err("no such device");
        assert!(matches!(err, SelectError::UnknownPart { .. }), "got {err}");
        assert!(
            err.to_string().contains("u280"),
            "error lists known devices",
        );
    }
}
