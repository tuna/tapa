//! Convert Vivado's physical census into the existing floorplan device model.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};
use tapa_floorplan::device::model::{
    Device, DeviceAnchor, DirCaps, PlacementStyle, PlannedResources, Slot, UNIT_DIST_X, UNIT_DIST_Y,
};
use tapa_ir::Area;

use crate::bd::Interface;
use crate::{Result, Shell};

#[derive(Clone, Serialize)]
pub struct Location {
    pub site: String,
    pub slr: u32,
    pub clock_region_x: i64,
    pub clock_region_y: i64,
}

#[derive(Default)]
struct Slr {
    area: Area,
    regions: Vec<(i64, i64)>,
    north: u64,
    south: u64,
}

#[derive(Deserialize)]
struct Noc {
    #[serde(rename = "Components")]
    components: Vec<NocComponent>,
}

#[derive(Deserialize)]
struct NocComponent {
    #[serde(rename = "Name")]
    site: String,
    #[serde(rename = "TrafficLInst")]
    instance: Option<String>,
}

#[derive(Default)]
pub struct Census {
    pub part: String,
    pub vivado: String,
    pub rm_cell: String,
    pub pins: BTreeMap<String, String>,
    pub clocks: BTreeMap<String, f64>,
    pub pblock_ranges: Vec<String>,
    slrs: BTreeMap<u32, Slr>,
    sites: BTreeMap<String, Location>,
    endpoints: BTreeMap<String, String>,
}

pub fn read(output: &Path) -> Result<Census> {
    let mut census = Census::default();
    for line in fs::read_to_string(output.join("census.tsv"))?.lines() {
        census.record(line)?;
    }
    if census.part.is_empty() || census.slrs.is_empty() {
        return Err("incomplete Vivado census".into());
    }
    for line in fs::read_to_string(output.join("pins.tsv"))?.lines() {
        let (name, direction) = line.split_once('\t').ok_or("invalid pin record")?;
        if census
            .pins
            .insert(name.to_owned(), direction.to_owned())
            .is_some()
        {
            return Err(format!("duplicate DCP pin {name}").into());
        }
    }
    let noc: Noc = serde_json::from_slice(&fs::read(output.join("noc.ncr"))?)?;
    for component in noc.components {
        if let Some(name) = component.instance.and_then(|i| {
            i.strip_prefix(&format!("{}/", census.rm_cell))
                .map(str::to_owned)
        }) {
            if census
                .endpoints
                .insert(name.clone(), component.site)
                .is_some()
            {
                return Err(format!("ambiguous NoC endpoint {name}").into());
            }
        }
    }
    census.subtract_static_slls(&fs::read_to_string(output.join("utilization.rpt"))?)?;
    Ok(census)
}

impl Census {
    fn record(&mut self, line: &str) -> Result<()> {
        let fields: Vec<_> = line.split('\t').collect();
        match fields.as_slice() {
            ["design", part, vivado, rm] => {
                (*part).clone_into(&mut self.part);
                (*vivado).clone_into(&mut self.vivado);
                (*rm).clone_into(&mut self.rm_cell);
            }
            ["range", range] => self.pblock_ranges.push((*range).to_owned()),
            ["region", slr, region] => self
                .slrs
                .entry(slr.parse()?)
                .or_default()
                .regions
                .push(coordinates(region)?),
            ["slr", slr, north, south] => {
                let slr = self.slrs.entry(slr.parse()?).or_default();
                slr.north = north.parse()?;
                slr.south = south.parse()?;
            }
            ["resource", slr, kind, state, count] => {
                let count: u64 = count.parse()?;
                if *state == "available" {
                    let area = &mut self.slrs.entry(slr.parse()?).or_default().area;
                    match *kind {
                        "SLICEL" | "SLICEM" => {
                            area.lut += 8 * count;
                            area.ff += 16 * count;
                        }
                        "DSP58_PRIMARY" => area.dsp += count,
                        "RAMB18_L" | "RAMB18_U" => area.bram_18k += count,
                        "URAM288" => area.uram += count,
                        _ => return Err(format!("unknown resource {kind}").into()),
                    }
                } else if *state != "excluded" {
                    return Err("invalid site state".into());
                }
            }
            ["site", name, slr, region] => {
                let (clock_region_x, clock_region_y) = coordinates(region)?;
                self.sites.insert(
                    (*name).to_owned(),
                    Location {
                        site: (*name).to_owned(),
                        slr: slr.parse()?,
                        clock_region_x,
                        clock_region_y,
                    },
                );
            }
            ["hbm", name, site] => {
                self.endpoints
                    .insert((*name).to_owned(), (*site).to_owned());
            }
            ["clock", name, period] => {
                self.clocks.insert((*name).to_owned(), period.parse()?);
            }
            _ => return Err(format!("unknown census record {line:?}").into()),
        }
        Ok(())
    }

    fn subtract_static_slls(&mut self, report: &str) -> Result<()> {
        for lower in 0..u32::try_from(self.slrs.len())? - 1 {
            let upper = lower + 1;
            let name = format!("SLR{upper} <-> SLR{lower}");
            let rows = report
                .lines()
                .map(|line| line.split('|').map(str::trim).collect::<Vec<_>>())
                .filter(|fields| fields.get(1) == Some(&name.as_str()))
                .collect::<Vec<_>>();
            let [fields] = rows.as_slice() else {
                return Err(format!("missing/ambiguous SLL utilization for {name}").into());
            };
            let used: u64 = fields.get(2).ok_or("missing SLL used count")?.parse()?;
            let available: u64 = fields.get(4).ok_or("missing SLL capacity")?.parse()?;
            let north = self.slrs.get(&lower).ok_or("missing lower SLR")?.north;
            let south = self.slrs.get(&upper).ok_or("missing upper SLR")?.south;
            if north != south || north != available {
                return Err(format!("inconsistent SLL capacities at {name}").into());
            }
            let remaining = available
                .checked_sub(used)
                .ok_or("SLL usage exceeds capacity")?;
            self.slrs.get_mut(&lower).ok_or("missing lower SLR")?.north = remaining;
            self.slrs.get_mut(&upper).ok_or("missing upper SLR")?.south = remaining;
        }
        Ok(())
    }

