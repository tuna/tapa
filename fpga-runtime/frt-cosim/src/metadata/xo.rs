use super::{ArgKind, ArgSpec, KernelSpec, Mode, StreamDir, StreamProtocol};
use crate::error::{CosimError, Result};
use quick_xml::events::Event;
use quick_xml::Reader;
use std::collections::HashMap;
use std::path::Path;

pub fn parse_kernel_xml(xml: &str, _verilog_dir: &Path) -> Result<KernelSpec> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut top_name = String::new();
    let mut args = Vec::new();
    let mut buf = Vec::new();
    // Map from port name to (mode, dataWidth) parsed from <port> elements.
    // For TAPA-generated kernel.xml, <ports> precedes <args>, so this is
    // populated before we process any <arg> elements in a single SAX pass.
    let mut port_info: HashMap<String, (String, u32)> = HashMap::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e) | Event::Empty(e)) => match e.name().as_ref() {
                b"kernel" => {
                    for attr in e.attributes().flatten() {
                        if attr.key.as_ref() == b"name" {
                            top_name = String::from_utf8_lossy(&attr.value).into_owned();
                        }
                    }
                }
                b"port" => {
                    let mut port_name = String::new();
                    let mut mode = String::new();
                    let mut data_width = 32u32;
                    for attr in e.attributes().flatten() {
                        let v = String::from_utf8_lossy(&attr.value).into_owned();
                        match attr.key.as_ref() {
                            b"name" => port_name = v,
                            b"mode" => mode = v,
                            b"dataWidth" => data_width = v.parse().unwrap_or(32),
                            _ => {}
                        }
                    }
                    if !port_name.is_empty() {
                        port_info.insert(port_name, (mode, data_width));
                    }
                }
                b"arg" => {
                    let mut name = String::new();
                    let mut id = 0u32;
                    let mut qualifier = 0u32;
                    let mut data_width = 32u32;
                    let mut addr_width = 64u32;
                    let mut depth = 16u32;
                    let mut port = String::new();
                    let mut scalar_width = None;

                    for attr in e.attributes().flatten() {
                        let v = String::from_utf8_lossy(&attr.value).into_owned();
                        match attr.key.as_ref() {
                            b"name" => name = v,
                            b"id" => id = v.parse().unwrap_or(0),
                            b"addressQualifier" => qualifier = v.parse().unwrap_or(0),
                            b"dataWidth" => data_width = v.parse().unwrap_or(32),
                            b"addrWidth" => addr_width = v.parse().unwrap_or(64),
                            b"depth" => depth = v.parse().unwrap_or(16),
                            b"port" => port = v,
                            b"width" => scalar_width = Some(v.parse().unwrap_or(32)),
                            _ => {}
                        }
                    }

                    let kind = match qualifier {
                        0 => ArgKind::Scalar {
                            width: scalar_width.unwrap_or(data_width),
                        },
                        1 => {
                            // Use dataWidth from the corresponding <port> element when
                            // available.  TAPA's kernel_metadata.py does not emit
                            // dataWidth on <arg> for mmap ports; it only appears on
                            // <port name="m_axi_<name>">.
                            let resolved_width =
                                port_info.get(&port).map_or(data_width, |(_, w)| *w);
                            ArgKind::Mmap {
                                data_width: resolved_width,
                                addr_width,
                            }
                        }
                        4 => {
                            // Determine direction from the <port mode="read_only|write_only">
                            // attribute.  TAPA sets port names to the bare arg name (e.g. "a"),
                            // so the old s_axis/istream heuristic fails; fall back to it only
                            // when no <port> entry is found (non-TAPA XO files).
                            let dir = if let Some((mode, _)) = port_info.get(&port) {
                                match mode.as_str() {
                                    "read_only" => StreamDir::In,
                                    "write_only" => StreamDir::Out,
                                    _ => fallback_stream_dir(&port),
                                }
                            } else {
                                fallback_stream_dir(&port)
                            };
                            // Use dataWidth from <port> when available.
                            let resolved_width =
                                port_info.get(&port).map_or(data_width, |(_, w)| *w);
                            ArgKind::Stream {
                                width: resolved_width,
                                depth,
                                dir,
                                protocol: StreamProtocol::Axis,
                            }
                        }
                        q => {
                            return Err(CosimError::Metadata(format!(
                                "unknown addressQualifier {q} for arg {name}"
                            )));
                        }
                    };
                    args.push(ArgSpec { name, id, kind });
                }
                _ => {}
            },
            Ok(Event::Eof) => break,
            Err(e) => return Err(CosimError::Metadata(e.to_string())),
            _ => {}
        }
        buf.clear();
    }

    if top_name.is_empty() {
        return Err(CosimError::Metadata(
            "no kernel name found in kernel.xml".into(),
        ));
    }

    Ok(KernelSpec {
        top_name,
        mode: Mode::Vitis,
        args,
        part_num: None,
        verilog_files: vec![],
        tcl_files: vec![],
        xci_files: vec![],
        scalar_register_map: HashMap::new(),
    })
}

/// Heuristic stream direction for non-TAPA XO files that do not have a
/// <port mode="read_only|write_only"> element (e.g. hand-crafted kernels).
fn fallback_stream_dir(port: &str) -> StreamDir {
    if port.starts_with("s_axis") || port.contains("istream") {
        StreamDir::In
    } else {
        StreamDir::Out
    }
}
