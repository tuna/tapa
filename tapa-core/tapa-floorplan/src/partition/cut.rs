//! Guillotine cuts: the clean horizontal/vertical bipartitions of the device
//! grid whose crossed wire width the floorplan ILP caps.
//!
//! Ported from RapidStream's `autobridge/partition/cut.py`. Each cut's
//! capacity is `usable_wire_ratio` (0.7) times the summed facing border wire
//! capacities of the two sides. A cut whose capacity reaches the "infinite"
//! sentinel (any uncapped border) does not bind and is dropped.

use crate::device::model::{Coor, Device, USABLE_WIRE_RATIO, WIRE_CAPACITY_INF};

/// A clean bipartition of the grid into two facing sides.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cut {
    /// Cut label, e.g. `y=0` (row boundary) or `x=1` (column boundary).
    pub name: String,
    /// Slots on the down/left side.
    pub lhs: Vec<Coor>,
    /// Slots on the up/right side.
    pub rhs: Vec<Coor>,
    /// Allowed crossing width: `round(0.7 · Σ facing border capacities)`.
    pub capacity: u64,
}

/// Apply the usable-wire ratio to a raw summed capacity.
///
/// Integer `raw · 7 / 10` stands in for `0.7 · raw`; the two agree for the
/// device tables' capacities and the difference is immaterial to a derating.
fn apply_ratio(raw: u64) -> u64 {
    // USABLE_WIRE_RATIO is 0.7; kept as a named constant for provenance.
    debug_assert!(
        (USABLE_WIRE_RATIO - 0.7).abs() < f64::EPSILON,
        "ratio drifted"
    );
    raw * 7 / 10
}

/// Enumerate every binding guillotine cut of `device`'s grid.
///
/// Horizontal cuts sit between adjacent rows (summing north/south border
/// capacities); vertical cuts between adjacent columns (east/west). Cuts whose
/// capacity reaches the infinite sentinel are omitted — they never constrain.
#[must_use]
pub fn find_cuts(device: &Device) -> Vec<Cut> {
    let mut cuts = Vec::new();
    let cap_threshold = WIRE_CAPACITY_INF / 2;

    // Horizontal cuts: row y_cut | row y_cut+1.
    for y_cut in 0..device.rows.saturating_sub(1) {
        let raw: u64 = (0..device.cols)
            .filter_map(|x| {
                let down = device.slot(x, y_cut)?;
                let up = device.slot(x, y_cut + 1)?;
                Some(down.wire_cap.north.min(up.wire_cap.south))
            })
            .sum();
        let capacity = apply_ratio(raw);
        if capacity >= cap_threshold {
            continue;
        }
        let lhs = slots_where(device, |_, y| y <= y_cut);
        let rhs = slots_where(device, |_, y| y > y_cut);
        cuts.push(Cut {
            name: format!("y={y_cut}"),
            lhs,
            rhs,
            capacity,
        });
    }

    // Vertical cuts: column x_cut | column x_cut+1.
    for x_cut in 0..device.cols.saturating_sub(1) {
        let raw: u64 = (0..device.rows)
            .filter_map(|y| {
                let left = device.slot(x_cut, y)?;
                let right = device.slot(x_cut + 1, y)?;
                Some(left.wire_cap.east.min(right.wire_cap.west))
            })
            .sum();
        let capacity = apply_ratio(raw);
        if capacity >= cap_threshold {
            continue;
        }
        let lhs = slots_where(device, |x, _| x <= x_cut);
        let rhs = slots_where(device, |x, _| x > x_cut);
        cuts.push(Cut {
            name: format!("x={x_cut}"),
            lhs,
            rhs,
            capacity,
        });
    }

    cuts
}

/// The single-slot [`Coor`]s of every device slot satisfying `keep(x, y)`.
fn slots_where(device: &Device, keep: impl Fn(u32, u32) -> bool) -> Vec<Coor> {
    device
        .slots
        .iter()
        .filter(|s| keep(s.x, s.y))
        .map(|s| Coor::slot(s.x, s.y))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::select::select_device;

    #[test]
    fn u280_has_two_row_cuts_and_one_column_cut() {
        let device = select_device("u280").expect("u280");
        let cuts = find_cuts(&device);
        let names: Vec<&str> = cuts.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["y=0", "y=1", "x=0"], "u280 grid is 2x3");

        // Row cut: two columns of min(11520, 11520); 0.7·23040 = 16128.
        let row = cuts.iter().find(|c| c.name == "y=0").unwrap();
        assert_eq!(row.capacity, 16128);
        assert_eq!(row.lhs.len(), 2, "row 0");
        assert_eq!(row.rhs.len(), 4, "rows 1 and 2");

        // Column cut: three rows of min(40320, 40320); 0.7·120960 = 84672.
        let col = cuts.iter().find(|c| c.name == "x=0").unwrap();
        assert_eq!(col.capacity, 84672);
        assert_eq!(col.lhs.len(), 3, "column 0");
        assert_eq!(col.rhs.len(), 3, "column 1");
    }

    #[test]
    fn u250_drops_the_uncapped_column_cut() {
        // u250 sets no east/west capacities, so the vertical cut is infinite
        // and non-binding; only the three row cuts survive.
        let device = select_device("u250").expect("u250");
        let names: Vec<String> = find_cuts(&device).into_iter().map(|c| c.name).collect();
        assert_eq!(names, ["y=0", "y=1", "y=2"]);
    }

    #[test]
    fn vck190_has_no_binding_cuts() {
        // vck190 ships without wire capacities, so nothing binds.
        let device = select_device("vck190").expect("vck190");
        assert!(find_cuts(&device).is_empty());
    }
}
