//! `kernel.xml` emission for `.xo` packaging.
//!
//! Implements — the
//! element tree and attribute ordering matter for Vivado's
//! `package_xo`, so the emitted text matches the template
//! byte-for-byte (modulo XML-escaping invariants).

use serde::{Deserialize, Serialize};

use crate::error::{Result, XilinxError};

const S_AXI_NAME: &str = "s_axi_control";
const M_AXI_PREFIX: &str = "m_axi_";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum PortCategory {
    Scalar,
    /// Memory-mapped AXI master (MMAP).
    MAxi,
    /// AXI-Stream input (ISTREAM).
    IStream,
    /// AXI-Stream output (OSTREAM).
    OStream,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct KernelXmlPort {
    pub name: String,
    pub category: PortCategory,
    /// Bit width; 32 for a typical `int` scalar, 512 for a wide MMAP
    /// channel, etc.
    pub width: u32,
    /// Optional user-specified port name override. Empty string means
    /// "use `name`" (matches the `arg.port` fallback).
    #[serde(default)]
    pub port: String,
    /// C type string (escaped on emission).
    pub ctype: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct KernelXmlArgs {
    pub top_name: String,
    pub clock_period: String,
    pub ports: Vec<KernelXmlPort>,
}

fn xml_escape(s: &str) -> String {
    quick_xml::escape::escape(s).into_owned()
}

#[derive(Debug, Clone, serde::Serialize)]
struct XmlPort {
    name: String,
    mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    range: Option<String>,
    data_width: u32,
    port_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    base: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
struct XmlArg {
    name: String,
    addr_qualifier: u8,
    id: usize,
    port: String,
    size: String,
    offset: String,
    host_offset: String,
    host_size: String,
    #[serde(rename = "type")]
    arg_type: String,
}

pub fn emit_kernel_xml(args: &KernelXmlArgs) -> Result<String> {
    if args.ports.is_empty() {
        return Err(XilinxError::KernelXml(format!(
            "no ports supplied for kernel `{}`",
            args.top_name
        )));
    }

    let mut ports: Vec<XmlPort> = Vec::new();
    let mut xml_args: Vec<XmlArg> = Vec::new();
    let mut offset: u64 = 0x10;
    let mut has_s_axi_control = false;

    for (arg_id, port) in args.ports.iter().enumerate() {
        let user_port = if port.port.is_empty() {
            None
        } else {
            Some(port.port.as_str())
        };
        let (addr_qualifier, size, host_size, port_name, arg_offset) = match port.category {
            PortCategory::Scalar => {
                has_s_axi_control = true;
                let host_size = u64::from(port.width) / 8;
                let size = host_size.max(4);
                let pname = user_port.unwrap_or(S_AXI_NAME).to_string();
                let off = offset;
                offset += size + 4;
                (0u8, size, host_size, pname, off)
            }
            PortCategory::MAxi => {
                has_s_axi_control = true;
                let size = 8u64;
                let host_size = 8u64;
                let base = user_port.unwrap_or(port.name.as_str());
                ports.push(XmlPort {
                    name: format!("{M_AXI_PREFIX}{base}"),
                    mode: "master".into(),
                    range: Some("0xFFFFFFFFFFFFFFFF".into()),
                    data_width: port.width,
                    port_type: "addressable".into(),
                    base: Some("0x0".into()),
                });
                let pname = format!("{M_AXI_PREFIX}{base}");
                let off = offset;
                offset += size + 4;
                (1u8, size, host_size, pname, off)
            }
            PortCategory::IStream | PortCategory::OStream => {
                let size = 8u64;
                let host_size = 8u64;
                let pname = user_port.unwrap_or(port.name.as_str()).to_string();
                let mode = if matches!(port.category, PortCategory::IStream) {
                    "read_only"
                } else {
                    "write_only"
                };
                ports.push(XmlPort {
                    name: port.name.clone(),
                    mode: mode.into(),
                    range: None,
                    data_width: port.width,
                    port_type: "stream".into(),
                    base: None,
                });
                (4u8, size, host_size, pname, 0u64)
            }
        };
        xml_args.push(XmlArg {
            name: port.name.clone(),
            addr_qualifier,
            id: arg_id,
            port: port_name,
            size: format!("{size:#x}"),
            offset: format!("{arg_offset:#x}"),
            host_offset: "0x0".into(),
            host_size: format!("{host_size:#x}"),
            arg_type: xml_escape(&port.ctype),
        });
    }

    if has_s_axi_control {
        ports.push(XmlPort {
            name: S_AXI_NAME.into(),
            mode: "slave".into(),
            range: Some("0x1000".into()),
            data_width: 32,
            port_type: "addressable".into(),
            base: Some("0x0".into()),
        });
    }

    let hw_ctrl_protocol = if has_s_axi_control {
        "ap_ctrl_hs"
    } else {
        "ap_ctrl_none"
    };

    let mut env = minijinja::Environment::new();
    env.add_template("kernel_xml", include_str!("templates/kernel.xml.j2"))
        .expect("template parses");
    env.get_template("kernel_xml")
        .expect("template exists")
        .render(minijinja::context! {
            name => args.top_name,
            hw_ctrl_protocol,
            ports,
            args => xml_args,
        })
        .map_err(|e| XilinxError::KernelXml(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_ports_rejected() {
        let args = KernelXmlArgs {
            top_name: "k".into(),
            clock_period: "3.33".into(),
            ports: vec![],
        };
        let err = emit_kernel_xml(&args).unwrap_err();
        assert!(matches!(err, XilinxError::KernelXml(_)));
    }

    #[test]
    fn mmap_port_produces_m_axi_prefix() {
        let args = KernelXmlArgs {
            top_name: "k".into(),
            clock_period: "3.33".into(),
            ports: vec![KernelXmlPort {
                name: "a".into(),
                category: PortCategory::MAxi,
                width: 512,
                port: String::new(),
                ctype: "int*".into(),
            }],
        };
        let xml = emit_kernel_xml(&args).unwrap();
        assert!(xml.contains("<port name=\"m_axi_a\""));
        assert!(xml.contains("hwControlProtocol=\"ap_ctrl_hs\""));
        assert!(xml.contains("<port name=\"s_axi_control\""));
        assert!(xml.contains("dataWidth=\"512\""));
    }

    #[test]
    fn streams_emit_axis_port_and_no_s_axi() {
        let args = KernelXmlArgs {
            top_name: "k".into(),
            clock_period: "3.33".into(),
            ports: vec![
                KernelXmlPort {
                    name: "i0".into(),
                    category: PortCategory::IStream,
                    width: 64,
                    port: String::new(),
                    ctype: "tapa::istream<int>".into(),
                },
                KernelXmlPort {
                    name: "o0".into(),
                    category: PortCategory::OStream,
                    width: 64,
                    port: String::new(),
                    ctype: "tapa::ostream<int>".into(),
                },
            ],
        };
        let xml = emit_kernel_xml(&args).unwrap();
        assert!(xml.contains("mode=\"read_only\""));
        assert!(xml.contains("mode=\"write_only\""));
        assert!(xml.contains("hwControlProtocol=\"ap_ctrl_none\""));
        assert!(!xml.contains("s_axi_control"));
    }

    #[test]
    fn ctype_is_xml_escaped() {
        let args = KernelXmlArgs {
            top_name: "k".into(),
            clock_period: "3.33".into(),
            ports: vec![KernelXmlPort {
                name: "x".into(),
                category: PortCategory::Scalar,
                width: 32,
                port: String::new(),
                ctype: "std::vector<int> &".into(),
            }],
        };
        let xml = emit_kernel_xml(&args).unwrap();
        assert!(xml.contains("std::vector&lt;int&gt; &amp;"));
    }

    #[test]
    fn scalar_port_generates_s_axi_control_and_offset() {
        let args = KernelXmlArgs {
            top_name: "k".into(),
            clock_period: "3.33".into(),
            ports: vec![KernelXmlPort {
                name: "n".into(),
                category: PortCategory::Scalar,
                width: 32,
                port: String::new(),
                ctype: "int".into(),
            }],
        };
        let xml = emit_kernel_xml(&args).unwrap();
        assert!(xml.contains("<port name=\"s_axi_control\""));
        assert!(xml.contains("addressQualifier=\"0\""));
        assert!(xml.contains("port=\"s_axi_control\""));
        assert!(xml.contains("hwControlProtocol=\"ap_ctrl_hs\""));
        assert!(xml.contains("size=\"0x4\""));
        assert!(xml.contains("offset=\"0x10\""));
    }

    #[test]
    fn mixed_ports_produce_correct_qualifiers() {
        let args = KernelXmlArgs {
            top_name: "mix".into(),
            clock_period: "5".into(),
            ports: vec![
                KernelXmlPort {
                    name: "a".into(),
                    category: PortCategory::Scalar,
                    width: 64,
                    port: String::new(),
                    ctype: "long".into(),
                },
                KernelXmlPort {
                    name: "b".into(),
                    category: PortCategory::MAxi,
                    width: 512,
                    port: String::new(),
                    ctype: "void*".into(),
                },
                KernelXmlPort {
                    name: "c".into(),
                    category: PortCategory::IStream,
                    width: 32,
                    port: String::new(),
                    ctype: "tapa::istream<int>".into(),
                },
            ],
        };
        let xml = emit_kernel_xml(&args).unwrap();
        assert!(xml.contains("addressQualifier=\"0\""));
        assert!(xml.contains("addressQualifier=\"1\""));
        assert!(xml.contains("addressQualifier=\"4\""));
        assert!(xml.contains("port=\"m_axi_b\""));
        assert!(xml.contains("mode=\"read_only\""));
    }

    #[test]
    fn xml_escape_uses_quick_xml() {
        assert_eq!(xml_escape("&"), "&amp;");
        assert_eq!(xml_escape("<"), "&lt;");
        assert_eq!(xml_escape(">"), "&gt;");
        assert_eq!(xml_escape("\""), "&quot;");
        assert_eq!(xml_escape("a & b"), "a &amp; b");
    }
}
