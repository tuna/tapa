//! Virtual FPGA device model: slots, device grid, pblock handling.

use serde::{Deserialize, Serialize};

use crate::area::{sum_area, Area};
use crate::FloorplanError;
use crate::SlotCoord;

/// Sentinel value for infinite wire capacity.
pub const WIRE_CAPACITY_INF: u64 = 100_000_000;

/// A virtual slot in the device grid.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VirtualSlot {
    pub area: Area,
    pub x: u32,
    pub y: u32,
    pub centroid_x_coor: f64,
    pub centroid_y_coor: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pblock_ranges: Option<Vec<String>>,
    #[serde(default = "default_wire_cap")]
    pub north_wire_capacity: u64,
    #[serde(default = "default_wire_cap")]
    pub south_wire_capacity: u64,
    #[serde(default = "default_wire_cap")]
    pub east_wire_capacity: u64,
    #[serde(default = "default_wire_cap")]
    pub west_wire_capacity: u64,
    #[serde(default)]
    pub north_anchor_region: Vec<String>,
    #[serde(default)]
    pub south_anchor_region: Vec<String>,
    #[serde(default)]
    pub east_anchor_region: Vec<String>,
    #[serde(default)]
    pub west_anchor_region: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

fn default_wire_cap() -> u64 {
    WIRE_CAPACITY_INF
}

impl VirtualSlot {
    /// Slot name in `SLOT_X{x}Y{y}_TO_SLOT_X{x}Y{y}` format.
    #[must_use]
    pub fn get_name(&self) -> String {
        format!(
            "SLOT_X{x}Y{y}_TO_SLOT_X{x}Y{y}",
            x = self.x,
            y = self.y
        )
    }

    /// Validate and sanitize all pblock-related attributes by merging
    /// `-add`/`-remove` lines.
    pub fn sanitize_pblock_range(&mut self) -> Result<(), FloorplanError> {
        let attrs: [&mut Vec<String>; 5] = [
            self.pblock_ranges.get_or_insert_with(Vec::new),
            &mut self.north_anchor_region,
            &mut self.south_anchor_region,
            &mut self.east_anchor_region,
            &mut self.west_anchor_region,
        ];
        for lines in attrs {
            sanitize_pblock_lines(lines)?;
        }
        Ok(())
    }
}

/// Merge multiple `-add`/`-remove` pblock lines into at most two lines.
fn sanitize_pblock_lines(lines: &mut Vec<String>) -> Result<(), FloorplanError> {
    if lines.is_empty() {
        return Ok(());
    }
    let mut add_ranges = Vec::new();
    let mut remove_ranges = Vec::new();
    for raw in lines.iter() {
        let line = raw.replace(['{', '}'], "");
        if let Some(rest) = line.strip_prefix("-add") {
            add_ranges.push(rest.to_owned());
        } else if let Some(rest) = line.strip_prefix("-remove") {
            remove_ranges.push(rest.to_owned());
        } else {
            return Err(FloorplanError::InvalidDevice(format!(
                "pblock line must start with -add or -remove: {raw}"
            )));
        }
    }
    let mut merged = Vec::new();
    if !add_ranges.is_empty() {
        merged.push(format!("-add {{{} }}", add_ranges.join("")));
    }
    if !remove_ranges.is_empty() {
        merged.push(format!("-remove {{{} }}", remove_ranges.join("")));
    }
    *lines = merged;
    Ok(())
}

/// A virtual FPGA device with a grid of slots.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VirtualDevice {
    pub slots: Vec<VirtualSlot>,
    pub rows: u32,
    pub cols: u32,
    pub pp_dist: f64,
    pub part_num: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub board_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_pblock_name: Option<String>,
}

impl VirtualDevice {
    /// Parse and validate a device from JSON.
    pub fn from_json(json: &str) -> Result<Self, FloorplanError> {
        let mut dev: Self = serde_json::from_str(json)?;
        dev.validate_and_init()?;
        Ok(dev)
    }

