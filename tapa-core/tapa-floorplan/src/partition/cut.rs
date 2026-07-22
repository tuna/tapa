//! Guillotine cuts over a partition iteration's candidate regions.
//!
//! Cuts are formed over the regions used by the *current* partition iteration,
//! rather than always over atomic slots. That distinction lets the same
//! placement ILP be reused for the row-level and column-level passes of
//! multilevel placement.

use crate::device::model::{Coor, Device, USABLE_WIRE_RATIO, WIRE_CAPACITY_INF};

/// A clean bipartition of the current candidate regions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cut {
    /// Cut label, e.g. `y=0` (row boundary) or `x=1` (column boundary).
    pub name: String,
    /// Regions on the down/left side.
    pub lhs: Vec<Coor>,
    /// Regions on the up/right side.
    pub rhs: Vec<Coor>,
    /// Allowed crossing width: Python `round(0.7 * raw_capacity)`.
    pub capacity: u64,
}

/// Apply the default usable-wire ratio with Python-compatible ties-to-even
/// rounding. The multiplication intentionally happens in binary64 first:
/// Python evaluates `45 * 0.7` just below 31.5 and therefore rounds it to 31.
/// A rational `7/10` implementation would not be equivalent.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    reason = "the formulation computes round(integer_capacity * 0.7) in binary64"
)]
fn apply_ratio(raw: u64) -> u64 {
    debug_assert!(
        (USABLE_WIRE_RATIO - 0.7).abs() < f64::EPSILON,
        "ratio drifted"
    );

    (raw as f64 * USABLE_WIRE_RATIO).round_ties_even() as u64
}

/// Enumerate the binding cuts for all atomic slots of `device`.
///
/// This compatibility entry point is used by callers that request flat
/// placement.  Multilevel placement calls [`find_cuts_for_regions`] directly.
#[must_use]
pub fn find_cuts(device: &Device) -> Vec<Cut> {
    let regions: Vec<Coor> = device
        .slots
        .iter()
        .map(crate::device::model::Slot::coor)
        .collect();
    find_cuts_for_regions(device, &regions)
}

/// Enumerate every straight guillotine cut that does not split any region.
///
/// The capacity is the sum of the complete facing-border capacities between
/// every neighboring region pair on opposing sides, derated by 0.7 and
/// rounded to the nearest integer with ties to even. Placeholder/infinite cuts
/// are omitted because the ILP would skip them.
#[must_use]
pub fn find_cuts_for_regions(device: &Device, regions: &[Coor]) -> Vec<Cut> {
    if regions.is_empty() {
        return Vec::new();
    }

    let max_x = regions.iter().map(|region| region.ur_x).max().unwrap_or(0);
    let max_y = regions.iter().map(|region| region.ur_y).max().unwrap_or(0);
    let mut cuts = Vec::new();

    for y in 0..max_y {
        let lhs: Vec<Coor> = regions
            .iter()
            .copied()
            .filter(|region| region.ur_y <= y)
            .collect();
        let rhs: Vec<Coor> = regions
            .iter()
            .copied()
            .filter(|region| region.dl_y > y)
            .collect();
        push_if_clean_and_binding(device, regions, &mut cuts, format!("y={y}"), lhs, rhs);
    }

    for x in 0..max_x {
        let lhs: Vec<Coor> = regions
            .iter()
            .copied()
            .filter(|region| region.ur_x <= x)
            .collect();
        let rhs: Vec<Coor> = regions
            .iter()
            .copied()
            .filter(|region| region.dl_x > x)
            .collect();
        push_if_clean_and_binding(device, regions, &mut cuts, format!("x={x}"), lhs, rhs);
    }

    cuts
}

fn push_if_clean_and_binding(
    device: &Device,
    all_regions: &[Coor],
    cuts: &mut Vec<Cut>,
    name: String,
    lhs: Vec<Coor>,
    rhs: Vec<Coor>,
) {
    if lhs.len() + rhs.len() != all_regions.len() {
        return;
    }

    let raw = lhs
        .iter()
        .flat_map(|left| rhs.iter().map(move |right| (left, right)))
        .filter(|(left, right)| left.is_neighbor(right))
        .map(|(left, right)| border_capacity(device, left, right))
        .sum();
    let capacity = apply_ratio(raw);
    if capacity < WIRE_CAPACITY_INF / 2 {
        cuts.push(Cut {
            name,
            lhs,
            rhs,
            capacity,
        });
    }
}

/// Sum the complete facing border of each region, then take the smaller side's
/// capacity.
fn border_capacity(device: &Device, lhs: &Coor, rhs: &Coor) -> u64 {
    if lhs.is_south_neighbor_of(rhs) {
        return north_capacity(device, lhs).min(south_capacity(device, rhs));
    }
    if lhs.is_north_neighbor_of(rhs) {
        return south_capacity(device, lhs).min(north_capacity(device, rhs));
    }
    if lhs.is_west_neighbor_of(rhs) {
        return east_capacity(device, lhs).min(west_capacity(device, rhs));
    }
    if lhs.is_east_neighbor_of(rhs) {
        return west_capacity(device, lhs).min(east_capacity(device, rhs));
    }
    0
}

