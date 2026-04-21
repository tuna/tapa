use std::fs::File;
use std::path::Path;
use zip::ZipArchive;

use crate::common::{archive_text, Result};

const SRC_DIR: &str = "ip_repo/tapa_xrtl_VecAddShared_1_0/src";
const EXPECTED_PRAGMAS: &[(&str, &str)] = &[
    ("Load_fsm.v", "RS clk port=ap_clk"),
    ("Load_fsm.v", "RS rst port=ap_rst_n active=low"),
    (
        "Load_fsm.v",
        "RS ap-ctrl ap_start=ap_start ap_done=ap_done ap_idle=ap_idle ap_ready=ap_ready scalar=(srcs_offset|n)",
    ),
    (
        "Load_fsm.v",
        "RS ap-ctrl ap_start=Mmap2Stream_0__ap_start ap_done=Mmap2Stream_0__ap_done ap_idle=Mmap2Stream_0__ap_idle ap_ready=Mmap2Stream_0__ap_ready scalar=Mmap2Stream_0___.*",
    ),
];

pub fn check_shared_mmap_pragmas(path: &Path) -> Result<()> {
    let file =
        File::open(path).map_err(|error| format!("failed to open {}: {error}", path.display()))?;
    let mut archive = ZipArchive::new(file)
        .map_err(|error| format!("failed to read {} as zip: {error}", path.display()))?;

    for (file_name, pragma) in EXPECTED_PRAGMAS {
        let path = format!("{SRC_DIR}/{file_name}");
        let source = archive_text(&mut archive, &path)?;
        if !has_pragma(&source, pragma) {
            return Err(format!("{path} missing pragma: {pragma}"));
        }
    }
    Ok(())
}

fn has_pragma(source: &str, pragma: &str) -> bool {
    let expected = format!("// pragma {pragma}");
    source.lines().any(|line| line.trim() == expected)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn has_pragma_ignores_outer_whitespace() {
        assert!(has_pragma(
            "  // pragma RS clk port=ap_clk  \n",
            "RS clk port=ap_clk"
        ));
    }

    #[test]
    fn has_pragma_rejects_partial_lines() {
        assert!(!has_pragma(
            "// pragma RS clk port=ap_clk extra",
            "RS clk port=ap_clk"
        ));
    }
}
