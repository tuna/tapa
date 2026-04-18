//! Slot naming, coordinate parsing, and adjacency utilities.

use regex::Regex;
use std::sync::LazyLock;

/// Regex matching TAPA slot naming pattern: `SLOT_X{dl_x}Y{dl_y}_TO_SLOT_X{ur_x}Y{ur_y}`.
static SLOT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^SLOT_X(\d+)Y(\d+)_TO_SLOT_X(\d+)Y(\d+)$").unwrap());

/// A rectangular region defined by lower-left and upper-right coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SlotCoord {
    pub down_left_x: u32,
    pub down_left_y: u32,
    pub up_right_x: u32,
    pub up_right_y: u32,
}

impl SlotCoord {
    /// Create a new `SlotCoord`.
    pub fn new(down_left_x: u32, down_left_y: u32, up_right_x: u32, up_right_y: u32) -> Self {
        Self {
            down_left_x,
            down_left_y,
            up_right_x,
            up_right_y,
        }
    }

    /// Format as a TAPA slot name.
    pub fn to_slot_name(&self) -> String {
        format!(
            "SLOT_X{}Y{}_TO_SLOT_X{}Y{}",
            self.down_left_x, self.down_left_y, self.up_right_x, self.up_right_y
        )
    }

    /// Convert to configuration pattern format: `SLOT_X{dl_x}Y{dl_y}:SLOT_X{ur_x}Y{ur_y}`.
    pub fn to_config_pattern(&self) -> String {
        format!(
            "SLOT_X{}Y{}:SLOT_X{}Y{}",
            self.down_left_x, self.down_left_y, self.up_right_x, self.up_right_y
        )
    }

    /// Check if `self` is geometrically inside `other` (inclusive boundaries).
    pub fn is_inside(&self, other: &Self) -> bool {
        self.down_left_x >= other.down_left_x
            && self.down_left_y >= other.down_left_y
            && self.up_right_x <= other.up_right_x
            && self.up_right_y <= other.up_right_y
    }

    /// Check if `self` is the south neighbor of `other` (shares a horizontal edge).
    pub fn is_south_neighbor_of(&self, other: &Self) -> bool {
        self.up_right_y + 1 == other.down_left_y
            && self.down_left_x.max(other.down_left_x) <= self.up_right_x.min(other.up_right_x)
    }

    /// Check if `self` is the north neighbor of `other`.
    pub fn is_north_neighbor_of(&self, other: &Self) -> bool {
        self.down_left_y == other.up_right_y + 1
            && self.down_left_x.max(other.down_left_x) <= self.up_right_x.min(other.up_right_x)
    }

    /// Check if `self` is the east neighbor of `other`.
    pub fn is_east_neighbor_of(&self, other: &Self) -> bool {
        self.down_left_x == other.up_right_x + 1
            && self.down_left_y.max(other.down_left_y) <= self.up_right_y.min(other.up_right_y)
    }

    /// Check if `self` is the west neighbor of `other`.
    pub fn is_west_neighbor_of(&self, other: &Self) -> bool {
        self.up_right_x + 1 == other.down_left_x
            && self.down_left_y.max(other.down_left_y) <= self.up_right_y.min(other.up_right_y)
    }

    /// Check if `self` is adjacent to `other` (any shared edge).
    pub fn is_neighbor(&self, other: &Self) -> bool {
        self.is_north_neighbor_of(other)
            || self.is_south_neighbor_of(other)
            || self.is_east_neighbor_of(other)
            || self.is_west_neighbor_of(other)
    }

    /// Check if two slots are horizontal neighbors (east or west).
    pub fn is_horizontal_neighbor(&self, other: &Self) -> bool {
        self.is_east_neighbor_of(other) || self.is_west_neighbor_of(other)
    }

    /// Check if two slots are vertical neighbors (north or south).
    pub fn is_vertical_neighbor(&self, other: &Self) -> bool {
        self.is_north_neighbor_of(other) || self.is_south_neighbor_of(other)
    }

    /// Check if the current rectangle overlaps with another (inclusive boundaries).
    pub fn has_overlap(&self, other: &Self) -> bool {
        if other.down_left_x > self.up_right_x {
            return false;
        }
        if other.up_right_x < self.down_left_x {
            return false;
        }
        if other.down_left_y > self.up_right_y {
            return false;
        }
        other.up_right_y >= self.down_left_y
    }