    pub fn location(&self, name: &str) -> Result<Option<Location>> {
        self.endpoints
            .get(name)
            .map(|site| {
                self.sites
                    .get(site)
                    .cloned()
                    .ok_or_else(|| format!("unknown NoC site {site} for {name}").into())
            })
            .transpose()
    }

    pub fn device(&self, shell: Shell, interfaces: &[Interface], pblock: &str) -> Result<Device> {
        let mut slots = Vec::new();
        let mut anchors = Vec::new();
        for (&y, slr) in &self.slrs {
            let (min_x, max_x, min_y, max_y) = bounds(&slr.regions)?;
            let mut tags = Vec::new();
            for interface in interfaces {
                if let (Some(tag), Some(location)) = (&interface.tag, &interface.location) {
                    if location.slr != y {
                        continue;
                    }
                    tags.push(tag.clone());
                    anchors.push(DeviceAnchor {
                        tag: tag.clone(),
                        site: location.site.clone(),
                        x_offset_quarters: offset(
                            location.clock_region_x,
                            min_x,
                            max_x,
                            UNIT_DIST_X,
                        )?,
                        y_offset_quarters: offset(
                            location.clock_region_y,
                            min_y,
                            max_y,
                            UNIT_DIST_Y,
                        )?,
                    });
                }
            }
            slots.push(Slot {
                x: 0,
                y,
                area: slr.area,
                centroid_x: 0,
                centroid_y: i64::from(y) * UNIT_DIST_Y,
                // Descriptive envelope: SlrAssignment emits no child pblocks;
                // the exact shell outline remains in the parent pblock.
                pblock_ranges: region_ranges(&slr.regions),
                wire_cap: DirCaps {
                    north: slr.north,
                    south: slr.south,
                    ..DirCaps::default()
                },
                tags,
            });
        }
        Ok(Device {
            key: format!("v80_{}", shell.name()),
            part_num: self.part.clone(),
            platform_name: Some(format!("slash_v80_{}", shell.name())),
            rows: u32::try_from(slots.len())?,
            cols: 1,
            placement: PlacementStyle::SlrAssignment,
            planned_resources: PlannedResources::default(),
            usage_ceiling: None,
            user_pblock_name: Some(pblock.to_owned()),
            slots,
            anchors,
        })
    }
}

fn coordinates(region: &str) -> Result<(i64, i64)> {
    let (x, y) = region
        .strip_prefix('X')
        .and_then(|s| s.split_once('Y'))
        .ok_or("invalid clock region")?;
    Ok((x.parse()?, y.parse()?))
}

fn region_ranges(regions: &[(i64, i64)]) -> Vec<String> {
    let mut rows = BTreeMap::new();
    for &(x, y) in regions {
        let (min, max) = rows.entry(y).or_insert((x, x));
        *min = (*min).min(x);
        *max = (*max).max(x);
    }
    rows.into_iter()
        .map(|(y, (min, max))| format!("CLOCKREGION_X{min}Y{y}:CLOCKREGION_X{max}Y{y}"))
        .collect()
}

fn bounds(regions: &[(i64, i64)]) -> Result<(i64, i64, i64, i64)> {
    let &(x, y) = regions.first().ok_or("SLR has no fabric clock regions")?;
    Ok(regions
        .iter()
        .fold((x, x, y, y), |(min_x, max_x, min_y, max_y), &(x, y)| {
            (min_x.min(x), max_x.max(x), min_y.min(y), max_y.max(y))
        }))
}

fn offset(coordinate: i64, min: i64, max: i64, spacing: i64) -> Result<i64> {
    if !(min..=max).contains(&coordinate) {
        return Err("anchor lies outside its SLR clock-region envelope".into());
    }
    Ok((2 * coordinate - min - max) * 2 * spacing / (max - min + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v80_counts_physical_units_and_subtracts_static_slls() -> Result<()> {
        let mut census = Census::default();
        for line in [
            "slr\t0\t18870\t0",
            "slr\t1\t0\t18870",
            "resource\t0\tSLICEL\tavailable\t2",
            "resource\t0\tSLICEM\tavailable\t3",
            "resource\t0\tSLICEL\texcluded\t1",
            "resource\t0\tRAMB18_L\tavailable\t4",
            "resource\t0\tRAMB18_U\tavailable\t4",
            "resource\t0\tDSP58_PRIMARY\tavailable\t6",
        ] {
            census.record(line)?;
        }
        census.subtract_static_slls("| SLR1 <-> SLR0 | 7 | | 18870 | 0.04 |")?;
        assert_eq!(
            census.slrs[&0].area,
            Area {
                lut: 40,
                ff: 80,
                bram_18k: 8,
                dsp: 6,
                uram: 0
            },
            "count LUT6, registers, DSP58 and 18K RAM without alternate-site double counting"
        );
        assert_eq!(
            census.slrs[&0].north, 18863,
            "static routes consume SLL capacity"
        );
        assert_eq!(census.slrs[&1].south, 18863, "both sides must agree");
        Ok(())
    }
}