    /// Validate slot count and sanitize pblocks.
    pub fn validate_and_init(&mut self) -> Result<(), FloorplanError> {
        let expected = (self.rows * self.cols) as usize;
        if self.slots.len() != expected {
            return Err(FloorplanError::InvalidDevice(format!(
                "slots count ({}) != rows * cols ({} * {})",
                self.slots.len(),
                self.rows,
                self.cols
            )));
        }

        // Validate all slot coordinates exist and sanitize pblocks
        for slot in &mut self.slots {
            slot.sanitize_pblock_range()?;
        }

        // Verify all (x, y) positions are populated
        for x in 0..self.cols {
            for y in 0..self.rows {
                if !self.slots.iter().any(|s| s.x == x && s.y == y) {
                    return Err(FloorplanError::SlotNotFound(x, y));
                }
            }
        }

        Ok(())
    }

    /// Look up a slot by grid coordinates.
    pub fn get_slot(&self, x: u32, y: u32) -> Result<&VirtualSlot, FloorplanError> {
        self.slots
            .iter()
            .find(|s| s.x == x && s.y == y)
            .ok_or(FloorplanError::SlotNotFound(x, y))
    }

    /// Centroid of an island (average of lower-left and upper-right slot centroids).
    pub fn get_island_centroid(&self, coor: &SlotCoord) -> Result<(f64, f64), FloorplanError> {
        let dl = self.get_slot(coor.down_left_x, coor.down_left_y)?;
        let ur = self.get_slot(coor.up_right_x, coor.up_right_y)?;
        Ok((
            f64::midpoint(dl.centroid_x_coor, ur.centroid_x_coor),
            f64::midpoint(dl.centroid_y_coor, ur.centroid_y_coor),
        ))
    }

    /// Sum the areas of all slots in an island region.
    pub fn get_island_area(&self, coor: &SlotCoord) -> Result<Area, FloorplanError> {
        let mut areas = Vec::new();
        for (x, y) in coor.get_all_slot_coors() {
            areas.push(self.get_slot(x, y)?.area);
        }
        Ok(sum_area(&areas))
    }

    /// Collect pblock ranges for all slots in an island region.
    pub fn get_island_pblock_range(&self, coor: &SlotCoord) -> Result<Vec<String>, FloorplanError> {
        let mut ranges = Vec::new();
        for (x, y) in coor.get_all_slot_coors() {
            let slot = self.get_slot(x, y)?;
            let slot_ranges = slot.pblock_ranges.as_ref().ok_or_else(|| {
                FloorplanError::InvalidDevice(format!(
                    "slot ({x}, {y}) does not have pblock ranges"
                ))
            })?;
            ranges.extend(slot_ranges.iter().cloned());
        }
        Ok(ranges)
    }

