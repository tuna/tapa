//! Plain/gzip vbin metadata reading and in-place clock derating in Rust.

use std::io::{Read, Write};
use std::path::Path;

use camino::Utf8Path;
use flate2::Compression;
use quick_xml::events::{BytesText, Event};
use serde::Deserialize;

use crate::error::{Result, XilinxError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SystemMap {
    pub platform: String,
    pub clock_hz: u64,
}

pub(super) fn read_system_map(path: &Utf8Path) -> Result<SystemMap> {
    if is_gzip(path)? {
        system_map_from_tar(flate2::read::GzDecoder::new(fs_err::File::open(path)?))
    } else {
        system_map_from_tar(fs_err::File::open(path)?)
    }
}

fn system_map_from_tar(reader: impl Read) -> Result<SystemMap> {
    let mut archive = tar::Archive::new(reader);
    let mut xml = None;
    for entry in archive.entries()? {
        let mut entry = entry?;
        if entry.path()? == Path::new("system_map.xml") {
            if xml.is_some() {
                return Err(slash_error(
                    "vbin contains duplicate system_map.xml members",
                ));
            }
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes)?;
            xml = Some(bytes);
        }
    }
    let xml = xml.ok_or_else(|| slash_error("vbin has no system_map.xml member"))?;
    let parsed: SystemMapXml = quick_xml::de::from_reader(xml.as_slice())
        .map_err(|error| slash_error(format!("cannot parse vbin system_map.xml: {error}")))?;
    if parsed.clock_frequency == 0 {
        return Err(slash_error("vbin ClockFrequency must be positive"));
    }
    Ok(SystemMap {
        platform: parsed.platform,
        clock_hz: parsed.clock_frequency,
    })
}

#[derive(Debug, Deserialize)]
struct SystemMapXml {
    #[serde(rename = "Platform")]
    platform: String,
    #[serde(rename = "ClockFrequency")]
    clock_frequency: u64,
}

/// Rewrite only ClockFrequency, preserving every other archive member and
/// whether the outer tar was plain or gzip-compressed.
pub(super) fn rewrite_clock(path: &Utf8Path, recorded_hz: u64) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| slash_error(format!("vbin path `{path}` has no parent")))?;
    let permissions = fs_err::metadata(path)?.permissions();
    let temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.as_file().set_permissions(permissions)?;

    if is_gzip(path)? {
        let reader = flate2::read::GzDecoder::new(fs_err::File::open(path)?);
        let writer = flate2::write::GzEncoder::new(temporary.reopen()?, Compression::default());
        let encoder = rewrite_archive(reader, writer, recorded_hz)?;
        let output = encoder.finish()?;
        output.sync_all()?;
    } else {
        let reader = fs_err::File::open(path)?;
        let writer = temporary.reopen()?;
        let output = rewrite_archive(reader, writer, recorded_hz)?;
        output.sync_all()?;
    }
    temporary.persist(path).map_err(|error| error.error)?;
    Ok(())
}

fn rewrite_archive<R: Read, W: Write>(reader: R, writer: W, recorded_hz: u64) -> Result<W> {
    let mut archive = tar::Archive::new(reader);
    let mut builder = tar::Builder::new(writer);
    let mut maps = 0_u32;
    for entry in archive.entries()? {
        let mut entry = entry?;
        let mut header = entry.header().clone();
        if entry.path()? == Path::new("system_map.xml") {
            maps += 1;
            let mut xml = Vec::new();
            entry.read_to_end(&mut xml)?;
            let xml = rewrite_system_map(&xml, recorded_hz)?;
            header.set_size(xml.len() as u64);
            header.set_cksum();
            builder.append(&header, xml.as_slice())?;
        } else {
            builder.append(&header, &mut entry)?;
        }
    }
    if maps != 1 {
        return Err(slash_error(format!(
            "expected one vbin system_map.xml while rewriting, found {maps}"
        )));
    }
    Ok(builder.into_inner()?)
}

