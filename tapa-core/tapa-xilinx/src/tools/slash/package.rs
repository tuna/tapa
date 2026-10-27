//! K5 package extraction and typed kernel-interface recovery.

use std::io::Read as _;

use camino::Utf8Path;
use serde::Deserialize;
use tapa_protocol::M_AXI_PREFIX;

use crate::error::{Result, XilinxError};
use crate::platform::kernel_xml::{KernelXmlPort, PortCategory};

#[derive(Debug)]
pub(super) struct PackageMetadata {
    pub top_name: String,
    pub ports: Vec<KernelXmlPort>,
}

/// Extract `impl/ip` and the metadata location slashkit infers from the
/// extracted component path. Only the package layout is interpreted here;
/// packaging and register-map generation belong to K5.
pub(super) fn extract_package(
    package: &Utf8Path,
    ip_dir: &Utf8Path,
    hls_data_path: &Utf8Path,
) -> Result<PackageMetadata> {
    if ip_dir.exists() {
        return Err(slash_error(format!(
            "SLASH IP directory `{ip_dir}` already exists; use a fresh link work directory"
        )));
    }
    fs_err::create_dir_all(ip_dir)?;
    let file = fs_err::File::open(package)?;
    let mut archive = zip::ZipArchive::new(file)?;
    let mut kernel_xml = None;
    let mut hls_data = None;

    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let name = entry.name()?.to_string();
        match name.as_str() {
            "kernel.xml" => {
                if kernel_xml.is_some() {
                    return Err(slash_error("SLASH package contains duplicate kernel.xml"));
                }
                let mut bytes = Vec::new();
                entry.read_to_end(&mut bytes)?;
                kernel_xml = Some(bytes);
            }
            "hls_data.json" => {
                if hls_data.is_some() {
                    return Err(slash_error(
                        "SLASH package contains duplicate hls_data.json",
                    ));
                }
                let mut bytes = Vec::new();
                entry.read_to_end(&mut bytes)?;
                hls_data = Some(bytes);
            }
            _ if name.starts_with("impl/ip/") && !entry.is_dir() => {
                let enclosed = entry
                    .enclosed_name()
                    .ok_or_else(|| slash_error(format!("unsafe SLASH package entry `{name}`")))?;
                let relative = enclosed
                    .strip_prefix("impl/ip")
                    .map_err(|_| slash_error(format!("unsafe SLASH IP entry `{name}`")))?;
                let relative = Utf8Path::from_path(relative)
                    .ok_or_else(|| slash_error(format!("non-UTF-8 SLASH IP entry `{name}`")))?;
                let destination = ip_dir.join(relative);
                if let Some(parent) = destination.parent() {
                    fs_err::create_dir_all(parent)?;
                }
                let mut output = fs_err::File::create(destination)?;
                std::io::copy(&mut entry, &mut output)?;
            }
            _ => {}
        }
    }

    let kernel_xml = kernel_xml
        .ok_or_else(|| slash_error(format!("SLASH package `{package}` has no root kernel.xml")))?;
    let hls_data = hls_data.ok_or_else(|| {
        slash_error(format!(
            "SLASH package `{package}` has no root hls_data.json"
        ))
    })?;
    if !ip_dir.join("component.xml").is_file() {
        return Err(slash_error(format!(
            "SLASH package `{package}` has no impl/ip/component.xml"
        )));
    }
    if let Some(parent) = hls_data_path.parent() {
        fs_err::create_dir_all(parent)?;
    }
    fs_err::write(hls_data_path, hls_data)?;

    let metadata = parse_kernel_xml(
        std::str::from_utf8(&kernel_xml)
            .map_err(|error| slash_error(format!("SLASH kernel.xml is not UTF-8: {error}")))?,
    )?;
    Ok(metadata)
}

fn parse_kernel_xml(xml: &str) -> Result<PackageMetadata> {
    let document: KernelDocument = quick_xml::de::from_str(xml)
        .map_err(|error| slash_error(format!("cannot parse SLASH kernel.xml: {error}")))?;
    let kernel = document.kernel;
    if kernel.hw_control_protocol != "ap_ctrl_hs" {
        return Err(slash_error(format!(
            "SLASH kernel `{}` must use ap_ctrl_hs",
            kernel.name
        )));
    }
    let mut ports = Vec::new();
    for arg in kernel.args.args {
        if arg.address_qualifier != 1 {
            continue;
        }
        let physical = kernel
            .ports
            .ports
            .iter()
            .find(|port| port.name == arg.port && port.mode == "master")
            .ok_or_else(|| {
                slash_error(format!(
                    "SLASH kernel argument `{}` has no master port `{}`",
                    arg.name, arg.port
                ))
            })?;
        let base = physical.name.strip_prefix(M_AXI_PREFIX).ok_or_else(|| {
            slash_error(format!(
                "SLASH M-AXI port `{}` lacks the m_axi_ prefix",
                physical.name
            ))
        })?;
        ports.push(KernelXmlPort {
            name: arg.name,
            category: PortCategory::MAxi,
            width: physical.data_width,
            port: base.to_string(),
            ctype: arg.ctype,
        });
    }
    Ok(PackageMetadata {
        top_name: kernel.name,
        ports,
    })
}

#[derive(Debug, Deserialize)]
struct KernelDocument {
    kernel: KernelMetadata,
}

#[derive(Debug, Deserialize)]
struct KernelMetadata {
    #[serde(rename = "@name")]
    name: String,
    #[serde(rename = "@hwControlProtocol")]
    hw_control_protocol: String,
    ports: PortList,
    args: ArgList,
}

#[derive(Debug, Deserialize)]
struct PortList {
    #[serde(rename = "port", default)]
    ports: Vec<PhysicalPort>,
}

#[derive(Debug, Deserialize)]
struct PhysicalPort {
    #[serde(rename = "@name")]
    name: String,
    #[serde(rename = "@mode")]
    mode: String,
    #[serde(rename = "@dataWidth")]
    data_width: u32,
}

#[derive(Debug, Deserialize)]
struct ArgList {
    #[serde(rename = "arg", default)]
    args: Vec<KernelArgument>,
}

#[derive(Debug, Deserialize)]
struct KernelArgument {
    #[serde(rename = "@name")]
    name: String,
    #[serde(rename = "@addressQualifier")]
    address_qualifier: u8,
    #[serde(rename = "@port")]
    port: String,
    #[serde(rename = "@type")]
    ctype: String,
}

fn slash_error(detail: impl Into<String>) -> XilinxError {
    XilinxError::SlashLink(detail.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use camino::Utf8PathBuf;

    #[test]
    fn extracts_real_k5_layout_and_recovers_typed_ports() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = crate::util::utf8(temp.path());
        let work = root.join("work").join("link");
        let metadata = extract_package(
            &Utf8PathBuf::from(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/testdata/slash/vadd-package.zip"
            )),
            &work.join("ip"),
            &root.join("work").join("hls_data.json"),
        )
        .expect("extract K5 package");
        assert_eq!(metadata.top_name, "VecAdd");
        assert_eq!(metadata.ports.len(), 3);
        assert_eq!(
            metadata.ports[0].compiled_m_axi_name().as_deref(),
            Some("m_axi_a")
        );
        assert!(work.join("ip").join("component.xml").is_file());
        assert!(work.join("ip").join("VecAdd.v").is_file());
        assert!(root.join("work").join("hls_data.json").is_file());

        let config = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/slash/vadd-config.cfg"
        ));
        crate::connectivity::prepare_slash_config(
            config,
            &metadata.top_name,
            &metadata.ports,
            300_000_000,
        )
        .expect("real config matches real typed ports");
    }
}
