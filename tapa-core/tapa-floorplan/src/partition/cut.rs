//! Guillotine cuts over a partition iteration's candidate regions.
//!
//! Cuts are formed over the regions used by the *current* partition iteration,
//! rather than always over atomic slots. That distinction lets the same
//! placement ILP be reused for the row-level and column-level passes of
//! multilevel placement.

use tapa_ir::Area;

use crate::device::model::{bounded_border_capacity, Coor, Device, Slot};

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
/// [`bounded_border_capacity`] of its two facing declarations — the same
/// per-boundary budget the routing MILP enforces. A cut with even one
/// unconstrained cell pair cannot bind (wires would simply flow there), so it
/// is omitted rather than given a huge finite budget.
#[must_use]
pub fn find_cuts_for_regions(device: &Device, regions: &[Coor]) -> Vec<Cut> {
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

    let capacity: Option<u64> = lhs
        .iter()
        .flat_map(|left| rhs.iter().map(move |right| (left, right)))
        .filter(|(left, right)| left.is_neighbor(right))
        .map(|(left, right)| border_capacity(device, left, right))
        .sum();
    if let Some(capacity) = capacity {
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
///
/// A cell pair with a slot that has no fabric contributes nothing: a route
/// across it would end in that slot or, passing through, leave a Body
/// register there, and the router admits neither (its register budget for
/// such a slot is zero). Counting the pair would let placement plan
/// crossings no route can carry, e.g. through U250's shell-only `X1Y1`.
///
/// `None` when any cell pair on the shared border is unconstrained, or when
/// the bounded pairs sum past `u64` — either way the pair cannot bind.
fn border_capacity(device: &Device, lhs: &Coor, rhs: &Coor) -> Option<u64> {
    let shared_x = lhs.dl_x.max(rhs.dl_x)..=lhs.ur_x.min(rhs.ur_x);
    let shared_y = lhs.dl_y.max(rhs.dl_y)..=lhs.ur_y.min(rhs.ur_y);
    // Each arm names one cell pair and its two facing declarations: the side
    // `lhs` presents to `rhs`, then the side `rhs` presents back.
    let facing: Vec<(&Slot, &Slot, u64, u64)> = if lhs.is_south_neighbor_of(rhs) {
        shared_x
            .filter_map(|x| device.slot(x, lhs.ur_y).zip(device.slot(x, rhs.dl_y)))
            .map(|(near, far)| (near, far, near.wire_cap.north, far.wire_cap.south))
            .collect()
    } else if lhs.is_north_neighbor_of(rhs) {
        shared_x
            .filter_map(|x| device.slot(x, lhs.dl_y).zip(device.slot(x, rhs.ur_y)))
            .map(|(near, far)| (near, far, near.wire_cap.south, far.wire_cap.north))
            .collect()
    } else if lhs.is_west_neighbor_of(rhs) {
        shared_y
            .filter_map(|y| device.slot(lhs.ur_x, y).zip(device.slot(rhs.dl_x, y)))
            .map(|(near, far)| (near, far, near.wire_cap.east, far.wire_cap.west))
            .collect()
    } else {
        shared_y
            .filter_map(|y| device.slot(lhs.dl_x, y).zip(device.slot(rhs.ur_x, y)))
            .map(|(near, far)| (near, far, near.wire_cap.west, far.wire_cap.east))
            .collect()
    };

    facing
        .into_iter()
        .try_fold(0_u64, |total, (near, far, near_cap, far_cap)| {
            let capacity = if near.area == Area::default() || far.area == Area::default() {
                0
            } else {
                bounded_border_capacity(near_cap, far_cap)?
            };
            total.checked_add(capacity)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::model::{
        usable_wire_capacity, DirCaps, PlacementStyle, PlannedResources, Slot, WIRE_CAPACITY_INF,
    };
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

    /// An ordinary slot: some fabric, and the given vertical budgets.
    fn mk_slot(x: u32, y: u32, north: u64, south: u64) -> Slot {
        Slot {
            x,
            y,
            area: Area {
                lut: 1,
                ..Area::default()
            },
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
        }
    }

    fn toy_device(rows: u32, cols: u32, slots: Vec<Slot>) -> Device {
        Device {
            key: "toy".to_string(),
            part_num: "toy".to_string(),
            platform_name: None,
            rows,
            cols,
            placement: PlacementStyle::default(),
            planned_resources: PlannedResources::default(),
            usage_ceiling: None,
            user_pblock_name: None,
            anchors: Vec::new(),
            slots,
        }
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
        // Each physical cell-pair boundary is governed by the *smaller* of its
        // two facing declarations: min(100, 1) + min(1, 100), derated. Summing
        // each region's full side first (min(101, 101) = 101) would credit
        // capacity that neither boundary can support — routing models the
        // same per-pair minimum.
        let device = toy_device(
            2,
            2,
            vec![
                mk_slot(0, 0, 100, 0),
                mk_slot(1, 0, 1, 0),
                mk_slot(0, 1, 0, 1),
                mk_slot(1, 1, 0, 100),
            ],
        );
        let rows = [Coor::span(0, 0, 1, 0), Coor::span(0, 1, 1, 1)];
        let cuts = find_cuts_for_regions(&device, &rows);
        assert_eq!(cuts[0].capacity, 2, "round(1 * 0.7) + round(1 * 0.7)");
    }

    #[test]
    fn partially_aligned_borders_count_only_the_shared_interval() {
        let device = toy_device(
            2,
            3,
            vec![
                mk_slot(0, 0, 10, 0),
                mk_slot(1, 0, 10, 0),
                mk_slot(2, 0, 10, 0),
                mk_slot(0, 1, 0, 10),
                mk_slot(1, 1, 0, 10),
                mk_slot(2, 1, 0, 10),
            ],
        );
        // The narrow region below shares only x=1 with the wide region above;
        // its other two slots border vertical cuts, not this pair.
        let narrow = Coor::span(1, 0, 1, 0);
        let wide = Coor::span(0, 1, 2, 1);
        assert_eq!(
            border_capacity(&device, &narrow, &wide),
            Some(7),
            "only the aligned slot pair contributes: round(10 * 0.7)",
        );
    }

    #[test]
    fn one_unconstrained_cell_pair_drops_the_whole_cut() {
        let device = toy_device(
            2,
            2,
            vec![
                mk_slot(0, 0, 10, 0),
                mk_slot(1, 0, WIRE_CAPACITY_INF, 0),
                mk_slot(0, 1, 0, 10),
                mk_slot(1, 1, 0, WIRE_CAPACITY_INF),
            ],
        );
        let rows = [Coor::span(0, 0, 1, 0), Coor::span(0, 1, 1, 1)];
        assert!(
            find_cuts_for_regions(&device, &rows).is_empty(),
            "column 1 is unconstrained, so the row boundary cannot bind",
        );
    }

    #[test]
    fn bounded_pairs_keep_the_cut_even_when_their_sum_is_huge() {
        let device = toy_device(
            2,
            2,
            vec![
                mk_slot(0, 0, 60_000_000, 0),
                mk_slot(1, 0, 60_000_000, 0),
                mk_slot(0, 1, 0, 60_000_000),
                mk_slot(1, 1, 0, 60_000_000),
            ],
        );
        let rows = [Coor::span(0, 0, 1, 0), Coor::span(0, 1, 1, 1)];
        let cuts = find_cuts_for_regions(&device, &rows);
        assert_eq!(
            cuts.len(),
            1,
            "every cell pair is bounded, so the cut binds"
        );
        assert_eq!(
            cuts[0].capacity,
            2 * usable_wire_capacity(60_000_000),
            "the sum of the two per-pair budgets: 2 * round(60M * 0.7)",
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

    /// U250's `X1Y1` is all shell. Its SLR borders lead nowhere a route may
    /// go, so the row cuts beside it hold only column 0's budget, while the
    /// y=2 cut keeps both columns.
    #[test]
    fn u250_row_cuts_skip_the_shell_slot_borders() {
        let device = select_device("u250").expect("u250");
        assert_eq!(device.slot(1, 1).expect("X1Y1").area, Area::default());
        let one_column = usable_wire_capacity(11_520);
        let capacities: Vec<(String, u64)> = find_all_slot_cuts(&device)
            .into_iter()
            .map(|cut| (cut.name, cut.capacity))
            .collect();
        assert_eq!(
            capacities,
            [
                ("y=0".to_string(), one_column),
                ("y=1".to_string(), one_column),
                ("y=2".to_string(), 2 * one_column),
            ]
        );
    }

    #[test]
    fn vck190_has_no_binding_cuts() {
        let device = select_device("vck190").expect("vck190");
        assert!(find_all_slot_cuts(&device).is_empty());
    }
}