fn rewrite_system_map(xml: &[u8], recorded_hz: u64) -> Result<Vec<u8>> {
    let text = std::str::from_utf8(xml)
        .map_err(|error| slash_error(format!("vbin system_map.xml is not UTF-8: {error}")))?;
    // Validate before changing the archive so malformed metadata cannot be
    // hidden by the event-level rewrite below.
    let _: SystemMapXml = quick_xml::de::from_str(text)
        .map_err(|error| slash_error(format!("cannot parse vbin system_map.xml: {error}")))?;

    let mut reader = quick_xml::Reader::from_str(text);
    reader.config_mut().trim_text(false);
    let mut writer = quick_xml::Writer::new(Vec::new());
    let mut in_clock = false;
    loop {
        let event = reader.read_event()?;
        match event {
            Event::Eof => break,
            Event::Start(ref start) if start.name().as_ref() == "ClockFrequency" => {
                in_clock = true;
                writer.write_event(event)?;
            }
            Event::Text(_) if in_clock => {
                writer.write_event(Event::Text(BytesText::new(&recorded_hz.to_string())))?;
            }
            Event::End(ref end) if end.name().as_ref() == "ClockFrequency" => {
                in_clock = false;
                writer.write_event(event)?;
            }
            _ => writer.write_event(event)?,
        }
    }
    Ok(writer.into_inner())
}

fn is_gzip(path: &Utf8Path) -> Result<bool> {
    let mut file = fs_err::File::open(path)?;
    let mut magic = [0_u8; 2];
    let count = file.read(&mut magic)?;
    Ok(count == magic.len() && magic == [0x1f, 0x8b])
}

fn slash_error(detail: impl Into<String>) -> XilinxError {
    XilinxError::SlashLink(detail.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use camino::Utf8PathBuf;
    use std::path::PathBuf;

    fn fixture(name: &str) -> Utf8PathBuf {
        Utf8PathBuf::from(format!(
            "{}/testdata/slash/{name}",
            env!("CARGO_MANIFEST_DIR")
        ))
    }

    #[test]
    fn accepts_real_plain_and_gzip_vbins() {
        for name in ["vadd-plain.vbin", "vadd-gzip.vbin"] {
            let map = read_system_map(&fixture(name)).expect("read real-shaped vbin");
            assert_eq!(map.platform, "Hardware");
            assert_eq!(map.clock_hz, 300_000_000);
        }
    }

    #[test]
    fn rewrites_only_clock_and_preserves_compression_and_members() {
        for name in ["vadd-plain.vbin", "vadd-gzip.vbin"] {
            let temp = tempfile::tempdir().expect("tempdir");
            let path = crate::util::utf8(temp.path().join(name));
            fs_err::copy(fixture(name), &path).expect("copy vbin fixture");
            let compressed = is_gzip(&path).expect("detect input compression");
            rewrite_clock(&path, 210_526_315).expect("rewrite clock");
            assert_eq!(
                is_gzip(&path).expect("detect output compression"),
                compressed
            );
            assert_eq!(
                read_system_map(&path).expect("read rewritten map").clock_hz,
                210_526_315,
            );
            let reader: Box<dyn Read> = if compressed {
                Box::new(flate2::read::GzDecoder::new(
                    fs_err::File::open(&path).expect("open gzip"),
                ))
            } else {
                Box::new(fs_err::File::open(&path).expect("open tar"))
            };
            let mut archive = tar::Archive::new(reader);
            let names = archive
                .entries()
                .expect("entries")
                .map(|entry| entry.expect("entry").path().expect("path").into_owned())
                .collect::<Vec<_>>();
            assert!(names.iter().any(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.ends_with("_partial.pdi"))
            }));
            assert!(names.contains(&PathBuf::from("report_utilization_vadd_hw.xml")));
        }
    }

    #[test]
    fn clock_rewrite_does_not_relocate_kernel() {
        let xml = fs_err::read(fixture("vadd-system_map.xml")).expect("read system map");
        let rewritten = rewrite_system_map(&xml, 200_000_000).expect("rewrite XML");
        let text = String::from_utf8(rewritten).expect("UTF-8");
        assert!(text.contains("<BaseAddress>0x20200000000</BaseAddress>"));
        assert!(!text.contains("020200010000"));
        assert!(text.contains("<ClockFrequency>200000000</ClockFrequency>"));
    }
}
