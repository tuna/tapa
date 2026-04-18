//! `kernel.xml` port projection + `m_axi` bus-parameter block for
//! `tapa pack`.
//!
//! Mirrors the `print_kernel_xml` projection in
//! `tapa/verilog/xilinx/pack.py` plus the `range_or_none` channel
//! fan-out unrolling, and the bus-parameter emission that Python adds
//! for every `m_axi` port.

use tapa_task_graph::port::{ArgCategory, Port};
use tapa_xilinx::{KernelXmlPort, PortCategory};

/// Project a `tapa_task_graph::Port` list into the `KernelXmlPort`
/// shape `tapa_xilinx::emit_kernel_xml` expects. Mirrors the Python
/// `print_kernel_xml` logic in `tapa/verilog/xilinx/pack.py` plus the
/// `range_or_none` channel-fan-out unrolling.
pub(super) fn build_kernel_xml_ports(ports: &[Port]) -> Vec<KernelXmlPort> {
    let mut out = Vec::<KernelXmlPort>::new();
    for port in ports {
        let chan_count = port.chan_count.unwrap_or(0);
        let names: Vec<String> = if chan_count == 0 {
            vec![port.name.clone()]
        } else {
            (0..chan_count)
                .map(|i| format!("{}_{i}", port.name))
                .collect()
        };
        let category = match port.cat {
            ArgCategory::Scalar => Some(PortCategory::Scalar),
            ArgCategory::Mmap | ArgCategory::Immap | ArgCategory::Ommap | ArgCategory::AsyncMmap => {
                Some(PortCategory::MAxi)
            }
            ArgCategory::Istream | ArgCategory::Istreams => Some(PortCategory::IStream),
            ArgCategory::Ostream | ArgCategory::Ostreams => Some(PortCategory::OStream),
        };
        let Some(cat) = category else { continue };
        for name in names {
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

/// Python `pack` adds two bus parameters per `m_axi` port:
/// `HAS_BURST=0`, `SUPPORTS_NARROW_BURST=0`. Mirror that here so the
/// emitted `.xo` matches the Python output.
pub(super) fn m_axi_param_block(ports: &[Port]) -> Vec<(String, Vec<(String, String)>)> {
    let mut out = Vec::<(String, Vec<(String, String)>)>::new();
    let kv = vec![
        ("HAS_BURST".to_string(), "0".to_string()),
        ("SUPPORTS_NARROW_BURST".to_string(), "0".to_string()),
    ];
    for port in ports {
        let is_mmap = matches!(
            port.cat,
            ArgCategory::Mmap | ArgCategory::Immap | ArgCategory::Ommap | ArgCategory::AsyncMmap
        );
        if !is_mmap {
            continue;
        }
        let chan_count = port.chan_count.unwrap_or(0);
        let names: Vec<String> = if chan_count == 0 {
            vec![port.name.clone()]
        } else {
            (0..chan_count)
                .map(|i| format!("{}_{i}", port.name))
                .collect()
        };
        for name in names {
            out.push((name, kv.clone()));
        }
    }
    out
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