    /// Check if the current rectangle is perfectly (exactly) tiled by the given children.
    ///
    /// Returns `false` if any children overlap each other, if any child cell falls
    /// outside of `self`, or if any cell within `self` is not covered.
    pub fn is_perfectly_covered_by(&self, children: &[Self]) -> bool {
        let width = (self.up_right_x - self.down_left_x + 1) as usize;
        let height = (self.up_right_y - self.down_left_y + 1) as usize;
        let mut visited = vec![false; width * height];

        for child in children {
            for x in child.down_left_x..=child.up_right_x {
                for y in child.down_left_y..=child.up_right_y {
                    if x < self.down_left_x
                        || x > self.up_right_x
                        || y < self.down_left_y
                        || y > self.up_right_y
                    {
                        return false;
                    }
                    let idx =
                        (x - self.down_left_x) as usize * height + (y - self.down_left_y) as usize;
                    if visited[idx] {
                        return false; // overlap
                    }
                    visited[idx] = true;
                }
            }
        }

        visited.iter().all(|&v| v)
    }

    /// Check if `self` fully contains `other` (inclusive boundaries).
    pub fn covers(&self, other: &Self) -> bool {
        self.down_left_x <= other.down_left_x
            && self.down_left_y <= other.down_left_y
            && self.up_right_x >= other.up_right_x
            && self.up_right_y >= other.up_right_y
    }

    /// Calculate the overlap area between two rectangles.
    ///
    /// Returns the number of grid cells in the intersection,
    /// measured as the area between grid lines (not grid point count).
    pub fn calculate_overlap(&self, other: &Self) -> u32 {
        let left = self.down_left_x.max(other.down_left_x);
        let bottom = self.down_left_y.max(other.down_left_y);
        let right = self.up_right_x.min(other.up_right_x);
        let top = self.up_right_y.min(other.up_right_y);

        let width = right.saturating_sub(left);
        let height = top.saturating_sub(bottom);

        width * height
    }

    /// Check if a point `(x, y)` is within this rectangle (inclusive).
    pub fn point_covers(&self, x: u32, y: u32) -> bool {
        self.down_left_x <= x
            && x <= self.up_right_x
            && self.down_left_y <= y
            && y <= self.up_right_y
    }

    /// Return all `(x, y)` grid positions within this rectangle (inclusive).
    pub fn get_all_slot_coors(&self) -> Vec<(u32, u32)> {
        let mut result = Vec::new();
        for x in self.down_left_x..=self.up_right_x {
            for y in self.down_left_y..=self.up_right_y {
                result.push((x, y));
            }
        }
        result
    }
}

/// Check if a string matches the TAPA slot naming pattern.
pub fn is_valid_slot(name: &str) -> bool {
    SLOT_RE.is_match(name)
}

/// Parse a slot name into coordinates.
///
/// Returns `None` if the name doesn't match the expected pattern.
pub fn parse_slot_name(name: &str) -> Option<SlotCoord> {
    let caps = SLOT_RE.captures(name)?;
    Some(SlotCoord::new(
        caps[1].parse().ok()?,
        caps[2].parse().ok()?,
        caps[3].parse().ok()?,
        caps[4].parse().ok()?,
    ))
}