fn north_capacity(device: &Device, region: &Coor) -> u64 {
    (region.dl_x..=region.ur_x)
        .filter_map(|x| device.slot(x, region.ur_y))
        .map(|slot| slot.wire_cap.north)
        .sum()
}

fn south_capacity(device: &Device, region: &Coor) -> u64 {
    (region.dl_x..=region.ur_x)
        .filter_map(|x| device.slot(x, region.dl_y))
        .map(|slot| slot.wire_cap.south)
        .sum()
}

fn east_capacity(device: &Device, region: &Coor) -> u64 {
    (region.dl_y..=region.ur_y)
        .filter_map(|y| device.slot(region.ur_x, y))
        .map(|slot| slot.wire_cap.east)
        .sum()
}

fn west_capacity(device: &Device, region: &Coor) -> u64 {
    (region.dl_y..=region.ur_y)
        .filter_map(|y| device.slot(region.dl_x, y))
        .map(|slot| slot.wire_cap.west)
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::model::{DirCaps, DirRegions, Slot};
    use crate::device::select::select_device;
    use tapa_ir::Area;

    #[test]
    fn ratio_uses_python_ties_to_even() {
        assert_eq!(apply_ratio(5), 4, "3.5 rounds to the even integer 4");
        assert_eq!(apply_ratio(15), 10, "10.5 rounds to the even integer 10");
        assert_eq!(apply_ratio(25), 18, "17.5 rounds to the even integer 18");
        assert_eq!(
            apply_ratio(45),
            31,
            "binary64 45 * 0.7 is just below 31.5, exactly as in Python"
        );
    }

    #[test]
    fn u280_has_two_row_cuts_and_one_column_cut() {
        let device = select_device("u280").expect("u280");
        let cuts = find_cuts(&device);
        let names: Vec<&str> = cuts.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["y=0", "y=1", "x=0"], "u280 grid is 2x3");

        let row = cuts.iter().find(|c| c.name == "y=0").unwrap();
        assert_eq!(row.capacity, 11758);
        assert_eq!(row.lhs.len(), 2, "row 0");
        assert_eq!(row.rhs.len(), 4, "rows 1 and 2");

        let row = cuts.iter().find(|c| c.name == "y=1").unwrap();
        assert_eq!(row.capacity, 13141);

        let col = cuts.iter().find(|c| c.name == "x=0").unwrap();
        assert_eq!(col.capacity, 84672);
        assert_eq!(col.lhs.len(), 3, "column 0");
        assert_eq!(col.rhs.len(), 3, "column 1");
    }

    #[test]
    fn row_level_regions_have_only_horizontal_cuts() {
        let device = select_device("u280").expect("u280");
        let rows = vec![
            Coor::span(0, 0, 1, 0),
            Coor::span(0, 1, 1, 1),
            Coor::span(0, 2, 1, 2),
        ];
        let cuts = find_cuts_for_regions(&device, &rows);
        assert_eq!(
            cuts.iter().map(|cut| cut.name.as_str()).collect::<Vec<_>>(),
            ["y=0", "y=1"]
        );
        assert_eq!(cuts[0].capacity, 11758);
        assert_eq!(cuts[1].capacity, 13141);
    }

    #[test]
    fn region_border_is_summed_before_taking_the_minimum() {
        let mk_slot = |x, y, north, south| Slot {
            x,
            y,
            area: Area::default(),
            centroid_x: i64::from(x),
            centroid_y: i64::from(y),
            pblock_ranges: Vec::new(),
            wire_cap: DirCaps {
                north,
                south,
                east: WIRE_CAPACITY_INF,
                west: WIRE_CAPACITY_INF,
            },
            anchor: DirRegions::default(),
            tags: Vec::new(),
        };
        let device = Device {
            key: "toy".to_string(),
            part_num: "toy".to_string(),
            platform_name: None,
            rows: 2,
            cols: 2,
            pp_dist: 1,
            is_versal: false,
            user_pblock_name: None,
            // Pairwise min would be 1 + 1. Summing each complete border first
            // instead yields min(101, 101) = 101.
            slots: vec![
                mk_slot(0, 0, 100, 0),
                mk_slot(1, 0, 1, 0),
                mk_slot(0, 1, 0, 1),
                mk_slot(1, 1, 0, 100),
            ],
        };
        let rows = [Coor::span(0, 0, 1, 0), Coor::span(0, 1, 1, 1)];
        let cuts = find_cuts_for_regions(&device, &rows);
        assert_eq!(cuts[0].capacity, 71, "round(101 * 0.7)");
    }

    #[test]
    fn u250_drops_the_uncapped_column_cut() {
        let device = select_device("u250").expect("u250");
        let names: Vec<String> = find_cuts(&device).into_iter().map(|c| c.name).collect();
        assert_eq!(names, ["y=0", "y=1", "y=2"]);
    }

    #[test]
    fn vck190_has_no_binding_cuts() {
        let device = select_device("vck190").expect("vck190");
        assert!(find_cuts(&device).is_empty());
    }
}
