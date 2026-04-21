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
            "{} and {} have different extracted contents",
            actual.display(),
            expected.display()
        ));
    }

    let actual_bytes = fs::read(actual)
        .map_err(|error| format!("failed to read {}: {error}", actual.display()))?;
    let expected_bytes = fs::read(expected)
        .map_err(|error| format!("failed to read {}: {error}", expected.display()))?;
    if actual_bytes != expected_bytes {
        return Err(format!(
            "{} and {} have different zip bytes",
            actual.display(),
            expected.display()
        ));
    }
    Ok(())
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