    /// Pipeline level between two islands.
    ///
    /// At least one pipeline stage is needed per non-zero dimension.
    pub fn get_pipeline_level(
        &self,
        src: &SlotCoord,
        sink: &SlotCoord,
    ) -> Result<u32, FloorplanError> {
        let (src_x, src_y) = self.get_island_centroid(src)?;
        let (sink_x, sink_y) = self.get_island_centroid(sink)?;
        let dist_x = (src_x - sink_x).abs();
        let dist_y = (src_y - sink_y).abs();
        let pp_x = if dist_x > 0.0 {
            (dist_x / self.pp_dist).max(1.0)
        } else {
            0.0
        };
        let pp_y = if dist_y > 0.0 {
            (dist_y / self.pp_dist).max(1.0)
        } else {
            0.0
        };
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "pipeline levels are small positive integers"
        )]
        Ok((pp_x + pp_y).ceil() as u32)
    }

    /// Run all sanity checks on the device. Returns errors for fatal issues
    /// and logs warnings for non-fatal ones.
    pub fn sanity_check(&self) -> Result<(), FloorplanError> {
        self.check_unique_tags()
    }

    /// Check that no tag is assigned to multiple slots.
    fn check_unique_tags(&self) -> Result<(), FloorplanError> {
        let mut seen = std::collections::HashSet::new();
        for slot in &self.slots {
            for tag in &slot.tags {
                if !seen.insert(tag.clone()) {
                    return Err(FloorplanError::InvalidDevice(format!(
                        "tag {tag} assigned to multiple slots"
                    )));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_device(rows: u32, cols: u32) -> VirtualDevice {
        let mut slots = Vec::new();
        for x in 0..cols {
            for y in 0..rows {
                slots.push(VirtualSlot {
                    area: Area::new(100, 200, 10, 5, 2),
                    x,
                    y,
                    centroid_x_coor: f64::from(x) * 100.0,
                    centroid_y_coor: f64::from(y) * 150.0,
                    pblock_ranges: Some(vec![format!("-add {{SLICE_X{x}Y{y}}}")]),
                    north_wire_capacity: WIRE_CAPACITY_INF,
                    south_wire_capacity: WIRE_CAPACITY_INF,
                    east_wire_capacity: WIRE_CAPACITY_INF,
                    west_wire_capacity: WIRE_CAPACITY_INF,
                    north_anchor_region: Vec::new(),
                    south_anchor_region: Vec::new(),
                    east_anchor_region: Vec::new(),
                    west_anchor_region: Vec::new(),
                    tags: Vec::new(),
                });
            }
        }
        VirtualDevice {
            slots,
            rows,
            cols,
            pp_dist: 100.0,
            part_num: "xcu280".to_owned(),
            board_name: None,
            platform_name: None,
            user_pblock_name: None,
        }
    }

    #[test]
    fn device_validate_ok() {
        let mut dev = make_device(2, 2);
        dev.validate_and_init().expect("should validate");
    }

    #[test]
    fn device_wrong_slot_count() {
        let mut dev = make_device(2, 2);
        dev.slots.pop();
        let err = dev.validate_and_init().unwrap_err();
        assert!(
            err.to_string().contains("slots count"),
            "got: {err}"
        );
    }

    #[test]
    fn get_slot_ok() {
        let mut dev = make_device(2, 2);
        dev.validate_and_init().unwrap();
        let slot = dev.get_slot(1, 0).unwrap();
        assert_eq!(slot.x, 1);
        assert_eq!(slot.y, 0);
    }

    #[test]
    fn get_slot_out_of_bounds() {
        let mut dev = make_device(2, 2);
        dev.validate_and_init().unwrap();
        let err = dev.get_slot(5, 5).unwrap_err();
        assert!(
            err.to_string().contains("not found"),
            "got: {err}"
        );
    }

    #[test]
    fn island_centroid() {
        let mut dev = make_device(2, 2);
        dev.validate_and_init().unwrap();
        let coor = SlotCoord::new(0, 0, 1, 1);
        let (cx, cy) = dev.get_island_centroid(&coor).unwrap();
        // Average of (0,0) and (100,150) centroids
        assert!((cx - 50.0).abs() < 0.01, "cx={cx}");
        assert!((cy - 75.0).abs() < 0.01, "cy={cy}");
    }

    #[test]
    fn island_area() {
        let mut dev = make_device(2, 2);
        dev.validate_and_init().unwrap();
        let coor = SlotCoord::new(0, 0, 1, 1);
        let area = dev.get_island_area(&coor).unwrap();
        // 4 slots * 100 LUT each
        assert_eq!(area.lut, 400);
    }

    #[test]
    fn island_pblock_range() {
        let mut dev = make_device(2, 2);
        dev.validate_and_init().unwrap();
        let coor = SlotCoord::new(0, 0, 0, 0);
        let ranges = dev.get_island_pblock_range(&coor).unwrap();
        assert_eq!(ranges.len(), 1);
        assert!(
            ranges[0].starts_with("-add"),
            "got: {}",
            ranges[0]
        );
    }

    #[test]
    fn pipeline_level_same_slot() {
        let mut dev = make_device(2, 2);
        dev.validate_and_init().unwrap();
        let coor = SlotCoord::new(0, 0, 0, 0);
        let level = dev.get_pipeline_level(&coor, &coor).unwrap();
        assert_eq!(level, 0);
    }

    #[test]
    fn pipeline_level_adjacent() {
        let mut dev = make_device(2, 2);
        dev.validate_and_init().unwrap();
        let src = SlotCoord::new(0, 0, 0, 0);
        let sink = SlotCoord::new(1, 0, 1, 0);
        let level = dev.get_pipeline_level(&src, &sink).unwrap();
        assert!(level >= 1, "adjacent slots should need >= 1 pipeline stage, got {level}");
    }

    #[test]
    fn sanity_check_ok() {
        let mut dev = make_device(2, 2);
        dev.validate_and_init().unwrap();
        dev.sanity_check().unwrap();
    }

    #[test]
    fn sanity_check_duplicate_tags() {
        let mut dev = make_device(2, 2);
        dev.validate_and_init().unwrap();
        dev.slots[0].tags = vec!["dup".into()];
        dev.slots[1].tags = vec!["dup".into()];
        let err = dev.sanity_check().unwrap_err();
        assert!(
            err.to_string().contains("dup"),
            "got: {err}"
        );
    }

    #[test]
    fn pblock_sanitization() {
        let mut slot = VirtualSlot {
            area: Area::default(),
            x: 0,
            y: 0,
            centroid_x_coor: 0.0,
            centroid_y_coor: 0.0,
            pblock_ranges: Some(vec![
                "-add {SLICE_X0Y0}".into(),
                "-add {SLICE_X1Y0}".into(),
                "-remove {SLICE_X2Y0}".into(),
            ]),
            north_wire_capacity: WIRE_CAPACITY_INF,
            south_wire_capacity: WIRE_CAPACITY_INF,
            east_wire_capacity: WIRE_CAPACITY_INF,
            west_wire_capacity: WIRE_CAPACITY_INF,
            north_anchor_region: Vec::new(),
            south_anchor_region: Vec::new(),
            east_anchor_region: Vec::new(),
            west_anchor_region: Vec::new(),
            tags: Vec::new(),
        };
        slot.sanitize_pblock_range().unwrap();
        let ranges = slot.pblock_ranges.unwrap();
        assert_eq!(ranges.len(), 2);
        assert!(ranges[0].starts_with("-add"), "got: {}", ranges[0]);
        assert!(ranges[1].starts_with("-remove"), "got: {}", ranges[1]);
    }

    #[test]
    fn pblock_invalid_line_rejected() {
        let mut slot = VirtualSlot {
            area: Area::default(),
            x: 0,
            y: 0,
            centroid_x_coor: 0.0,
            centroid_y_coor: 0.0,
            pblock_ranges: Some(vec!["INVALID_LINE".into()]),
            north_wire_capacity: WIRE_CAPACITY_INF,
            south_wire_capacity: WIRE_CAPACITY_INF,
            east_wire_capacity: WIRE_CAPACITY_INF,
            west_wire_capacity: WIRE_CAPACITY_INF,
            north_anchor_region: Vec::new(),
            south_anchor_region: Vec::new(),
            east_anchor_region: Vec::new(),
            west_anchor_region: Vec::new(),
            tags: Vec::new(),
        };
        let err = slot.sanitize_pblock_range().unwrap_err();
        assert!(
            err.to_string().contains("-add or -remove"),
            "got: {err}"
        );
    }

    #[test]
    fn device_serde_round_trip() {
        let mut dev = make_device(2, 2);
        dev.validate_and_init().unwrap();
        let json = serde_json::to_string(&dev).unwrap();
        let dev2: VirtualDevice = serde_json::from_str(&json).unwrap();
        assert_eq!(dev2.rows, 2);
        assert_eq!(dev2.cols, 2);
        assert_eq!(dev2.slots.len(), 4);
    }
}
