//! Device / platform resolution + `<work_dir>/settings.json` persistence
//! helpers for `tapa synth`.
//!
//! Splits the platform-directory lookup and the
//! `--platform` / `--part-num` / `--clock-period` precedence rules out
//! of `mod.rs` so the dispatcher stays focused on flow.

use std::path::{Path, PathBuf};

use tapa_xilinx::{parse_device_info as xilinx_parse_device_info, DeviceInfo};

use crate::error::{CliError, Result};

use super::SynthArgs;

pub(super) fn resolve_device_info(args: &SynthArgs) -> Result<DeviceInfo> {
    let part_override = args.part_num.as_deref();
    let clock_override_owned = args.clock_period.map(|c| format!("{c}"));
    let clock_override = clock_override_owned.as_deref();

    if let Some(platform) = args.platform.as_deref() {
        let resolved = resolve_platform_dir(platform).ok_or_else(|| {
            CliError::InvalidArg(format!(
                "cannot find the specified platform `{platform}`; are you sure it has \
                 been installed, e.g., in `/opt/xilinx/platforms`?",
            ))
        })?;
        return Ok(xilinx_parse_device_info(
            &resolved,
            part_override,
            clock_override,
        )?);
    }

    let Some(part_num) = part_override else {
        return Err(CliError::InvalidArg(
            "cannot determine the target part number; please either specify \
             `--platform` so the target part number can be extracted from it, or \
             specify `--part-num` directly."
                .to_string(),
        ));
    };
    let Some(clock_period) = clock_override else {
        return Err(CliError::InvalidArg(
            "cannot determine the target clock period; please either specify \
             `--platform` so the target clock period can be extracted from it, or \
             specify `--clock-period` directly."
                .to_string(),
        ));
    };
    Ok(DeviceInfo {
        part_num: part_num.to_string(),
        clock_period: clock_period.to_string(),
    })
}

fn resolve_platform_dir(platform: &str) -> Option<PathBuf> {
    let raw = Path::new(platform);
    let parent = raw.parent().map(Path::to_path_buf).unwrap_or_default();
    let basename = raw.file_name().map_or_else(
        || platform.to_string(),
        |s| s.to_string_lossy().into_owned(),
    );
    let normalized = basename.replace([':', '.'], "_");
    let direct = if parent.as_os_str().is_empty() {
        PathBuf::from(&normalized)
    } else {
        parent.join(&normalized)
    };
    if direct.is_dir() {
        return Some(direct);
    }
    for root in platform_roots() {
        let candidate = root.join("platforms").join(&normalized);
        if candidate.is_dir() {
            return Some(candidate);
        }
    }
    None
}

fn platform_roots() -> Vec<PathBuf> {
    let mut out = vec![PathBuf::from("/opt/xilinx")];
    if let Ok(p) = std::env::var("XILINX_VITIS") {
        out.push(PathBuf::from(p));
    }
    if let Ok(p) = std::env::var("XILINX_SDX") {
        out.push(PathBuf::from(p));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    use clap::Parser;

    fn parse_synth(extra: &[&str]) -> SynthArgs {
        let mut argv = vec!["synth"];
        argv.extend_from_slice(extra);
        SynthArgs::try_parse_from(argv).expect("parse synth args")
    }

    #[test]
    fn part_num_without_clock_errors() {
        let args = parse_synth(&["--part-num", "xcvu37p"]);
        let err = resolve_device_info(&args).expect_err("missing clock");
        assert!(matches!(err, CliError::InvalidArg(ref m) if m.contains("clock period")));
    }

    #[test]
    fn part_num_and_clock_resolve_without_platform() {
        let args = parse_synth(&["--part-num", "xcvu37p-fsvh2892-2L-e", "--clock-period", "3.33"]);
        let info = resolve_device_info(&args).expect("must resolve");
        assert_eq!(info.part_num, "xcvu37p-fsvh2892-2L-e");
        assert_eq!(info.clock_period, "3.33");
    }

    #[test]
    fn resolve_platform_dir_normalizes_separators() {
        let dir = tempfile::tempdir().expect("tempdir");
        let raw = "weird_platform:1.0";
        let normalized = "weird_platform_1_0";
        let target = dir.path().join(normalized);
        std::fs::create_dir_all(&target).expect("mkdir");
        let qualified = dir.path().join(raw);
        let resolved = resolve_platform_dir(qualified.to_str().expect("utf-8"))
            .expect("must resolve normalized basename");
        assert_eq!(resolved, target);
    }
}
