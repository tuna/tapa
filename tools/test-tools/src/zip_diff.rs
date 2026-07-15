use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::Read;
use std::path::Path;
use zip::ZipArchive;

use crate::common::Result;

pub fn zip_diff(actual: &Path, expected: &Path) -> Result<()> {
    let actual_entries = read_zip_entries(actual)?;
    let expected_entries = read_zip_entries(expected)?;
    if actual_entries != expected_entries {
        return Err(format!(
            "{} and {} have different extracted contents\n{}",
            actual.display(),
            expected.display(),
            describe_entry_differences(&actual_entries, &expected_entries),
        ));
    }

    let actual_bytes = fs::read(actual)
        .map_err(|error| format!("failed to read {}: {error}", actual.display()))?;
    let expected_bytes = fs::read(expected)
        .map_err(|error| format!("failed to read {}: {error}", expected.display()))?;
    if actual_bytes != expected_bytes {
        return Err(format!(
            "{} and {} have identical extracted contents but different zip bytes \
             (container-level difference: compression, ordering, or metadata)",
            actual.display(),
            expected.display()
        ));
    }
    Ok(())
}

/// Render a human-readable summary of how two extracted zip trees differ:
/// entries present on only one side, and entries present on both whose
/// bytes differ (with a short textual preview when both are UTF-8).
fn describe_entry_differences(
    actual: &BTreeMap<String, Vec<u8>>,
    expected: &BTreeMap<String, Vec<u8>>,
) -> String {
    let mut lines = Vec::new();

    let only_actual: Vec<&String> = actual
        .keys()
        .filter(|k| !expected.contains_key(*k))
        .collect();
    if !only_actual.is_empty() {
        lines.push(format!("only in actual ({}):", only_actual.len()));
        for name in only_actual {
            lines.push(format!("  + {name} ({} bytes)", actual[name].len()));
        }
    }

    let only_expected: Vec<&String> = expected
        .keys()
        .filter(|k| !actual.contains_key(*k))
        .collect();
    if !only_expected.is_empty() {
        lines.push(format!("only in expected ({}):", only_expected.len()));
        for name in only_expected {
            lines.push(format!("  - {name} ({} bytes)", expected[name].len()));
        }
    }

    let mut differing = Vec::new();
    for (name, actual_bytes) in actual {
        if let Some(expected_bytes) = expected.get(name) {
            if actual_bytes != expected_bytes {
                differing.push((name, actual_bytes, expected_bytes));
            }
        }
    }
    if !differing.is_empty() {
        lines.push(format!("differing content ({}):", differing.len()));
        for (name, actual_bytes, expected_bytes) in differing {
            lines.push(format!(
                "  ~ {name} (actual {} bytes, expected {} bytes)",
                actual_bytes.len(),
                expected_bytes.len(),
            ));
            if let Some(preview) = first_line_difference(actual_bytes, expected_bytes) {
                lines.push(preview);
            }
        }
    }

    lines.join("\n")
}

/// When both sides are UTF-8, locate the first line that differs and
/// return a two-line `actual`/`expected` preview. Returns `None` for
/// binary payloads (where a textual preview would be noise).
fn first_line_difference(actual: &[u8], expected: &[u8]) -> Option<String> {
    let actual_text = std::str::from_utf8(actual).ok()?;
    let expected_text = std::str::from_utf8(expected).ok()?;
    for (index, (a, e)) in actual_text.lines().zip(expected_text.lines()).enumerate() {
        if a != e {
            return Some(format!(
                "      first diff at line {}:\n        actual:   {}\n        expected: {}",
                index + 1,
                truncate(a),
                truncate(e),
            ));
        }
    }
    // Same prefix; the shorter file is a truncation of the longer.
    Some(format!(
        "      identical for the first {} shared lines; line counts differ",
        actual_text
            .lines()
            .count()
            .min(expected_text.lines().count()),
    ))
}

fn truncate(line: &str) -> String {
    const MAX: usize = 200;
    if line.len() <= MAX {
        return line.to_string();
    }
    // Cut on a char boundary at or below MAX so multi-byte codepoints
    // are never split.
    let mut end = MAX;
    while end > 0 && !line.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}… ({} bytes total)", &line[..end], line.len())
}

fn read_zip_entries(path: &Path) -> Result<BTreeMap<String, Vec<u8>>> {
    let file =
        File::open(path).map_err(|error| format!("failed to open {}: {error}", path.display()))?;
    let mut archive = ZipArchive::new(file)
        .map_err(|error| format!("failed to read {} as zip: {error}", path.display()))?;
    let mut entries = BTreeMap::new();
    for index in 0..archive.len() {
        let mut file = archive
            .by_index(index)
            .map_err(|error| format!("failed to read {} entry {index}: {error}", path.display()))?;
        if file.is_dir() {
            continue;
        }
        let name = file.name().to_string();
        let mut contents = Vec::new();
        file.read_to_end(&mut contents)
            .map_err(|error| format!("failed to read {name} from {}: {error}", path.display()))?;
        if entries.insert(name.clone(), contents).is_some() {
            return Err(format!(
                "{} contains duplicate zip entry {name}",
                path.display()
            ));
        }
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::write::SimpleFileOptions;
    use zip::ZipWriter;

    #[test]
    fn zip_diff_accepts_identical_archives() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("first.zip");
        let second = dir.path().join("second.zip");
        write_zip(&first, &[("a.txt", b"hello")]);
        fs::copy(&first, &second).unwrap();

        zip_diff(&first, &second).unwrap();
    }

    #[test]
    fn zip_diff_rejects_content_differences() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("first.zip");
        let second = dir.path().join("second.zip");
        write_zip(&first, &[("a.txt", b"hello")]);
        write_zip(&second, &[("a.txt", b"bye")]);

        let error = zip_diff(&first, &second).unwrap_err();
        assert!(
            error.contains("different extracted contents"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn zip_diff_names_differing_and_missing_entries() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("first.zip");
        let second = dir.path().join("second.zip");
        write_zip(
            &first,
            &[
                ("shared.txt", b"line one\nline two\n"),
                ("only-actual.txt", b"x"),
            ],
        );
        write_zip(
            &second,
            &[
                ("shared.txt", b"line one\nline TWO\n"),
                ("only-expected.txt", b"y"),
            ],
        );

        let error = zip_diff(&first, &second).unwrap_err();
        assert!(
            error.contains("+ only-actual.txt"),
            "missing actual: {error}"
        );
        assert!(
            error.contains("- only-expected.txt"),
            "missing expected: {error}"
        );
        assert!(error.contains("~ shared.txt"), "missing differ: {error}");
        assert!(
            error.contains("first diff at line 2"),
            "missing line locator: {error}"
        );
    }

    fn write_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let file = File::create(path).unwrap();
        let mut writer = ZipWriter::new(file);
        let options = SimpleFileOptions::default();
        for (name, contents) in entries {
            writer.start_file(*name, options).unwrap();
            writer.write_all(contents).unwrap();
        }
        writer.finish().unwrap();
    }
}
