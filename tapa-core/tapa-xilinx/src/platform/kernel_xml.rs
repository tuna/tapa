//! `kernel.xml` emission for `.xo` packaging.
//!
//! The element tree and attribute ordering matter for Vivado's
//! `package_xo`, so the emitted text follows the `kernel.xml.j2`
//! template byte-for-byte (modulo XML-escaping invariants).

use serde::{Deserialize, Serialize};
use tapa_protocol::{axis_data_width, M_AXI_PREFIX, S_AXI_NAME};

use crate::error::{Result, XilinxError};

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
    /// channel, etc. Stream payload widths are rounded to whole bytes when
    /// emitting the AXIS dataWidth attribute.
    pub width: u32,
    /// Optional user-specified port name override. Empty string means
    /// "use `name`".
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

#[derive(Debug, Clone)]
struct XmlPort {
    name: String,
    mode: String,
    range: Option<String>,
    data_width: u32,
    port_type: String,
    base: Option<String>,
}

#[derive(Debug, Clone)]
struct XmlArg {
    name: String,
    addr_qualifier: u8,
    id: usize,
    port: String,
    size: String,
    offset: String,
    host_offset: String,
    host_size: String,
    arg_type: String,
    size_bytes: u64,
    offset_bytes: u64,
    host_size_bytes: u64,
}

#[allow(
    clippy::too_many_lines,
    reason = "kernel.xml emission is inherently sequential; \
              splitting would fragment the element tree construction"
)]
fn project_kernel_xml(args: &KernelXmlArgs) -> Result<(Vec<XmlPort>, Vec<XmlArg>)> {
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
                    data_width: axis_data_width(port.width),
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
            size_bytes: size,
            offset_bytes: arg_offset,
            host_size_bytes: host_size,
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

    Ok((ports, xml_args))
}

pub fn emit_kernel_xml(args: &KernelXmlArgs) -> Result<String> {
    #[derive(askama::Template)]
    #[template(path = "kernel.xml.j2", escape = "none")]
    struct Template<'a> {
        name: &'a str,
        hw_ctrl_protocol: &'a str,
        ports: &'a [XmlPort],
        args: &'a [XmlArg],
    }
    let (ports, xml_args) = project_kernel_xml(args)?;
    let hw_ctrl_protocol = if xml_args.iter().any(|arg| arg.addr_qualifier != 4) {
        "ap_ctrl_hs"
    } else {
        "ap_ctrl_none"
    };

    askama::Template::render(&Template {
        name: &args.top_name,
        hw_ctrl_protocol,
        ports: &ports,
        args: &xml_args,
    })
    .map_err(|e| XilinxError::KernelXml(e.to_string()))
}

struct SlashAbi {
    ports: Vec<XmlPort>,
    args: Vec<XmlArg>,
}

fn slash_abi(args: &KernelXmlArgs) -> Result<SlashAbi> {
    let (ports, xml_args) = project_kernel_xml(args)?;
    if xml_args.is_empty()
        || xml_args
            .iter()
            .any(|arg| !matches!(arg.addr_qualifier, 0 | 1) || !matches!(arg.size_bytes, 4 | 8))
    {
        return Err(XilinxError::KernelXml(
            "SLASH packaging requires an ap_ctrl_hs kernel with only 32- or 64-bit \
             scalar and mmap arguments"
                .into(),
        ));
    }
    Ok(SlashAbi {
        ports,
        args: xml_args,
    })
}

/// Render the SLASH IP-XACT register map from the kernel.xml ABI projection.
pub fn render_slash_registers(args: &KernelXmlArgs) -> Result<String> {
    let abi = slash_abi(args)?;
    let mut registers = vec![
        ("CTRL".to_string(), 0),
        ("GIER".to_string(), 4),
        ("IP_IER".to_string(), 8),
        ("IP_ISR".to_string(), 12),
    ];
    for arg in &abi.args {
        for word in 0..arg.size_bytes / 4 {
            registers.push((format!("{}_{word}", arg.name), arg.offset_bytes + word * 4));
        }
    }
    let entries = registers
        .iter()
        .map(|(name, offset)| format!("{{{name} {offset:#x}}}"))
        .collect::<Vec<_>>()
        .join(" ");
    Ok(format!(
        "set tapa_control_block [ipx::get_address_blocks reg0 -of_objects \
         [ipx::get_memory_maps s_axi_control -of_objects [ipx::current_core]]]\n\
         foreach {{tapa_register_name tapa_register_offset}} {{{entries}}} {{\n\
         \x20 set tapa_register [ipx::add_register $tapa_register_name $tapa_control_block]\n\
         \x20 set_property address_offset $tapa_register_offset $tapa_register\n\
         \x20 set_property size 32 $tapa_register\n\
         \x20 set_property access read-write $tapa_register\n\
         }}\n"
    ))
}

