//! `.xo` archive redaction for reproducible builds.

use std::io::{Read, Seek, SeekFrom, Write};

use quick_xml::events::BytesStart;
use zip::write::SimpleFileOptions;

use crate::error::{Result, XilinxError};

fn redact_rpt(text: &str) -> String {
    use std::sync::OnceLock;
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        regex::Regex::new("Date:           ... ... .. ..:..:.. ....")
            .expect("static regex compiles")
    });
    let redacted = re.replace_all(text, "Date:           Tue Jan 01 00:00:00 1980");
    redact_cpp_paths(&redacted)
}

fn redact_xml_payload(text: &str) -> String {
    match redact_xml_event_based(text) {
        Ok(out) => out,
        Err(_) => redact_cpp_paths(text),
    }
}

fn redact_xml_event_based(text: &str) -> std::result::Result<String, quick_xml::Error> {
    use quick_xml::events::{BytesText, Event};
    use quick_xml::{Reader, Writer};

    let mut reader = Reader::from_str(text);
    reader.config_mut().trim_text(false);

    let mut writer = Writer::new(Vec::new());
    let mut buf = Vec::new();
    let mut stack: Vec<String> = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Eof) => break,
            Ok(Event::Start(mut e)) => {
                redact_element_attrs(&mut e);
                let name = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                stack.push(name);
                writer.write_event(Event::Start(e))?;
            }
            Ok(Event::Empty(mut e)) => {
                redact_element_attrs(&mut e);
                writer.write_event(Event::Empty(e))?;
            }
            Ok(Event::Text(t)) => {
                let text_content = t.unescape()?.into_owned();
                let redacted = if matches!(
                    stack.last().map(String::as_str),
                    Some("xilinx:coreCreationDateTime" | "coreCreationDateTime")
                ) {
                    "1980-01-01T00:00:00Z".to_string()
                } else {
                    redact_cpp_paths(&text_content)
                };
                writer.write_event(Event::Text(BytesText::new(&redacted)))?;
            }
            Ok(Event::End(e)) => {
                stack.pop();
                writer.write_event(Event::End(e))?;
            }
            Ok(event) => {
                writer.write_event(event)?;
            }
            Err(e) => return Err(e),
        }
        buf.clear();
    }

    Ok(String::from_utf8(writer.into_inner()).unwrap_or_default())
}

fn redact_element_attrs(elem: &mut BytesStart<'_>) {
    let attrs: Vec<(String, String)> = elem
        .attributes()
        .filter_map(|a| a.ok())
        .map(|attr| {
            let key = std::str::from_utf8(attr.key.as_ref())
                .unwrap_or("")
                .to_string();
            let value = attr
                .unescape_value()
                .unwrap_or_else(|_| std::str::from_utf8(&attr.value).unwrap_or("").into())
                .into_owned();
            (key, value)
        })
        .collect();
    elem.clear_attributes();
    for (key, value) in attrs {
        let new_value = if key == "ProjectID" || key.ends_with(":ProjectID") {
            String::from("0123456789abcdef0123456789abcdef")
        } else {
            redact_source_location(&value)
        };
        elem.push_attribute((key.as_str(), new_value.as_str()));
    }
}

fn redact_source_location(text: &str) -> String {
    for marker in ["rootfscpp/", "cpp/"] {
        if let Some(idx) = text.rfind(marker) {
            return text[idx..].to_string();
        }
    }
    text.to_string()
}

