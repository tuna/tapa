//! `.xo` packaging: kernel.xml + Vivado `package_xo` + ZIP redaction.
//!
//! Ports `tapa/backend/xilinx_tools.py::PackageXo`,
//! `tapa/verilog/xilinx/pack.py`, and `tapa/program/pack.py` redaction.
//! The kernel.xml emission is delegated to
//! `platform::kernel_xml::emit_kernel_xml`; the full Vivado-backed
//! build lands alongside `run_vivado`.

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;

use zip::write::SimpleFileOptions;

use crate::error::{Result, XilinxError};
use crate::platform::device::DeviceInfo;
use crate::platform::kernel_xml::{emit_kernel_xml, KernelXmlArgs};
use crate::runtime::process::ToolRunner;

#[derive(Debug, Clone)]
pub struct PackageXoInputs {
    pub top_name: String,
    pub verilog_files: Vec<PathBuf>,
    pub device_info: DeviceInfo,
    pub clock_period: String,
    pub kernel_xml: KernelXmlArgs,
    pub kernel_out_path: PathBuf,
}

/// Build the `.xo` for the given inputs using the provided runner.
///
/// The Vivado-backed implementation emits kernel.xml, invokes
/// `package_xo` via TCL, and then redacts the produced ZIP. Only the
/// redaction and kernel-xml emission are implemented in this revision;
/// the TCL runner wire-up lands with the Vivado milestone.
pub fn pack_xo(_runner: &dyn ToolRunner, inputs: &PackageXoInputs) -> Result<PathBuf> {
    if inputs.verilog_files.is_empty() {
        return Err(XilinxError::KernelXml(
            "pack_xo called with empty verilog_files list".into(),
        ));
    }
    // Ensure kernel.xml would emit successfully before calling Vivado.
    let _xml = emit_kernel_xml(&inputs.kernel_xml)?;
    Err(XilinxError::XoRedaction(
        "pack_xo Vivado backend not yet implemented".into(),
    ))
}

fn redact_rpt(text: &str) -> String {
    // Matches Python's `Date:           <Day Mon DD HH:MM:SS YYYY>` pattern.
    let re = regex::Regex::new("Date:           ... ... .. ..:..:.. ....")
        .expect("static regex compiles");
    re.replace_all(text, "Date:           Tue Jan 01 00:00:00 1980")
        .into_owned()
}

fn redact_xml_payload(text: &str) -> String {
    let re_time = regex::Regex::new(
        "<xilinx:coreCreationDateTime>....-..-..T..:..:..Z</xilinx:coreCreationDateTime>",
    )
    .expect("static regex compiles");
    let step1 = re_time.replace_all(
        text,
        "<xilinx:coreCreationDateTime>1980-01-01T00:00:00Z</xilinx:coreCreationDateTime>",
    );

    let re_src = regex::Regex::new("<SourceLocation>.*/(cpp/[^<]*)</SourceLocation>")
        .expect("static regex compiles");
    let step2 = re_src.replace_all(&step1, "<SourceLocation>$1</SourceLocation>");

    let re_pid = regex::Regex::new("ProjectID=\"[0-9a-fA-F]{32}\"")
        .expect("static regex compiles");
    re_pid
        .replace_all(&step2, "ProjectID=\"0123456789abcdef0123456789abcdef\"")
        .into_owned()
}

