//! Candidate path enumeration for the post-placement routing MILP.
//!
//! Candidate generation enumerates the aligned direct path, the two one-bend
//! paths, and every H-V-H / V-H-V path in the device grid. Stable
//! deduplication happens before paths with more than `max_detour` extra slot
//! visits or repeated slots are removed.

use std::collections::BTreeSet;

/// A grid coordinate `(x, y)`.
pub type Cell = (u32, u32);

/// Generate the x-first straight path from `start` to `end`.
///
/// All segments constructed by [`enumerate_paths`] are axis-aligned. Keeping
/// the x-first behavior here makes the ordering deterministic if this helper's
/// use changes later.
fn straight_path(start: Cell, end: Cell) -> Vec<Cell> {
    let mut path = vec![start];
    while path.last().copied() != Some(end) {
        let (x, y) = path.last().copied().expect("a path always has a head");
        let next = if x == end.0 {
            (x, if end.1 > y { y + 1 } else { y - 1 })
        } else {
            (if end.0 > x { x + 1 } else { x - 1 }, y)
        };
        path.push(next);
    }
    path
}

/// Join axis-aligned path segments, retaining each shared endpoint once.
fn join_segments(segments: impl IntoIterator<Item = Vec<Cell>>) -> Vec<Cell> {
    let mut path = Vec::new();
    for segment in segments {
        if path.is_empty() {
            path.extend(segment);
        } else {
            path.extend(segment.into_iter().skip(1));
        }
    }
    path
}

/// Insert `path` once while preserving the generation order.
fn push_unique(paths: &mut Vec<Vec<Cell>>, seen: &mut BTreeSet<Vec<Cell>>, path: Vec<Cell>) {
    if seen.insert(path.clone()) {
        paths.push(path);
    }
}

