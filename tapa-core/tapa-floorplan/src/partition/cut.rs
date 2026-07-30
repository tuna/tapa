//! Guillotine cuts over a partition iteration's candidate regions.
//!
//! Cuts are formed over the regions used by the *current* partition iteration,
//! rather than always over atomic slots. That distinction lets the same
//! placement ILP be reused for the row-level and column-level passes of
//! multilevel placement.

use crate::device::model::{effective_border_capacity, Coor, Device, WIRE_CAPACITY_INF};

/// A clean bipartition of the current candidate regions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cut {
    /// Cut label, e.g. `y=0` (row boundary) or `x=1` (column boundary).
    pub name: String,
    /// Regions on the down/left side.
    pub lhs: Vec<Coor>,
    /// Regions on the up/right side.
    pub rhs: Vec<Coor>,
    /// Allowed crossing width: the sum of the per-cell-pair
    /// effective border capacities along the shared border.
    pub capacity: u64,
}

/// Enumerate every straight guillotine cut that does not split any region.
///
/// The capacity of each neighboring region pair is computed over only their
/// *shared* border interval, with each cell pair contributing
/// [`effective_border_capacity`] of its two facing declarations — the same
/// per-boundary budget the routing MILP enforces. Cuts whose capacity is at
/// the unconstrained sentinel scale are omitted because the ILP would skip
/// them.
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

    let capacity: u64 = lhs
        .iter()
        .flat_map(|left| rhs.iter().map(move |right| (left, right)))
        .filter(|(left, right)| left.is_neighbor(right))
        .map(|(left, right)| border_capacity(device, left, right))
        .sum();
    if capacity < WIRE_CAPACITY_INF / 2 {
        cuts.push(Cut {
            name,
            lhs,
            rhs,
            capacity,
        });
    }
}

/// Sum the per-cell-pair effective capacities along the *shared* border
/// interval of two neighboring regions. Cells without a facing partner
/// (partially overlapping borders) contribute nothing to this pair's border,
/// so the cut never credits capacity the crossing wires cannot physically use.
fn border_capacity(device: &Device, lhs: &Coor, rhs: &Coor) -> u64 {
    let shared_x = lhs.dl_x.max(rhs.dl_x)..=lhs.ur_x.min(rhs.ur_x);
    let shared_y = lhs.dl_y.max(rhs.dl_y)..=lhs.ur_y.min(rhs.ur_y);
    if lhs.is_south_neighbor_of(rhs) {
        return shared_x
            .filter_map(|x| device.slot(x, lhs.ur_y).zip(device.slot(x, rhs.dl_y)))
            .map(|(a, b)| effective_border_capacity(a.wire_cap.north, b.wire_cap.south))
            .sum();
    }
    if lhs.is_north_neighbor_of(rhs) {
        return shared_x
            .filter_map(|x| device.slot(x, lhs.dl_y).zip(device.slot(x, rhs.ur_y)))
            .map(|(a, b)| effective_border_capacity(a.wire_cap.south, b.wire_cap.north))
            .sum();
    }
    if lhs.is_west_neighbor_of(rhs) {
        return shared_y
            .filter_map(|y| device.slot(lhs.ur_x, y).zip(device.slot(rhs.dl_x, y)))
            .map(|(a, b)| effective_border_capacity(a.wire_cap.east, b.wire_cap.west))
            .sum();
    }
    if lhs.is_east_neighbor_of(rhs) {
        return shared_y
            .filter_map(|y| device.slot(lhs.dl_x, y).zip(device.slot(rhs.ur_x, y)))
            .map(|(a, b)| effective_border_capacity(a.wire_cap.west, b.wire_cap.east))
            .sum();
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::model::{DirCaps, Slot};
    use crate::device::select::select_device;
    use tapa_ir::Area;

    fn find_all_slot_cuts(device: &Device) -> Vec<Cut> {
        let regions: Vec<Coor> = device
            .slots
            .iter()
            .map(crate::device::model::Slot::coor)
            .collect();
        find_cuts_for_regions(device, &regions)
    }

    #[test]
    fn u280_has_two_row_cuts_and_one_column_cut() {
        let device = select_device("u280").expect("u280");
        let cuts = find_all_slot_cuts(&device);
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
    fn region_border_uses_the_per_cell_pair_minimum() {
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
            // Each physical cell-pair boundary is governed by the *smaller* of
            // its two facing declarations: min(100, 1) + min(1, 100), derated.
            // Summing each region's full side first (min(101, 101) = 101)
            // would credit capacity that neither boundary can support —
            // routing models the same per-pair minimum.
            slots: vec![
                mk_slot(0, 0, 100, 0),
                mk_slot(1, 0, 1, 0),
                mk_slot(0, 1, 0, 1),
                mk_slot(1, 1, 0, 100),
            ],
        };
        let rows = [Coor::span(0, 0, 1, 0), Coor::span(0, 1, 1, 1)];
        let cuts = find_cuts_for_regions(&device, &rows);
        assert_eq!(cuts[0].capacity, 2, "round(1 * 0.7) + round(1 * 0.7)");
    }

    #[test]
    fn partially_aligned_borders_count_only_the_shared_interval() {
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
            tags: Vec::new(),
        };
        let device = Device {
            key: "toy".to_string(),
            part_num: "toy".to_string(),
            platform_name: None,
            rows: 2,
            cols: 3,
            pp_dist: 1,
            is_versal: false,
            user_pblock_name: None,
            slots: vec![
                mk_slot(0, 0, 10, 0),
                mk_slot(1, 0, 10, 0),
                mk_slot(2, 0, 10, 0),
                mk_slot(0, 1, 0, 10),
                mk_slot(1, 1, 0, 10),
                mk_slot(2, 1, 0, 10),
            ],
        };
        // The narrow region below shares only x=1 with the wide region above;
        // its other two slots border vertical cuts, not this pair.
        let narrow = Coor::span(1, 0, 1, 0);
        let wide = Coor::span(0, 1, 2, 1);
        assert_eq!(
            border_capacity(&device, &narrow, &wide),
            7,
            "only the aligned slot pair contributes: round(10 * 0.7)",
        );
    }

    #[test]
    fn u250_drops_the_uncapped_column_cut() {
        let device = select_device("u250").expect("u250");
        let names: Vec<String> = find_all_slot_cuts(&device)
            .into_iter()
            .map(|c| c.name)
            .collect();
        assert_eq!(names, ["y=0", "y=1", "y=2"]);
    }

    #[test]
    fn vck190_has_no_binding_cuts() {
        let device = select_device("vck190").expect("vck190");
        assert!(find_all_slot_cuts(&device).is_empty());
    }
}
