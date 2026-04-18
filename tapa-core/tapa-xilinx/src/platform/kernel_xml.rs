//! `kernel.xml` emission for `.xo` packaging.
//!
//! Ports `tapa/backend/kernel_metadata.py::print_kernel_xml` — the
//! element tree and attribute ordering matter for Vivado's
//! `package_xo`, so the emitted text matches the Python template
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
    /// "use `name`" (matches the Python `arg.port` fallback).
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
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

const KERNEL_XML_TEMPLATE: &str = r#"
<?xml version="1.0" encoding="UTF-8"?>
<root versionMajor="1" versionMinor="6">
  <kernel name="{name}"           language="ip_c"           vlnv="tapa:xrtl:{name}:1.0"           attributes=""           preferredWorkGroupSizeMultiple="0"           workGroupSize="1"           interrupt="true"           hwControlProtocol="{hw_ctrl_protocol}">
    <ports>{ports}
    </ports>
    <args>{args}
    </args>
  </kernel>
</root>
"#;

fn s_axi_port() -> String {
    format!(
        "\n      <port name=\"{S_AXI_NAME}\"             mode=\"slave\"             range=\"0x1000\"             dataWidth=\"32\"             portType=\"addressable\"             base=\"0x0\"/>"
    )
}

fn m_axi_port(name: &str, width: u32) -> String {
    format!(
        "\n      <port name=\"{M_AXI_PREFIX}{name}\"             mode=\"master\"             range=\"0xFFFFFFFFFFFFFFFF\"             dataWidth=\"{width}\"             portType=\"addressable\"             base=\"0x0\"/>"
    )
}

fn axis_port(name: &str, mode: &str, width: u32) -> String {
    format!(
        "\n      <port name=\"{name}\"             mode=\"{mode}\"             dataWidth=\"{width}\"             portType=\"stream\"/>"
    )
}

fn arg_xml(
    name: &str,
    addr_qualifier: u8,
    arg_id: usize,
    port_name: &str,
    ctype: &str,
    size: u64,
    offset: u64,
    host_size: u64,
) -> String {
    format!(
        "\n      <arg name=\"{name}\"           addressQualifier=\"{addr_qualifier}\"           id=\"{arg_id}\"           port=\"{port_name}\"           size=\"{size:#x}\"           offset=\"{offset:#x}\"           hostOffset=\"0x0\"           hostSize=\"{host_size:#x}\"           type=\"{ctype}\"/>"
    )
}

pub fn emit_kernel_xml(args: &KernelXmlArgs) -> Result<String> {
    if args.ports.is_empty() {
        return Err(XilinxError::KernelXml(format!(
            "no ports supplied for kernel `{}`",
            args.top_name
        )));
    }

    let mut kernel_ports = String::new();
    let mut kernel_args = String::new();
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
                kernel_ports.push_str(&m_axi_port(base, port.width));
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
                kernel_ports.push_str(&axis_port(&port.name, mode, port.width));
                (4u8, size, host_size, pname, 0u64)
            }
        };
        kernel_args.push_str(&arg_xml(
            &port.name,
            addr_qualifier,
            arg_id,
            &port_name,
            &xml_escape(&port.ctype),
            size,
            arg_offset,
            host_size,
        ));
    }

    if has_s_axi_control {
        kernel_ports.push_str(&s_axi_port());
    }

    let hw_ctrl = if has_s_axi_control {
        "ap_ctrl_hs"
    } else {
        "ap_ctrl_none"
    };

    #[allow(
        clippy::literal_string_with_formatting_args,
        reason = "placeholders are template tags, not format-macro args"
    )]
    let result = KERNEL_XML_TEMPLATE
        .replace("{name}", &args.top_name)
        .replace("{hw_ctrl_protocol}", hw_ctrl)
        .replace("{ports}", &kernel_ports)
        .replace("{args}", &kernel_args);
    Ok(result)
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
}
