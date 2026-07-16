//! `kernel.xml` port projection + `m_axi` bus-parameter block for
//! `tapa pack`.
//!
//! Projects task ports into kernel XML, expands channel arrays, and
//! emits bus parameters for every `m_axi` port.

use std::collections::BTreeSet;
use tapa_ir::port::{ArgCategory, Port};
use tapa_rtl::module::sanitize_array_name;
use tapa_xilinx::{KernelXmlPort, PortCategory};

/// Project a `tapa_ir::Port` list into the `KernelXmlPort`
/// shape `tapa_xilinx::emit_kernel_xml` expects, including the
/// channel fan-out unrolling for hmap ports.
#[cfg(test)]
pub(super) fn build_kernel_xml_ports(ports: &[Port]) -> Vec<KernelXmlPort> {
    build_kernel_xml_ports_impl(ports, None)
}

pub(super) fn build_kernel_xml_ports_for_rtl(
    ports: &[Port],
    m_axi_bases: &BTreeSet<String>,
) -> Vec<KernelXmlPort> {
    build_kernel_xml_ports_impl(ports, Some(m_axi_bases))
}

fn build_kernel_xml_ports_impl(
    ports: &[Port],
    m_axi_bases: Option<&BTreeSet<String>>,
) -> Vec<KernelXmlPort> {
    let mut out = Vec::<KernelXmlPort>::new();
    for port in ports {
        let category = match port.cat {
            ArgCategory::Scalar => Some(PortCategory::Scalar),
            ArgCategory::Mmap
            | ArgCategory::Immap
            | ArgCategory::Ommap
            | ArgCategory::AsyncMmap => Some(PortCategory::MAxi),
            ArgCategory::Istream | ArgCategory::Istreams => Some(PortCategory::IStream),
            ArgCategory::Ostream | ArgCategory::Ostreams => Some(PortCategory::OStream),
        };
        let Some(cat) = category else { continue };
        for name in projected_port_names(port, m_axi_bases) {
            out.push(KernelXmlPort {
                name,
                category: cat,
                width: port.width,
                port: String::new(),
                ctype: port.ctype.clone(),
            });
        }
    }
    out
}

/// Add `HAS_BURST=0` and `SUPPORTS_NARROW_BURST=0` to each `m_axi`
/// port.
#[cfg(test)]
pub(super) fn m_axi_param_block(ports: &[Port]) -> Vec<(String, Vec<(String, String)>)> {
    m_axi_param_block_impl(ports, None)
}

pub(super) fn m_axi_param_block_for_rtl(
    ports: &[Port],
    m_axi_bases: &BTreeSet<String>,
) -> Vec<(String, Vec<(String, String)>)> {
    m_axi_param_block_impl(ports, Some(m_axi_bases))
}

fn m_axi_param_block_impl(
    ports: &[Port],
    m_axi_bases: Option<&BTreeSet<String>>,
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

fn projected_port_names(port: &Port, m_axi_bases: Option<&BTreeSet<String>>) -> Vec<String> {
    let base = sanitize_array_name(&port.name);
    let chan_count = port.chan_count.unwrap_or(0);
    let default_names: Vec<String> = if chan_count == 0 {
        vec![base.clone()]
    } else {
        (0..chan_count).map(|i| format!("{base}_{i}")).collect()
    };
    if !port.cat.is_mmap_like() {
        return default_names;
    }
    let Some(m_axi_bases) = m_axi_bases else {
        return default_names;
    };
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

    #[test]
    fn build_kernel_xml_ports_translates_categories() {
        let ports = vec![
            Port {
                cat: ArgCategory::Scalar,
                name: "n".into(),
                ctype: "int".into(),
                width: 32,
                chan_count: None,
                chan_size: None,
            },
            Port {
                cat: ArgCategory::Mmap,
                name: "gmem".into(),
                ctype: "int*".into(),
                width: 512,
                chan_count: None,
                chan_size: None,
            },
            Port {
                cat: ArgCategory::Istream,
                name: "i0".into(),
                ctype: "tapa::istream<int>".into(),
                width: 32,
                chan_count: None,
                chan_size: None,
            },
        ];
        let out = build_kernel_xml_ports(&ports);
        assert_eq!(out.len(), 3);
        assert!(matches!(out[0].category, PortCategory::Scalar));
        assert!(matches!(out[1].category, PortCategory::MAxi));
        assert!(matches!(out[2].category, PortCategory::IStream));
    }

    #[test]
    fn build_kernel_xml_ports_unrolls_chan_count() {
        let ports = vec![Port {
            cat: ArgCategory::Mmap,
            name: "gmem".into(),
            ctype: "int*".into(),
            width: 64,
            chan_count: Some(3),
            chan_size: None,
        }];
        let out = build_kernel_xml_ports(&ports);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].name, "gmem_0");
        assert_eq!(out[1].name, "gmem_1");
        assert_eq!(out[2].name, "gmem_2");
    }

    #[test]
    fn build_kernel_xml_ports_sanitizes_indexed_names() {
        let ports = vec![Port {
            cat: ArgCategory::Mmap,
            name: "chan[0]".into(),
            ctype: "int*".into(),
            width: 64,
            chan_count: None,
            chan_size: None,
        }];
        let out = build_kernel_xml_ports(&ports);
        assert_eq!(out[0].name, "chan_0");
        let block = m_axi_param_block(&ports);
        assert_eq!(block[0].0, "chan_0");
    }

    #[test]
    fn rtl_m_axi_bases_override_chan_count_unrolling() {
        let ports = vec![Port {
            cat: ArgCategory::Mmap,
            name: "mat_a".into(),
            ctype: "int*".into(),
            width: 512,
            chan_count: Some(2),
            chan_size: None,
        }];
        let m_axi_bases = BTreeSet::from(["mat_a".to_owned()]);

        let xml_ports = build_kernel_xml_ports_for_rtl(&ports, &m_axi_bases);
        assert_eq!(xml_ports.len(), 1);
        assert_eq!(xml_ports[0].name, "mat_a");

        let block = m_axi_param_block_for_rtl(&ports, &m_axi_bases);
        assert_eq!(block.len(), 1);
        assert_eq!(block[0].0, "mat_a");
    }

    #[test]
    fn m_axi_param_block_emits_default_burst_params_for_mmap_only() {
        let ports = vec![
            Port {
                cat: ArgCategory::Scalar,
                name: "n".into(),
                ctype: "int".into(),
                width: 32,
                chan_count: None,
                chan_size: None,
            },
            Port {
                cat: ArgCategory::Mmap,
                name: "gmem".into(),
                ctype: "int*".into(),
                width: 512,
                chan_count: None,
                chan_size: None,
            },
        ];
        let block = m_axi_param_block(&ports);
        assert_eq!(block.len(), 1);
        assert_eq!(block[0].0, "gmem");
        assert!(block[0].1.iter().any(|(k, v)| k == "HAS_BURST" && v == "0"));
        assert!(block[0]
            .1
            .iter()
            .any(|(k, v)| k == "SUPPORTS_NARROW_BURST" && v == "0"));
    }
}
