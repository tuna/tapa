//! Pblock XDC emission: turn a [`FloorplanResult`] plus its [`Device`] into the
//! `create_pblock`/`resize_pblock`/`add_cells_to_pblock` constraints Vivado
//! applies during implementation.
//!
//! Ported from RapidStream's `graphir/transforms/pblock_gen.py`. Cells are
//! matched with hierarchical wildcards so they still resolve once `v++` places
//! the kernel below a platform prefix and synthesis flattens names.

use std::collections::BTreeMap;

use tapa_ir::FloorplanResult;

use crate::device::model::{Coor, Device};

/// Render the floorplan as XDC pblock constraints, terminated by a newline.
#[must_use]
pub fn emit_xdc(result: &FloorplanResult, device: &Device) -> String {
    // Group instances by region so each pblock is created once.
    let mut by_region: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (instance, region) in &result.regions {
        by_region.entry(region).or_default().push(instance);
    }

    let mut lines: Vec<String> = Vec::new();
    for (region, instances) in &by_region {
        let ranges = region_pblock_ranges(device, region);
        lines.push(format!("create_pblock {region}"));
        lines.push(format!(
            "resize_pblock {region} -add {{{}}}",
            ranges.join(" ")
        ));
        for instance in instances {
            lines.push(format!(
                "add_cells_to_pblock {region} [get_cells -hierarchical -filter {{NAME =~ \"*{instance}*\"}}]"
            ));
        }
    }

    let mut text = lines.join("\n");
    text.push('\n');
    text
}

/// The union of pblock ranges of every atomic slot a region covers.
fn region_pblock_ranges(device: &Device, region: &str) -> Vec<String> {
    let Some(coor) = Coor::from_region_name(region) else {
        return Vec::new();
    };
    let mut ranges = Vec::new();
    for (x, y) in coor.all_slot_coors() {
        if let Some(slot) = device.slot(x, y) {
            ranges.extend(slot.pblock_ranges.iter().cloned());
        }
    }
    ranges
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::select::select_device;
    use std::collections::BTreeMap as Map;
    use tapa_ir::Area;

    #[test]
    #[allow(
        clippy::literal_string_with_formatting_args,
        reason = "the golden XDC contains literal Tcl braces, not format args"
    )]
    fn golden_xdc_for_a_colocated_design() {
        let result = FloorplanResult {
            device: "u280".to_string(),
            grid: (2, 3),
            regions: Map::from([
                ("A_0".to_string(), "SLOT_X0Y0_TO_SLOT_X0Y0".to_string()),
                ("B_0".to_string(), "SLOT_X0Y0_TO_SLOT_X0Y0".to_string()),
                (
                    "fifo_VecAdd".to_string(),
                    "SLOT_X0Y0_TO_SLOT_X0Y0".to_string(),
                ),
            ]),
            crossings: Vec::new(),
            slot_usage: Map::from([(
                "SLOT_X0Y0_TO_SLOT_X0Y0".to_string(),
                Area {
                    lut: 150,
                    ff: 260,
                    bram_18k: 0,
                    dsp: 0,
                    uram: 0,
                },
            )]),
        };
        let device = select_device("u280").expect("u280");

        let expected = "\
create_pblock SLOT_X0Y0_TO_SLOT_X0Y0
resize_pblock SLOT_X0Y0_TO_SLOT_X0Y0 -add {CLOCKREGION_X0Y0:CLOCKREGION_X3Y3}
add_cells_to_pblock SLOT_X0Y0_TO_SLOT_X0Y0 [get_cells -hierarchical -filter {NAME =~ \"*A_0*\"}]
add_cells_to_pblock SLOT_X0Y0_TO_SLOT_X0Y0 [get_cells -hierarchical -filter {NAME =~ \"*B_0*\"}]
add_cells_to_pblock SLOT_X0Y0_TO_SLOT_X0Y0 [get_cells -hierarchical -filter {NAME =~ \"*fifo_VecAdd*\"}]
";
        assert_eq!(emit_xdc(&result, &device), expected);
    }

    #[test]
    fn multi_slot_region_unions_ranges() {
        // A region spanning column 0's bottom two rows unions both pblocks.
        let result = FloorplanResult {
            device: "u280".to_string(),
            grid: (2, 3),
            regions: Map::from([("T".to_string(), "SLOT_X0Y0_TO_SLOT_X0Y1".to_string())]),
            crossings: Vec::new(),
            slot_usage: Map::new(),
        };
        let device = select_device("u280").expect("u280");
        let xdc = emit_xdc(&result, &device);
        assert!(
            xdc.contains("CLOCKREGION_X0Y0:CLOCKREGION_X3Y3"),
            "row 0 range"
        );
        assert!(
            xdc.contains("CLOCKREGION_X0Y4:CLOCKREGION_X3Y7"),
            "row 1 range"
        );
    }
}
