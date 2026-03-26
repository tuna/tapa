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

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => match e.name().as_ref() {
                b"kernel" => {
                    for attr in e.attributes().flatten() {
                        if attr.key.as_ref() == b"name" {
                            top_name = String::from_utf8_lossy(&attr.value).into_owned();
                        }
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
                        1 => ArgKind::Mmap {
                            data_width,
                            addr_width,
                        },
                        4 => {
                            let dir = if port.starts_with("s_axis") || port.contains("istream") {
                                StreamDir::In
                            } else {
                                StreamDir::Out
                            };
                            ArgKind::Stream {
                                width: data_width,
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
        scalar_register_map: HashMap::new(),
    })
}
