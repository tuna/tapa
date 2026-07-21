//! Selecting a [`Device`] table from a part number.
//!
//! The per-part tables are checked-in JSON, embedded with `include_str!` so
//! selection never depends on the source tree being present at runtime. Adding
//! a device is a new `*.json` plus one [`TABLES`] entry.

use crate::device::model::Device;

/// Every embedded device table, as `(key, json)`.
///
/// Only the three devices with Vivado-free precollected resource tables are
/// present (u250, u280, vck190); u50/u55c need per-slot areas extracted from
/// Vivado and are added once characterized.
const TABLES: &[(&str, &str)] = &[
    ("u250", include_str!("tables/u250.json")),
    ("u280", include_str!("tables/u280.json")),
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
}

/// The keys of every embedded device table.
#[must_use]
pub fn device_keys() -> Vec<&'static str> {
    TABLES.iter().map(|(key, _)| *key).collect()
}

/// Parse every embedded table, surfacing a malformed one as an error.
fn all_devices() -> Result<Vec<Device>, SelectError> {
    TABLES
        .iter()
        .map(|(key, json)| {
            serde_json::from_str::<Device>(json).map_err(|source| SelectError::MalformedTable {
                key: (*key).to_string(),
                source,
            })
        })
        .collect()
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
    let query = part_num.trim().to_ascii_lowercase();
    let query_family = part_family(&query);

    for device in all_devices()? {
        let key = device.key.to_ascii_lowercase();
        let part = device.part_num.to_ascii_lowercase();
        if query == key || query == part || query_family == part_family(&part) {
            return Ok(device);
        }
    }

    Err(SelectError::UnknownPart {
        query: part_num.to_string(),
        known: device_keys().join(", "),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_table_parses_and_covers_its_grid() {
        for device in all_devices().expect("all tables must parse") {
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
        for device in all_devices().expect("parse") {
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
