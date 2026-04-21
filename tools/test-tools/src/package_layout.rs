use std::collections::BTreeMap;
use std::fs::File;
use std::path::Path;

use tar::{Archive, EntryType};

use crate::common::{require_file, Result};

#[derive(Debug)]
struct Entry {
    kind: EntryType,
}

pub fn check_package_layout(path: &Path) -> Result<()> {
    require_file(path)?;
    let file =
        File::open(path).map_err(|error| format!("failed to open {}: {error}", path.display()))?;
    let mut archive = Archive::new(file);
    let mut entries = BTreeMap::new();

    for entry in archive
        .entries()
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?
    {
        let entry =
            entry.map_err(|error| format!("failed to read {} entry: {error}", path.display()))?;
        let path = entry
            .path()
            .map_err(|error| format!("failed to read tar entry path: {error}"))?
            .to_string_lossy()
            .into_owned();
        entries.insert(
            path,
            Entry {
                kind: entry.header().entry_type(),
            },
        );
    }

    require_regular(&entries, "usr/bin/tapa")?;
    require_regular(&entries, "usr/bin/tapa-cpp")?;
    require_regular(&entries, "usr/bin/tapacc")?;
    require_regular(&entries, "usr/include/tapa.h")?;
    require_regular(&entries, "usr/lib/libtapa.so")?;
    require_dir_entry(&entries, "usr/share/tapa/system-include/")?;

    if entries.contains_key("usr/share/tapa/runtime/tapa") {
        return Err(
            "package must not ship a second tapa binary under usr/share/tapa/runtime/tapa"
                .to_string(),
        );
    }
    for path in [
        "usr/lib/libOpenCL.a",
        "usr/lib/libOpenCL.so",
        "usr/lib/libasio.a",
        "usr/lib/libasio.so",
        "usr/lib/libfilesystem.a",
        "usr/lib/libfilesystem.so",
        "usr/lib/libminizip_ng.a",
        "usr/lib/libminizip_ng.so",
        "usr/lib/libtinyxml2.a",
        "usr/lib/libtinyxml2.so",
        "usr/lib/libyaml-cpp.a",
        "usr/lib/libyaml-cpp.so",
        "usr/lib/libz.a",
        "usr/lib/libz.so",
    ] {
        if entries.contains_key(path) {
            return Err(format!("package must not ship {path}"));
        }
    }

    Ok(())
}

fn require_regular(entries: &BTreeMap<String, Entry>, path: &str) -> Result<()> {
    match entries.get(path) {
        Some(entry) if entry.kind == EntryType::Regular => Ok(()),
        Some(entry) => Err(format!(
            "{path} must be a regular file, got {:?}",
            entry.kind
        )),
        None => Err(format!("package missing {path}")),
    }
}

fn require_dir_entry(entries: &BTreeMap<String, Entry>, path: &str) -> Result<()> {
    match entries.get(path) {
        Some(entry) if entry.kind == EntryType::Directory => Ok(()),
        Some(entry) => Err(format!("{path} must be a directory, got {:?}", entry.kind)),
        None => Err(format!("package missing {path}")),
    }
}
