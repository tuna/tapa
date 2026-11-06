//! Read the RM boundary in the static top-level BD, not the RM's placeholder BD.

use std::collections::BTreeMap;
use std::fmt::Write;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::census::{Census, Location};
use crate::Result;

#[derive(Deserialize)]
struct Document {
    design: Design,
}

#[derive(Deserialize)]
struct Design {
    design_info: Info,
    components: Components,
}

#[derive(Deserialize)]
struct Components {
    slash: Option<Component>,
}

#[derive(Deserialize)]
struct Info {
    device: String,
    tool_version: String,
}

#[derive(Default, Deserialize)]
struct Component {
    #[serde(default)]
    interface_ports: BTreeMap<String, Bus>,
    #[serde(default)]
    ports: BTreeMap<String, Port>,
}

#[derive(Deserialize)]
struct Parameter {
    value: String,
}

#[derive(Deserialize)]
struct Bus {
    mode: String,
    vlnv: String,
    #[serde(default)]
    parameters: BTreeMap<String, Parameter>,
    #[serde(default)]
    apertures: BTreeMap<String, Aperture>,
    port_maps: BTreeMap<String, Port>,
}

#[derive(Deserialize)]
struct Aperture {
    offset: String,
    range: String,
}

#[derive(Deserialize)]
struct Port {
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default)]
    physical_name: String,
    direction: String,
    left: Option<String>,
    right: Option<String>,
    #[serde(default)]
    parameters: BTreeMap<String, Parameter>,
}

pub struct Bd {
    pub part: String,
    pub version: String,
    rm: Component,
}

#[derive(Debug, Serialize)]
pub struct AddressRange {
    pub base: u64,
    pub size: u64,
}

#[derive(Serialize)]
pub struct Interface {
    pub name: String,
    pub tag: Option<String>,
    pub mode: String,
    pub vlnv: String,
    pub address_width: Option<u64>,
    pub data_width: Option<u64>,
    pub apertures: Vec<AddressRange>,
    pub location: Option<Location>,
}

#[derive(Serialize)]
pub struct Clock {
    pub port: String,
    pub frequency_hz: u64,
    pub period_ns: f64,
    pub associated_interfaces: Vec<String>,
}

pub fn read(path: &Path) -> Result<Bd> {
    let document: Document = serde_json::from_slice(&fs::read(path)?)?;
    let rm = document.design.components.slash.ok_or(
        "expected the static top.bd containing component slash; slash_base.bd is insufficient",
    )?;
    if rm.interface_ports.is_empty() {
        return Err("slash has no interface ports".into());
    }
    Ok(Bd {
        part: document.design.design_info.device,
        version: document.design.design_info.tool_version,
        rm,
    })
}

impl Bd {
    pub fn write_requests(&self, output: &Path) -> Result<()> {
        let mut requests = String::new();
        for (name, bus) in &self.rm.interface_ports {
            if name.starts_with("HBM_AXI_") {
                let pin = bus
                    .port_maps
                    .get("ARADDR")
                    .ok_or("HBM interface has no ARADDR")?;
                let first = pin
                    .pins(&pin.physical_name)?
                    .into_keys()
                    .next()
                    .ok_or("empty ARADDR")?;
                writeln!(requests, "hbm\t{name}\t{first}")?;
            }
        }
        for (name, port) in &self.rm.ports {
            if port.kind == "clk" {
                writeln!(requests, "clock\t{name}\t{name}")?;
            }
        }
        fs::write(output.join("requests.tsv"), requests)?;
        Ok(())
    }