fn redact_cpp_paths(text: &str) -> String {
    use std::sync::OnceLock;
    static RE_CPP_PATH: OnceLock<regex::Regex> = OnceLock::new();
    let re_cpp_path = RE_CPP_PATH.get_or_init(|| {
        regex::Regex::new(r#"(?:\.\./|/)?(?:[^\s<>"|]*/)+((?:cpp|rootfscpp)/)"#)
            .expect("static regex compiles")
    });
    re_cpp_path.replace_all(text, "$1").into_owned()
}

/// Rewrite a `.xo` ZIP in place so two invocations on the same inputs
/// produce semantically-equal outputs.
///
///   - ZIP timestamps are zeroed to the MS-DOS epoch.
///   - `*.rpt` `Date:` lines are rewritten to the epoch.
///   - `*.xml` entries have `xilinx:coreCreationDateTime`,
///     `SourceLocation` absolute paths, and `ProjectID` redacted.
///
/// Idempotent.
pub fn redact_xo(path: &camino::Utf8Path) -> Result<()> {
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
            let is_dir = name.ends_with('/');
            let unix_mode = if is_dir {
                0o755
            } else {
                entry.unix_mode().unwrap_or(0o644) & 0o777
            };
            let opts = SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated)
                .last_modified_time(zip::DateTime::default())
                .unix_permissions(unix_mode);
            if is_dir {
                writer
                    .add_directory(name.clone(), opts)
                    .map_err(|e| XilinxError::XoRedaction(format!("directory: {e}")))?;
                continue;
            }
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
    use camino::Utf8PathBuf;

    fn write_xo(path: &camino::Utf8Path, entries: &[(&str, &str)]) {
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
    fn redacts_sandbox_cpp_paths_from_reports() {
        let left =
            "../home/.cache/bazel/_bazel_1000/sandbox/processwrapper-sandbox/62/execroot/_main/\
             bazel-out/k8-fastbuild/bin/tests/functional/reproducibility/vadd-xo.tapa/cpp/VecAdd.cpp:31";
        let right =
            "../home/.cache/bazel/_bazel_1000/sandbox/processwrapper-sandbox/54/execroot/_main/\
             bazel-out/k8-fastbuild/bin/tests/apps/vadd/vadd-xo.tapa/cpp/VecAdd.cpp:31";

        assert_eq!(
            redact_rpt(&format!("| interface | s_axilite | {left} in vecadd |")),
            redact_rpt(&format!("| interface | s_axilite | {right} in vecadd |")),
        );
        let xml = redact_xml_payload(&format!(r#"<Pragma location="{left}" SOURCE="{left}"/>"#));
        assert!(xml.contains(r#"location="cpp/VecAdd.cpp:31""#));
        assert!(xml.contains(r#"SOURCE="cpp/VecAdd.cpp:31""#));
    }

    #[test]
    fn redact_xo_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let path = Utf8PathBuf::from_path_buf(tmp.path().join("k.xo")).unwrap();
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
    fn redaction_rewrites_remote_rootfscpp_paths() {
        let input = "\
<SourceLocation>/tmp/tapa-remote/tapa-1-2-0/rootfscpp/Add.cpp:15</SourceLocation>\n\
| /tmp/tapa-remote/tapa-1-2-0/rootfscpp/Mmap2Stream.cpp:27:20 |\n";
        let xml = redact_xml_payload(input);
        assert!(xml.contains("<SourceLocation>rootfscpp/Add.cpp:15</SourceLocation>"));
        assert!(xml.contains("| rootfscpp/Mmap2Stream.cpp:27:20 |"));
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
        let path = Utf8PathBuf::from_path_buf(tmp.path().join("k.xo")).unwrap();
        write_xo(
            &path,
            &[(
                "ip/meta.xml",
                "<xilinx:coreCreationDateTime>2024-05-17T09:15:30Z</xilinx:coreCreationDateTime>",
            )],
        );
        redact_xo(&path).unwrap();
        let mut z =
            zip::ZipArchive::new(std::io::Cursor::new(std::fs::read(&path).unwrap())).unwrap();
        let mut body = String::new();
        z.by_name("ip/meta.xml")
            .unwrap()
            .read_to_string(&mut body)
            .unwrap();
        assert!(body.contains("1980-01-01T00:00:00Z"));
    }

    #[test]
    fn redact_xo_preserves_directory_entry_modes() {
        let tmp = tempfile::tempdir().unwrap();
        let path = Utf8PathBuf::from_path_buf(tmp.path().join("k.xo")).unwrap();
        let f = std::fs::File::create(&path).unwrap();
        let mut zw = zip::ZipWriter::new(f);
        zw.add_directory(
            "ip_repo/tapa_xrtl_Cannon_1_0/src/",
            SimpleFileOptions::default().unix_permissions(0o755),
        )
        .unwrap();
        zw.start_file(
            "ip_repo/tapa_xrtl_Cannon_1_0/src/Cannon.v",
            SimpleFileOptions::default().unix_permissions(0o644),
        )
        .unwrap();
        zw.write_all(b"module Cannon; endmodule\n").unwrap();
        zw.finish().unwrap();

        redact_xo(&path).unwrap();

        let mut z =
            zip::ZipArchive::new(std::io::Cursor::new(std::fs::read(&path).unwrap())).unwrap();
        let dir = z.by_name("ip_repo/tapa_xrtl_Cannon_1_0/src/").unwrap();
        let mode = dir.unix_mode().unwrap_or_default();
        assert_ne!(
            mode & 0o170_000,
            0o100_000,
            "directory entry was rewritten as a regular file: {mode:o}",
        );
        assert_eq!(mode & 0o777, 0o755);
    }
}