/// Rewrite a `.xo` ZIP in place so two invocations on the same inputs
/// produce semantically-equal outputs.
///
/// Matches the Python `_redact_and_zip` / `_redact_rpt` /
/// `_redact_xml` triple in `tapa/program/pack.py`:
///
///   - ZIP timestamps are zeroed to the MS-DOS epoch.
///   - `*.rpt` `Date:` lines are rewritten to the epoch.
///   - `*.xml` entries have `xilinx:coreCreationDateTime`,
///     `SourceLocation` absolute paths, and `ProjectID` redacted.
///
/// Idempotent.
pub fn redact_xo(path: &std::path::Path) -> Result<()> {
    let bytes = std::fs::read(path)?;
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|e| XilinxError::XoRedaction(format!("read zip: {e}")))?;
    let mut out = std::io::Cursor::new(Vec::<u8>::new());
    {
        let mut writer = zip::ZipWriter::new(&mut out);
        let mut names: Vec<String> = (0..archive.len())
            .filter_map(|i| archive.by_index(i).ok().map(|e| e.name().to_string()))
            .collect();
        names.sort();
        for name in &names {
            let mut entry = archive
                .by_name(name)
                .map_err(|e| XilinxError::XoRedaction(format!("entry {name}: {e}")))?;
            let opts = SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated)
                .last_modified_time(zip::DateTime::default());
            let mut buf = Vec::with_capacity(entry.size() as usize);
            entry.read_to_end(&mut buf)?;
            let redacted: Vec<u8> = if name.ends_with(".rpt") {
                match std::str::from_utf8(&buf) {
                    Ok(text) => redact_rpt(text).into_bytes(),
                    Err(_) => buf,
                }
            } else if name.ends_with(".xml") {
                match std::str::from_utf8(&buf) {
                    Ok(text) => redact_xml_payload(text).into_bytes(),
                    Err(_) => buf,
                }
            } else {
                buf
            };
            writer
                .start_file(name.clone(), opts)
                .map_err(|e| XilinxError::XoRedaction(format!("start: {e}")))?;
            writer.write_all(&redacted)?;
        }
        writer
            .finish()
            .map_err(|e| XilinxError::XoRedaction(format!("finish: {e}")))?;
    }
    out.seek(SeekFrom::Start(0))?;
    std::fs::write(path, out.into_inner())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::process::MockToolRunner;

    #[test]
    fn empty_verilog_list_is_rejected() {
        let runner = MockToolRunner::new();
        let inputs = PackageXoInputs {
            top_name: "k".into(),
            verilog_files: vec![],
            device_info: DeviceInfo {
                part_num: "x".into(),
                clock_period: "3.33".into(),
            },
            clock_period: "3.33".into(),
            kernel_xml: KernelXmlArgs {
                top_name: "k".into(),
                clock_period: "3.33".into(),
                ports: vec![],
            },
            kernel_out_path: PathBuf::from("/tmp/k.xo"),
        };
        let err = pack_xo(&runner, &inputs).unwrap_err();
        assert!(matches!(err, XilinxError::KernelXml(_)));
    }

    fn write_xo(path: &std::path::Path, entries: &[(&str, &str)]) {
        let f = std::fs::File::create(path).unwrap();
        let mut zw = zip::ZipWriter::new(f);
        let opts = SimpleFileOptions::default();
        for (name, body) in entries {
            zw.start_file((*name).to_string(), opts).unwrap();
            zw.write_all(body.as_bytes()).unwrap();
        }
        zw.finish().unwrap();
    }

    #[test]
    fn redact_xo_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("k.xo");
        write_xo(&path, &[("hello.txt", "hi")]);
        redact_xo(&path).unwrap();
        let first = std::fs::read(&path).unwrap();
        redact_xo(&path).unwrap();
        let second = std::fs::read(&path).unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn redact_xml_rewrites_timestamp_sourceloc_projectid() {
        let input = r#"<root>
  <xilinx:coreCreationDateTime>2024-05-17T09:15:30Z</xilinx:coreCreationDateTime>
  <SourceLocation>/work/alice/build/cpp/foo.cc</SourceLocation>
  <meta ProjectID="deadbeefcafebabe0123456789abcdef"/>
</root>"#;
        let out = redact_xml_payload(input);
        assert!(out.contains("<xilinx:coreCreationDateTime>1980-01-01T00:00:00Z"));
        assert!(out.contains("<SourceLocation>cpp/foo.cc</SourceLocation>"));
        assert!(out.contains(r#"ProjectID="0123456789abcdef0123456789abcdef""#));
    }

    #[test]
    fn redact_rpt_rewrites_date_line() {
        let input = "\
Copyright ...\n\
Date:           Fri Mar 14 10:20:30 2025\n\
--+--\n";
        let out = redact_rpt(input);
        assert!(out.contains("Date:           Tue Jan 01 00:00:00 1980"));
    }

    #[test]
    fn redact_xo_applies_payload_redaction() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("k.xo");
        write_xo(
            &path,
            &[(
                "ip/meta.xml",
                "<xilinx:coreCreationDateTime>2024-05-17T09:15:30Z</xilinx:coreCreationDateTime>",
            )],
        );
        redact_xo(&path).unwrap();
        let mut z = zip::ZipArchive::new(std::io::Cursor::new(std::fs::read(&path).unwrap()))
            .unwrap();
        let mut body = String::new();
        z.by_name("ip/meta.xml")
            .unwrap()
            .read_to_string(&mut body)
            .unwrap();
        assert!(body.contains("1980-01-01T00:00:00Z"));
    }
}