/// Enumerate candidate paths from `src` to `dst`.
///
/// `grid_cols` and `grid_rows` are the grid dimensions, and `max_detour` is
/// the maximum number of additional *slot visits* beyond a Manhattan path.
/// Paths list every visited slot, including `src` and `dst`. Invalid endpoints
/// yield no candidates; a same-slot request yields the singleton path.
#[must_use]
pub fn enumerate_paths(
    src: Cell,
    dst: Cell,
    grid_cols: u32,
    grid_rows: u32,
    max_detour: usize,
) -> Vec<Vec<Cell>> {
    if src.0 >= grid_cols || src.1 >= grid_rows || dst.0 >= grid_cols || dst.1 >= grid_rows {
        return Vec::new();
    }

    let mut paths = Vec::new();
    let mut seen = BTreeSet::new();

    // Direct path.
    if src.0 == dst.0 || src.1 == dst.1 {
        push_unique(&mut paths, &mut seen, straight_path(src, dst));
    }

    // One-bend paths: vertical-horizontal, then horizontal-vertical.
    for bend in [(src.0, dst.1), (dst.0, src.1)] {
        push_unique(
            &mut paths,
            &mut seen,
            join_segments([straight_path(src, bend), straight_path(bend, dst)]),
        );
    }

    // H-V-H paths, one for every possible intermediate column.
    for x in 0..grid_cols {
        let first_bend = (x, src.1);
        let second_bend = (x, dst.1);
        push_unique(
            &mut paths,
            &mut seen,
            join_segments([
                straight_path(src, first_bend),
                straight_path(first_bend, second_bend),
                straight_path(second_bend, dst),
            ]),
        );
    }

    // V-H-V paths, one for every possible intermediate row.
    for y in 0..grid_rows {
        let first_bend = (src.0, y);
        let second_bend = (dst.0, y);
        push_unique(
            &mut paths,
            &mut seen,
            join_segments([
                straight_path(src, first_bend),
                straight_path(first_bend, second_bend),
                straight_path(second_bend, dst),
            ]),
        );
    }

    let optimal_slot_count = usize::try_from(src.0.abs_diff(dst.0) + src.1.abs_diff(dst.1))
        .expect("grid dimensions fit usize")
        + 1;
    paths
        .into_iter()
        .filter(|path| path.len() <= optimal_slot_count + max_detour)
        .filter(|path| path.iter().copied().collect::<BTreeSet<_>>().len() == path.len())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn straight_path_is_unique_without_detours() {
        assert_eq!(
            enumerate_paths((0, 0), (0, 2), 4, 4, 0),
            vec![vec![(0, 0), (0, 1), (0, 2)]],
            "an aligned run has one candidate when detours are disabled",
        );
    }

    #[test]
    fn same_slot_is_a_singleton() {
        assert_eq!(enumerate_paths((1, 1), (1, 1), 4, 4, 2), vec![vec![(1, 1)]],);
    }

    #[test]
    fn diagonal_has_two_one_bend_paths_without_detours() {
        assert_eq!(
            enumerate_paths((0, 0), (1, 1), 4, 4, 0),
            vec![vec![(0, 0), (0, 1), (1, 1)], vec![(0, 0), (1, 0), (1, 1)],],
            "candidate order is deterministic",
        );
    }

    #[test]
    fn one_by_two_has_expected_order() {
        assert_eq!(
            enumerate_paths((0, 0), (1, 2), 10, 10, 0),
            vec![
                vec![(0, 0), (0, 1), (0, 2), (1, 2)],
                vec![(0, 0), (1, 0), (1, 1), (1, 2)],
                vec![(0, 0), (0, 1), (1, 1), (1, 2)],
            ],
        );
    }

    #[test]
    fn adjacent_detours_are_grid_bounded() {
        assert_eq!(
            enumerate_paths((1, 0), (1, 1), 3, 4, 2),
            vec![
                vec![(1, 0), (1, 1)],
                vec![(1, 0), (0, 0), (0, 1), (1, 1)],
                vec![(1, 0), (2, 0), (2, 1), (1, 1)],
            ],
        );
        assert_eq!(
            enumerate_paths((0, 1), (0, 0), 10, 10, 2),
            vec![vec![(0, 1), (0, 0)], vec![(0, 1), (1, 1), (1, 0), (0, 0)],],
        );
    }

    #[test]
    fn reference_candidate_counts_match() {
        assert_eq!(enumerate_paths((1, 1), (3, 3), 10, 10, 0).len(), 4);
        assert_eq!(enumerate_paths((3, 0), (0, 5), 10, 10, 0).len(), 8);
        assert_eq!(enumerate_paths((1, 1), (3, 3), 4, 4, 2).len(), 6);
        assert_eq!(enumerate_paths((1, 1), (3, 3), 4, 5, 2).len(), 7);
        assert_eq!(enumerate_paths((1, 1), (3, 3), 5, 5, 2).len(), 8);
    }

    #[test]
    fn all_candidates_are_simple_adjacent_paths() {
        for path in enumerate_paths((1, 3), (3, 0), 5, 5, 2) {
            assert_eq!(path.first(), Some(&(1, 3)), "source is preserved");
            assert_eq!(path.last(), Some(&(3, 0)), "destination is preserved");
            assert_eq!(
                path.iter().copied().collect::<BTreeSet<_>>().len(),
                path.len(),
                "candidate may not revisit a slot: {path:?}",
            );
            assert!(
                path.windows(2)
                    .all(|hop| hop[0].0.abs_diff(hop[1].0) + hop[0].1.abs_diff(hop[1].1) == 1),
                "every hop must cross one adjacent boundary: {path:?}",
            );
        }
    }

    #[test]
    fn invalid_endpoints_have_no_candidates() {
        assert!(
            enumerate_paths((0, 0), (4, 0), 4, 4, 2).is_empty(),
            "destination is outside the grid",
        );
        assert!(
            enumerate_paths((0, 4), (0, 0), 4, 4, 2).is_empty(),
            "source is outside the grid",
        );
    }
}
