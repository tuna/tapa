//! `kernel.xml` port projection + `m_axi` bus-parameter block for
//! `tapa pack`.
//!
//! Projects task ports into kernel XML, expands channel arrays, and
//! emits bus parameters for every `m_axi` port.

use std::collections::BTreeSet;
use tapa_ir::{ArgCategory, Port, Target};
use tapa_rtl::module::sanitize_array_name;
use tapa_xilinx::{KernelXmlPort, PortCategory};

/// Project a `tapa_ir::Port` list into the `KernelXmlPort` shape
/// `tapa_xilinx::emit_kernel_xml` expects, including the channel fan-out
/// unrolling for hmap ports.
///
/// `m_axi_bases` names the M-AXI ports the generated RTL actually declares;
/// an empty set means "take the ports at their declared fan-out".
pub(super) fn build_kernel_xml_ports(
    ports: &[Port],
    m_axi_bases: &BTreeSet<String>,
) -> Vec<KernelXmlPort> {
    let mut out = Vec::<KernelXmlPort>::new();
    for port in ports {
        let category = match port.cat {
            ArgCategory::Scalar => PortCategory::Scalar,
            ArgCategory::Mmap
            | ArgCategory::Immap
            | ArgCategory::Ommap
            | ArgCategory::AsyncMmap => PortCategory::MAxi,
            ArgCategory::Istream | ArgCategory::Istreams => PortCategory::IStream,
            ArgCategory::Ostream | ArgCategory::Ostreams => PortCategory::OStream,
        };
        for name in projected_port_names(port, m_axi_bases) {
            out.push(KernelXmlPort {
                name,
                category,
                width: port.width,
                port: String::new(),
                ctype: port.ctype.clone(),
            });
        }
    }
    out
}

/// Add `HAS_BURST=0` and `SUPPORTS_NARROW_BURST=0` to each `m_axi` port.
pub(super) fn m_axi_param_block(
    ports: &[Port],
    m_axi_bases: &BTreeSet<String>,
) -> Vec<(String, Vec<(String, String)>)> {
    let mut out = Vec::<(String, Vec<(String, String)>)>::new();
    let kv = vec![
        ("HAS_BURST".to_string(), "0".to_string()),
        ("SUPPORTS_NARROW_BURST".to_string(), "0".to_string()),
    ];
    for port in ports {
        if !port.cat.is_mmap_like() {
            continue;
        }
        for name in projected_port_names(port, m_axi_bases) {
            out.push((name, kv.clone()));
        }
    }
    out
}

fn projected_port_names(port: &Port, m_axi_bases: &BTreeSet<String>) -> Vec<String> {
    let base = sanitize_array_name(&port.name);
    let default_names = port.kernel_arg_names(Target::XilinxVitis);
    if !port.cat.is_mmap_like() {
        return default_names;
    }
    if m_axi_bases.contains(&base) {
        return vec![base];
    }
    let present: Vec<String> = default_names
        .iter()
        .filter(|name| m_axi_bases.contains(*name))
        .cloned()
        .collect();
    if present.is_empty() {
        default_names
    } else {
        present
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::testutil::port;

    #[test]
    fn build_kernel_xml_ports_translates_categories() {
        let ports = vec![
            port("n", ArgCategory::Scalar, "int", 32),
            port("gmem", ArgCategory::Mmap, "int*", 512),
            port("i0", ArgCategory::Istream, "tapa::istream<int>", 32),
        ];
        let out = build_kernel_xml_ports(&ports, &BTreeSet::new());
        assert_eq!(out.len(), 3);
        assert!(matches!(out[0].category, PortCategory::Scalar));
        assert!(matches!(out[1].category, PortCategory::MAxi));
        assert!(matches!(out[2].category, PortCategory::IStream));
    }

    #[test]
    fn build_kernel_xml_ports_unrolls_chan_count() {
        let ports = vec![Port {
            chan_count: Some(3),
            ..port("gmem", ArgCategory::Mmap, "int*", 64)
        }];
        let out = build_kernel_xml_ports(&ports, &BTreeSet::new());
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].name, "gmem_0");
        assert_eq!(out[1].name, "gmem_1");
        assert_eq!(out[2].name, "gmem_2");
    }

    #[test]
    fn build_kernel_xml_ports_sanitizes_indexed_names() {
        let ports = vec![port("chan[0]", ArgCategory::Mmap, "int*", 64)];
        let out = build_kernel_xml_ports(&ports, &BTreeSet::new());
        assert_eq!(out[0].name, "chan_0");
        let block = m_axi_param_block(&ports, &BTreeSet::new());
        assert_eq!(block[0].0, "chan_0");
    }

    #[test]
    fn rtl_m_axi_bases_override_chan_count_unrolling() {
        let ports = vec![Port {
            chan_count: Some(2),
            ..port("mat_a", ArgCategory::Mmap, "int*", 512)
        }];
        let m_axi_bases = BTreeSet::from(["mat_a".to_owned()]);

        let xml_ports = build_kernel_xml_ports(&ports, &m_axi_bases);
        assert_eq!(xml_ports.len(), 1);
        assert_eq!(xml_ports[0].name, "mat_a");

        let block = m_axi_param_block(&ports, &m_axi_bases);
        assert_eq!(block.len(), 1);
        assert_eq!(block[0].0, "mat_a");
    }

    #[test]
    fn m_axi_param_block_emits_default_burst_params_for_mmap_only() {
        let ports = vec![
            port("n", ArgCategory::Scalar, "int", 32),
            port("gmem", ArgCategory::Mmap, "int*", 512),
        ];
        let block = m_axi_param_block(&ports, &BTreeSet::new());
        assert_eq!(block.len(), 1);
        assert_eq!(block[0].0, "gmem");
        assert!(block[0].1.iter().any(|(k, v)| k == "HAS_BURST" && v == "0"));
        assert!(block[0]
            .1
            .iter()
            .any(|(k, v)| k == "SUPPORTS_NARROW_BURST" && v == "0"));
    }
}
