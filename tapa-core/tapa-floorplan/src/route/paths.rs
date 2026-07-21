//! Candidate path enumeration for the routing ILP.
//!
//! For a net between two slots we enumerate the monotone grid paths (every
//! step moves toward the destination) with at most a bounded number of
//! direction changes, matching RapidStream's `route_design/router.py` ≤2-bend
//! candidate set. On the coarse device grid these are few, so the routing ILP
//! stays small.

/// A grid coordinate `(x, y)`.
pub type Cell = (u32, u32);

/// Enumerate the monotone paths from `src` to `dst`.
///
/// Paths have at most `max_bends` direction changes and list the full slot
/// sequence, `src` first and `dst` last; a same-slot request yields the
/// singleton path.
#[must_use]
pub fn enumerate_paths(src: Cell, dst: Cell, max_bends: usize) -> Vec<Vec<Cell>> {
    let (sx, sy) = src;
    let (dx, dy) = dst;
    let x_steps = dx.abs_diff(sx);
    let y_steps = dy.abs_diff(sy);
    let step_count = (x_steps + y_steps) as usize;
    if step_count == 0 {
        return vec![vec![src]];
    }

    let x_dir: i64 = if dx >= sx { 1 } else { -1 };
    let y_dir: i64 = if dy >= sy { 1 } else { -1 };

    let mut paths = Vec::new();
    // Each move slot is either an x-move or a y-move; a bit mask over the
    // `step_count` positions with exactly `x_steps` bits set is one monotone
    // interleaving.
    for mask in 0u32..(1u32 << step_count) {
        if mask.count_ones() != x_steps {
            continue;
        }
        let mut path = Vec::with_capacity(step_count + 1);
        path.push(src);
        let mut cx = i64::from(sx);
        let mut cy = i64::from(sy);
        let mut bends = 0usize;
        let mut prev_is_x: Option<bool> = None;
        for position in 0..step_count {
            let is_x = (mask >> position) & 1 == 1;
            if prev_is_x.is_some_and(|prev| prev != is_x) {
                bends += 1;
            }
            prev_is_x = Some(is_x);
            if is_x {
                cx += x_dir;
            } else {
                cy += y_dir;
            }
            let x = u32::try_from(cx).expect("monotone path stays in the grid");
            let y = u32::try_from(cy).expect("monotone path stays in the grid");
            path.push((x, y));
        }
        if bends <= max_bends {
            paths.push(path);
        }
    }
    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn straight_path_is_unique() {
        assert_eq!(
            enumerate_paths((0, 0), (0, 2), 2),
            vec![vec![(0, 0), (0, 1), (0, 2)]],
            "a vertical run has one monotone path",
        );
    }

    #[test]
    fn same_slot_is_a_singleton() {
        assert_eq!(enumerate_paths((1, 1), (1, 1), 2), vec![vec![(1, 1)]]);
    }

    #[test]
    fn diagonal_has_two_l_shaped_paths() {
        let paths = enumerate_paths((0, 0), (1, 1), 2);
        assert_eq!(paths.len(), 2, "two single-bend L paths");
        assert!(paths.contains(&vec![(0, 0), (1, 0), (1, 1)]), "x then y");
        assert!(paths.contains(&vec![(0, 0), (0, 1), (1, 1)]), "y then x");
    }

    #[test]
    fn bend_limit_prunes_staircases() {
        // From (0,0) to (1,2): monotone interleavings of 1 x and 2 y moves.
        // XYY / YYX are 1 bend; YXY is 2 bends. All survive at max_bends=2.
        assert_eq!(enumerate_paths((0, 0), (1, 2), 2).len(), 3);
        // At max_bends=1 the staircase YXY (2 bends) is pruned.
        assert_eq!(enumerate_paths((0, 0), (1, 2), 1).len(), 2);
    }

    #[test]
    fn every_path_starts_and_ends_correctly() {
        for path in enumerate_paths((1, 3), (0, 0), 2) {
            assert_eq!(path.first(), Some(&(1, 3)));
            assert_eq!(path.last(), Some(&(0, 0)));
        }
    }
}