    pub fn describe(&self, census: &Census) -> Result<(Vec<Interface>, Vec<Clock>)> {
        let mut expected = BTreeMap::new();
        for bus in self.rm.interface_ports.values() {
            for port in bus.port_maps.values() {
                expected.extend(port.pins(&port.physical_name)?);
            }
        }
        for (name, port) in &self.rm.ports {
            expected.extend(port.pins(name)?);
        }
        if expected != census.pins {
            let different = expected
                .iter()
                .find(|(pin, dir)| census.pins.get(*pin) != Some(*dir));
            return Err(format!(
                "BD/DCP pin mismatch: expected {}, found {}; first mismatch {different:?}",
                expected.len(),
                census.pins.len()
            )
            .into());
        }
        let interfaces = self
            .rm
            .interface_ports
            .iter()
            .map(|(name, bus)| {
                let tag = tag(name, bus)?;
                let location = census.location(name)?;
                if tag.is_some() && location.is_none() {
                    return Err(format!("no physical anchor for {name}").into());
                }
                let apertures = bus
                    .apertures
                    .values()
                    .map(address_range)
                    .collect::<Result<Vec<_>>>()?;
                bus.check_apertures(name, &apertures)?;
                Ok(Interface {
                    name: name.clone(),
                    tag,
                    mode: bus.mode.clone(),
                    vlnv: bus.vlnv.clone(),
                    address_width: number(&bus.parameters, "ADDR_WIDTH")?,
                    data_width: number(&bus.parameters, "DATA_WIDTH")?,
                    apertures,
                    location,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let clocks = self
            .rm
            .ports
            .iter()
            .filter(|(_, port)| port.kind == "clk")
            .map(|(name, port)| {
                let frequency_hz =
                    number(&port.parameters, "FREQ_HZ")?.ok_or("clock missing FREQ_HZ")?;
                let period_ns = *census.clocks.get(name).ok_or("DCP clock missing")?;
                // Vivado reports periods rounded to picoseconds.
                let frequency = f64::from(u32::try_from(frequency_hz)?);
                if frequency_hz == 0
                    || !period_ns.is_finite()
                    || period_ns <= 0.0
                    || (period_ns - 1e9 / frequency).abs() > 0.001
                {
                    return Err(format!(
                        "BD/DCP clock mismatch at {name}: {frequency_hz} Hz vs {period_ns} ns"
                    )
                    .into());
                }
                let associated_interfaces = port
                    .parameters
                    .get("ASSOCIATED_BUSIF")
                    .map_or_else(Vec::new, |p| {
                        p.value.split(':').map(str::to_owned).collect()
                    });
                Ok(Clock {
                    port: name.clone(),
                    frequency_hz,
                    period_ns,
                    associated_interfaces,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        if clocks.is_empty() {
            return Err("no RM clocks".into());
        }
        Ok((interfaces, clocks))
    }
}

impl Bus {
    fn check_apertures(&self, name: &str, apertures: &[AddressRange]) -> Result<()> {
        if apertures.is_empty()
            && matches!(
                self.vlnv.as_str(),
                "xilinx.com:interface:aximm_rtl:1.0" | "xilinx.com:interface:inimm_rtl:1.0"
            )
        {
            return Err(format!("{name} has no parent BD aperture").into());
        }
        Ok(())
    }
}

impl Port {
    fn pins(&self, name: &str) -> Result<BTreeMap<String, String>> {
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(format!("unsupported boundary pin name {name:?}").into());
        }
        let direction = match self.direction.as_str() {
            "I" => "IN",
            "O" => "OUT",
            "IO" => "INOUT",
            other => return Err(format!("unknown pin direction {other}").into()),
        };
        let names = match (&self.left, &self.right) {
            (Some(left), Some(right)) => {
                let left: u32 = left.parse()?;
                let right: u32 = right.parse()?;
                if left.abs_diff(right) > 4096 {
                    return Err("unsupported boundary bus width".into());
                }
                (left.min(right)..=left.max(right))
                    .map(|i| format!("{name}[{i}]"))
                    .collect()
            }
            (None, None) => vec![name.to_owned()],
            _ => return Err("boundary bus has only one bound".into()),
        };
        Ok(names
            .into_iter()
            .map(|name| (name, direction.to_owned()))
            .collect())
    }
}

fn number(parameters: &BTreeMap<String, Parameter>, key: &str) -> Result<Option<u64>> {
    parameters
        .get(key)
        .map(|p| p.value.parse().map_err(Into::into))
        .transpose()
}

fn address_range(aperture: &Aperture) -> Result<AddressRange> {
    let base = quantity(&aperture.offset)?;
    let size = quantity(&aperture.range)?;
    if size == 0 || base.checked_add(size - 1).is_none() {
        return Err("invalid address aperture".into());
    }
    Ok(AddressRange { base, size })
}

fn quantity(value: &str) -> Result<u64> {
    if let Some(hex) = value.strip_prefix("0x") {
        return Ok(u64::from_str_radix(hex, 16)?);
    }
    for (suffix, shift) in [("K", 10), ("M", 20), ("G", 30), ("T", 40)] {
        if let Some(number) = value.strip_suffix(suffix) {
            return number
                .parse::<u64>()?
                .checked_mul(1 << shift)
                .ok_or_else(|| "address range overflow".into());
        }
    }
    Ok(value.parse()?)
}

fn tag(name: &str, bus: &Bus) -> Result<Option<String>> {
    // SLASH's bd_ports.py names HBM_AXI_nn and Mnn_INI as HBM[n] and DDR[n].
    let result = if let Some(index) = name.strip_prefix("HBM_AXI_") {
        Some((
            format!("HBM[{}]", index.parse::<u32>()?),
            "Master",
            "aximm_rtl",
        ))
    } else if let Some(index) = name.strip_prefix('M').and_then(|n| n.strip_suffix("_INI")) {
        Some((
            format!("DDR[{}]", index.parse::<u32>()?),
            "Master",
            "inimm_rtl",
        ))
    } else if name == "S_AXILITE_INI" {
        Some(("S_AXI_CONTROL".to_owned(), "Slave", "inimm_rtl"))
    } else {
        None
    };
    result
        .map(|(tag, mode, protocol)| {
            if bus.mode != mode || bus.vlnv != format!("xilinx.com:interface:{protocol}:1.0") {
                return Err(format!("unexpected interface contract at {name}").into());
            }
            Ok(tag)
        })
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_stream_interfaces_need_no_address_window() -> Result<()> {
        let mut bus: Bus = serde_json::from_str(
            r#"{"mode":"Master","vlnv":"xilinx.com:interface:inis_rtl:1.0","port_maps":{}}"#,
        )?;
        bus.check_apertures("M_DCMAC_INIS0", &[])?;
        bus.vlnv = "xilinx.com:interface:inimm_rtl:1.0".to_owned();
        bus.check_apertures("M00_INI", &[])
            .expect_err("memory-mapped INI still requires a parent address window");
        Ok(())
    }

    #[test]
    fn preserves_full_width_apertures_and_multidigit_ddr_tags() -> Result<()> {
        let aperture = Aperture {
            offset: "0x060000000000".into(),
            range: "32G".into(),
        };
        let range = address_range(&aperture)?;
        assert_eq!(
            range.base, 0x0600_0000_0000,
            "preserve the high address bits"
        );
        assert_eq!(range.size, 32 << 30, "Vivado aperture units are binary");
        let bus: Bus = serde_json::from_str(
            r#"{"mode":"Master","vlnv":"xilinx.com:interface:inimm_rtl:1.0","port_maps":{}}"#,
        )?;
        assert_eq!(
            tag("M11_INI", &bus)?.as_deref(),
            Some("DDR[11]"),
            "multi-digit DDR index must survive"
        );
        assert!(
            quantity("18446744073709551615G").is_err(),
            "reject overflow"
        );
        Ok(())
    }
}