/// Emit slashkit's argument metadata from the same ABI as kernel.xml.
pub fn emit_slash_hls_data(args: &KernelXmlArgs) -> Result<String> {
    let abi = slash_abi(args)?;
    let interface_ports: std::collections::BTreeMap<&str, &XmlPort> = abi
        .ports
        .iter()
        .map(|port| (port.name.as_str(), port))
        .collect();
    let mut metadata = serde_json::Map::new();
    for (arg, source) in abi.args.iter().zip(&args.ports) {
        let mut refs = Vec::new();
        for word in 0..arg.size_bytes / 4 {
            refs.push(serde_json::json!({
                "type": "register",
                "interface": "s_axi_control",
                "name": format!("{}_{word}", arg.name),
                "usage": if arg.addr_qualifier == 1 { "address" } else { "data" },
            }));
        }
        if arg.addr_qualifier == 1 {
            if !interface_ports.contains_key(arg.port.as_str()) {
                return Err(XilinxError::KernelXml(format!(
                    "SLASH argument `{}` refers to missing interface `{}`",
                    arg.name, arg.port
                )));
            }
            refs.push(serde_json::json!({
                "type": "interface",
                "interface": arg.port,
            }));
        }
        metadata.insert(
            arg.name.clone(),
            serde_json::json!({
                "index": arg.id,
                "srcType": source.ctype,
                "srcSize": arg.host_size_bytes * 8,
                "hwRefs": refs,
            }),
        );
    }
    let mut json = serde_json::to_string_pretty(&serde_json::json!({ "Args": metadata }))?;
    json.push('\n');
    Ok(json)
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
                    width: 17,
                    port: String::new(),
                    ctype: "tapa::istream<int>".into(),
                },
                KernelXmlPort {
                    name: "o0".into(),
                    category: PortCategory::OStream,
                    width: 544,
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
        assert!(xml.contains("dataWidth=\"24\""));
        assert!(xml.contains("dataWidth=\"544\""));
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

    fn slash_test_args() -> KernelXmlArgs {
        KernelXmlArgs {
            top_name: "vadd".into(),
            clock_period: "3.33".into(),
            ports: vec![
                KernelXmlPort {
                    name: "a".into(),
                    category: PortCategory::MAxi,
                    width: 64,
                    port: String::new(),
                    ctype: "const int*".into(),
                },
                KernelXmlPort {
                    name: "n".into(),
                    category: PortCategory::Scalar,
                    width: 64,
                    port: String::new(),
                    ctype: "uint64_t".into(),
                },
            ],
        }
    }

    #[test]
    fn slash_registers_and_hls_data_share_the_kernel_xml_abi() {
        let args = slash_test_args();
        let xml = emit_kernel_xml(&args).unwrap();
        assert!(xml.contains("name=\"a\" addressQualifier=\"1\" id=\"0\""));
        assert!(xml.contains("size=\"0x8\" offset=\"0x10\""));
        assert!(xml.contains("name=\"n\" addressQualifier=\"0\" id=\"1\""));
        assert!(xml.contains("size=\"0x8\" offset=\"0x1c\""));

        let registers = render_slash_registers(&args).unwrap();
        for entry in [
            "{CTRL 0x0}",
            "{GIER 0x4}",
            "{IP_IER 0x8}",
            "{IP_ISR 0xc}",
            "{a_0 0x10}",
            "{a_1 0x14}",
            "{n_0 0x1c}",
            "{n_1 0x20}",
        ] {
            assert!(registers.contains(entry), "missing {entry}: {registers}");
        }
        assert!(registers.contains("ipx::add_register"));
        assert!(registers.contains("set_property size 32"));

        let data: serde_json::Value =
            serde_json::from_str(&emit_slash_hls_data(&args).unwrap()).unwrap();
        assert_eq!(data["Args"]["a"]["index"], 0);
        assert_eq!(data["Args"]["a"]["srcType"], "const int*");
        assert_eq!(data["Args"]["a"]["srcSize"], 64);
        assert_eq!(
            data["Args"]["a"]["hwRefs"],
            serde_json::json!([
                {"type":"register", "interface":"s_axi_control", "name":"a_0", "usage":"address"},
                {"type":"register", "interface":"s_axi_control", "name":"a_1", "usage":"address"},
                {"type":"interface", "interface":"m_axi_a"}
            ])
        );
        assert_eq!(data["Args"]["n"]["index"], 1);
        assert_eq!(data["Args"]["n"]["srcType"], "uint64_t");
        assert_eq!(data["Args"]["n"]["srcSize"], 64);
        assert_eq!(
            data["Args"]["n"]["hwRefs"],
            serde_json::json!([
                {"type":"register", "interface":"s_axi_control", "name":"n_0", "usage":"data"},
                {"type":"register", "interface":"s_axi_control", "name":"n_1", "usage":"data"}
            ])
        );
    }
}
