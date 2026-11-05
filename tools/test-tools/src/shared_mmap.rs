use std::fs::File;
use std::path::Path;
use zip::ZipArchive;

use crate::common::{archive_text, Result};

const LOAD_FSM_PATH: &str = "ip_repo/tapa_xrtl_VecAddShared_1_0/src/Load_fsm.v";
const UNSUPPORTED_PRAGMAS: &[&str] = &[
    "RS clk port=ap_clk",
    "RS rst port=ap_rst_n active=low",
    "RS ap-ctrl ap_start=ap_start ap_done=ap_done ap_idle=ap_idle ap_ready=ap_ready scalar=(srcs_offset|n)",
    "RS ap-ctrl ap_start=Mmap2Stream_0__ap_start ap_done=Mmap2Stream_0__ap_done ap_idle=Mmap2Stream_0__ap_idle ap_ready=Mmap2Stream_0__ap_ready scalar=Mmap2Stream_0___.*",
];

pub fn check_shared_mmap_pragmas(path: &Path) -> Result<()> {
    let file =
        File::open(path).map_err(|error| format!("failed to open {}: {error}", path.display()))?;
    let mut archive = ZipArchive::new(file)
        .map_err(|error| format!("failed to read {} as zip: {error}", path.display()))?;

    let source = archive_text(&mut archive, LOAD_FSM_PATH)?;
    for pragma in UNSUPPORTED_PRAGMAS {
        if has_pragma(&source, pragma) {
            return Err(format!(
                "{LOAD_FSM_PATH} contains unsupported pragma: {pragma}"
            ));
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
