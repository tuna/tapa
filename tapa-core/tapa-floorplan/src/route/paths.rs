//! Candidate path enumeration for the post-placement routing MILP.
//!
//! Every *simple* path between the endpoints within `Manhattan + max_detour`
//! hops is enumerated. Candidate grids are small (a few SLR rows by a few
//! columns), so the four-shape restriction of earlier revisions only ever
//! caused false infeasibility on congested mid-routes, at no meaningful
//! solver saving. Paths are returned shortest-first with a lexicographic
//! tie-break, which the routing refinement's stable rank objective relies on.

use std::collections::BTreeSet;

/// A grid coordinate `(x, y)`.
pub type Cell = (u32, u32);

/// Manhattan distance in hops between two cells.
fn manhattan(a: Cell, b: Cell) -> usize {
    usize::try_from(a.0.abs_diff(b.0) + a.1.abs_diff(b.1)).expect("distance fits usize")
}

/// Enumerate every simple grid path from `src` to `dst`.
///
/// `grid_cols` and `grid_rows` are the grid dimensions, and `max_detour` is
/// the maximum number of additional *hops* beyond a Manhattan path. Paths
/// list every visited slot, including `src` and `dst`, are returned
/// shortest-first with a lexicographic tie-break, and contain no repeated
/// slots. Invalid endpoints yield no candidates; a same-slot request yields
/// the singleton path.
#[must_use]
pub fn enumerate_paths(
    src: Cell,
    dst: Cell,
    grid_cols: u32,
    grid_rows: u32,
    max_detour: usize,
) -> Vec<Vec<Cell>> {
    let in_bounds = |&(x, y): &Cell| x < grid_cols && y < grid_rows;
    if !in_bounds(&src) || !in_bounds(&dst) {
        return Vec::new();
    }

    let hop_budget = manhattan(src, dst) + max_detour;
    let mut paths = Vec::new();
    let mut visited = BTreeSet::from([src]);
    let mut path = vec![src];
    visit(
        dst,
        hop_budget,
        &in_bounds,
        &mut path,
        &mut visited,
        &mut paths,
    );
    // Shortest first; the lexicographic cell order breaks length ties, so the
    // candidate index the routing refinement minimizes is fully stable.
    paths.sort_by(|a, b| a.len().cmp(&b.len()).then_with(|| a.cmp(b)));
    paths
}

/// Depth-first enumeration of simple paths to `dst` within `hop_budget` hops.
fn visit(
    dst: Cell,
    hop_budget: usize,
    in_bounds: &impl Fn(&Cell) -> bool,
    path: &mut Vec<Cell>,
    visited: &mut BTreeSet<Cell>,
    paths: &mut Vec<Vec<Cell>>,
) {
    let current = *path.last().expect("a path always has a head");
    if current == dst {
        paths.push(path.clone());
        return;
    }
    // Prune: the remaining hops cannot reach the destination in budget even
    // on a Manhattan-optimal continuation.
    let hops_used = path.len() - 1;
    if hops_used + manhattan(current, dst) > hop_budget {
        return;
    }
    let (x, y) = current;
    for next in [
        (x + 1, y),
        (x, y + 1),
        (x.wrapping_sub(1), y),
        (x, y.wrapping_sub(1)),
    ] {
        if !in_bounds(&next) || visited.contains(&next) {
            continue;
        }
        path.push(next);
        visited.insert(next);
        visit(dst, hop_budget, in_bounds, path, visited, paths);
        visited.remove(&next);
        path.pop();
    }
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
    fn one_by_two_enumerates_every_manhattan_path_shortest_first() {
        assert_eq!(
            enumerate_paths((0, 0), (1, 2), 10, 10, 0),
            vec![
                vec![(0, 0), (0, 1), (0, 2), (1, 2)],
                vec![(0, 0), (0, 1), (1, 1), (1, 2)],
                vec![(0, 0), (1, 0), (1, 1), (1, 2)],
            ],
            "all three Manhattan paths, length ties broken lexicographically",
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
        // Without detours: every Manhattan (monotone) path, C(dx+dy, dx).
        assert_eq!(enumerate_paths((1, 1), (3, 3), 10, 10, 0).len(), 6);
        assert_eq!(enumerate_paths((3, 0), (0, 5), 10, 10, 0).len(), 56);
        // A two-hop budget adds every single out-and-back detour that keeps
        // the path simple and in bounds.
        assert_eq!(enumerate_paths((1, 1), (3, 3), 4, 4, 2).len(), 20);
        assert_eq!(enumerate_paths((1, 1), (3, 3), 4, 5, 2).len(), 25);
        assert_eq!(enumerate_paths((1, 1), (3, 3), 5, 5, 2).len(), 30);
    }

    #[test]
    fn all_candidates_are_simple_adjacent_paths_within_budget() {
        let budget = manhattan((1, 3), (3, 0)) + 2 + 1; // hops + detour -> cells
        for path in enumerate_paths((1, 3), (3, 0), 5, 5, 2) {
            assert_eq!(path.first(), Some(&(1, 3)), "source is preserved");
            assert_eq!(path.last(), Some(&(3, 0)), "destination is preserved");
            assert!(path.len() <= budget, "within the detour budget: {path:?}");
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