/// Convert a slot name to configuration pattern format.
///
/// Returns `None` if the name is not a valid slot name.
pub fn convert_to_config_pattern(name: &str) -> Option<String> {
    parse_slot_name(name).map(|c| c.to_config_pattern())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_slot_names() {
        assert!(is_valid_slot("SLOT_X0Y0_TO_SLOT_X3Y3"));
        assert!(is_valid_slot("SLOT_X10Y20_TO_SLOT_X30Y40"));
    }

    #[test]
    fn invalid_slot_names() {
        assert!(!is_valid_slot("SLOT_X0Y0"));
        assert!(!is_valid_slot("invalid"));
        assert!(!is_valid_slot("SLOT_XaYb_TO_SLOT_XcYd"));
        assert!(!is_valid_slot(""));
    }

    #[test]
    fn parse_coordinates() {
        let coord = parse_slot_name("SLOT_X0Y0_TO_SLOT_X3Y3").unwrap();
        assert_eq!(coord.down_left_x, 0);
        assert_eq!(coord.down_left_y, 0);
        assert_eq!(coord.up_right_x, 3);
        assert_eq!(coord.up_right_y, 3);
    }

    #[test]
    fn round_trip_name() {
        let name = "SLOT_X1Y2_TO_SLOT_X5Y6";
        let coord = parse_slot_name(name).unwrap();
        assert_eq!(coord.to_slot_name(), name);
    }

    #[test]
    fn config_pattern() {
        assert_eq!(
            convert_to_config_pattern("SLOT_X0Y0_TO_SLOT_X3Y3"),
            Some("SLOT_X0Y0:SLOT_X3Y3".to_owned())
        );
    }

    #[test]
    fn inside_check() {
        let inner = SlotCoord::new(1, 1, 3, 3);
        let outer = SlotCoord::new(0, 0, 4, 4);
        assert!(inner.is_inside(&outer));
        assert!(!outer.is_inside(&inner));
        // Identical is inside
        assert!(inner.is_inside(&inner));
    }

    #[test]
    fn south_neighbor() {
        let south = SlotCoord::new(0, 0, 2, 1);
        let north = SlotCoord::new(0, 2, 2, 3);
        assert!(south.is_south_neighbor_of(&north));
        assert!(!north.is_south_neighbor_of(&south));
    }

    #[test]
    fn north_neighbor() {
        let south = SlotCoord::new(0, 0, 2, 1);
        let north = SlotCoord::new(0, 2, 2, 3);
        assert!(north.is_north_neighbor_of(&south));
    }

    #[test]
    fn east_west_neighbor() {
        let west = SlotCoord::new(0, 0, 1, 2);
        let east = SlotCoord::new(2, 0, 3, 2);
        assert!(east.is_east_neighbor_of(&west));
        assert!(west.is_west_neighbor_of(&east));
    }

    #[test]
    fn non_adjacent_no_overlap() {
        let a = SlotCoord::new(0, 0, 1, 1);
        let b = SlotCoord::new(3, 3, 4, 4);
        assert!(!a.is_neighbor(&b));
    }

    #[test]
    fn horizontal_vertical_helpers() {
        let left = SlotCoord::new(0, 0, 1, 2);
        let right = SlotCoord::new(2, 0, 3, 2);
        assert!(left.is_horizontal_neighbor(&right));
        assert!(!left.is_vertical_neighbor(&right));
    }

    // -- has_overlap tests (from Python Coor doctests) --

    #[test]
    fn has_overlap_identical() {
        let a = SlotCoord::new(1, 1, 3, 3);
        assert!(a.has_overlap(&a));
    }

    #[test]
    fn has_overlap_disjoint_right() {
        let a = SlotCoord::new(1, 1, 3, 3);
        let b = SlotCoord::new(4, 1, 6, 3);
        assert!(!a.has_overlap(&b));
    }

    #[test]
    fn has_overlap_disjoint_above() {
        let a = SlotCoord::new(1, 1, 3, 3);
        let b = SlotCoord::new(1, 4, 3, 6);
        assert!(!a.has_overlap(&b));
    }

    #[test]
    fn has_overlap_disjoint_diagonal() {
        let a = SlotCoord::new(1, 1, 3, 3);
        let b = SlotCoord::new(4, 4, 6, 6);
        assert!(!a.has_overlap(&b));
    }

    #[test]
    fn has_overlap_partial() {
        let a = SlotCoord::new(1, 1, 3, 3);
        let b = SlotCoord::new(2, 2, 4, 4);
        assert!(a.has_overlap(&b));
    }

    #[test]
    fn has_overlap_contained() {
        let a = SlotCoord::new(1, 1, 3, 3);
        let b = SlotCoord::new(2, 2, 2, 2);
        assert!(a.has_overlap(&b));
    }

    #[test]
    fn has_overlap_corner_touch() {
        let a = SlotCoord::new(1, 1, 3, 3);
        // (0,0)-(0,0) does NOT overlap (1,1)-(3,3)
        let b = SlotCoord::new(0, 0, 0, 0);
        assert!(!a.has_overlap(&b));
        // (0,0)-(1,1) DOES overlap (1,1)-(3,3)
        let c = SlotCoord::new(0, 0, 1, 1);
        assert!(a.has_overlap(&c));
    }

    #[test]
    fn has_overlap_adjacent_not_overlapping() {
        // Adjacent slots sharing an edge boundary but not overlapping
        let a = SlotCoord::new(0, 0, 1, 1);
        let b = SlotCoord::new(2, 0, 3, 1);
        assert!(!a.has_overlap(&b));
    }

    // -- covers tests --

    #[test]
    fn covers_identical() {
        let a = SlotCoord::new(1, 1, 2, 2);
        assert!(a.covers(&a));
    }

    #[test]
    fn covers_subset() {
        let outer = SlotCoord::new(1, 1, 3, 3);
        let inner = SlotCoord::new(1, 2, 2, 2);
        assert!(outer.covers(&inner));
        assert!(!inner.covers(&outer));
    }

    #[test]
    fn covers_partial_not() {
        let a = SlotCoord::new(1, 1, 2, 2);
        let b = SlotCoord::new(1, 1, 3, 3);
        assert!(!a.covers(&b));
    }

    // -- is_perfectly_covered_by tests --

    #[test]
    fn perfectly_covered_by_self() {
        let a = SlotCoord::new(1, 1, 3, 3);
        assert!(a.is_perfectly_covered_by(&[a]));
    }

    #[test]
    fn perfectly_covered_gap() {
        let a = SlotCoord::new(1, 1, 3, 3);
        let partial = SlotCoord::new(1, 1, 2, 3);
        assert!(!a.is_perfectly_covered_by(&[partial]));
    }

    #[test]
    fn perfectly_covered_with_overlap() {
        let a = SlotCoord::new(1, 1, 3, 3);
        let left = SlotCoord::new(1, 1, 2, 3);
        let right = SlotCoord::new(2, 1, 3, 3); // overlaps at x=2
        assert!(!a.is_perfectly_covered_by(&[left, right]));
    }

    #[test]
    fn perfectly_covered_exact_tiling() {
        let a = SlotCoord::new(1, 1, 3, 3);
        let left = SlotCoord::new(1, 1, 2, 3);
        let right = SlotCoord::new(3, 1, 3, 3);
        assert!(a.is_perfectly_covered_by(&[left, right]));
    }

    // -- calculate_overlap tests --

    #[test]
    fn calculate_overlap_partial() {
        let a = SlotCoord::new(0, 0, 5, 5);
        let b = SlotCoord::new(3, 3, 8, 8);
        assert_eq!(a.calculate_overlap(&b), 4);
    }

    #[test]
    fn calculate_overlap_disjoint() {
        let a = SlotCoord::new(0, 0, 1, 1);
        let b = SlotCoord::new(2, 2, 3, 3);
        assert_eq!(a.calculate_overlap(&b), 0);
    }

    #[test]
    fn calculate_overlap_touching_corner() {
        let a = SlotCoord::new(0, 0, 5, 5);
        let b = SlotCoord::new(5, 5, 10, 10);
        assert_eq!(a.calculate_overlap(&b), 0);
    }

    #[test]
    fn calculate_overlap_identical() {
        let a = SlotCoord::new(0, 0, 10, 10);
        assert_eq!(a.calculate_overlap(&a), 100);
    }

    // -- point_covers tests --

    #[test]
    fn point_covers_inside() {
        let a = SlotCoord::new(1, 1, 3, 3);
        assert!(a.point_covers(1, 1));
        assert!(a.point_covers(2, 2));
        assert!(a.point_covers(3, 3));
    }

    #[test]
    fn point_covers_outside() {
        let a = SlotCoord::new(1, 1, 3, 3);
        assert!(!a.point_covers(0, 0));
        assert!(!a.point_covers(4, 4));
    }

    // -- get_all_slot_coors tests --

    #[test]
    fn get_all_slot_coors_single() {
        let a = SlotCoord::new(2, 3, 2, 3);
        assert_eq!(a.get_all_slot_coors(), vec![(2, 3)]);
    }

    #[test]
    fn get_all_slot_coors_rect() {
        let a = SlotCoord::new(0, 0, 1, 1);
        let coors = a.get_all_slot_coors();
        assert_eq!(coors.len(), 4);
        assert!(coors.contains(&(0, 0)));
        assert!(coors.contains(&(0, 1)));
        assert!(coors.contains(&(1, 0)));
        assert!(coors.contains(&(1, 1)));
    }
}
