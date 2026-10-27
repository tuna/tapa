//! Device / platform resolution helpers for `tapa synth`. The resolved
//! values are persisted by the caller into `<work_dir>/tapa.json`.
//!
//! Splits the platform-directory lookup and the
//! `--platform` / `--part-num` / `--clock-period` precedence rules out
//! of `mod.rs` so the dispatcher stays focused on flow.

use camino::Utf8PathBuf;
use std::path::Path;

use tapa_ir::{ClockPeriod, Target};
use tapa_xilinx::{
    find_slash_platform, parse_device_info as xilinx_parse_device_info, DeviceInfo, SLASH_PLATFORMS,
};

use crate::error::{CliError, Result};

use super::SynthArgs;

/// Resolve the device from the `--platform` / `--part-num` /
/// `--clock-period` flags, cross-checked against the work directory's
/// flow `target`.
///
/// A built-in SLASH shell (typed table in `tapa-xilinx::platform`) is
/// consulted before any directory search; it and the `xilinx-slash`
/// target require each other, and a `--part-num` given alongside it must
/// equal the table's part. Every other combination resolves exactly as
/// before the SLASH target existed.
pub(super) fn resolve_device_info(args: &SynthArgs, target: Target) -> Result<DeviceInfo> {
    let part_override = args.part_num.as_deref();
    let clock_override_owned = args.clock_period.map(|c| c.to_string());
    let clock_override = clock_override_owned.as_deref();

    if let Some(platform) = args.platform.as_deref() {
        // The SLASH shells have no `.xpfm` on disk, so the typed table
        // answers for them before the directory search runs.
        if let Some(shell) = find_slash_platform(platform) {
            if target != Target::XilinxSlash {
                return Err(CliError::InvalidArg(format!(
                    "`--platform {platform}` is a SLASH shell, but this work directory \
                     targets `{}`; analyze with `--target xilinx-slash`",
                    target.as_str(),
                )));
            }
            if let Some(part) = part_override {
                if part != shell.part_num {
                    return Err(CliError::InvalidArg(format!(
                        "`--part-num {part}` does not match `--platform {platform}`, \
                         which builds part `{}`",
                        shell.part_num,
                    )));
                }
            }
            return Ok(DeviceInfo {
                part_num: shell.part_num.to_string(),
                clock_period: clock_override
                    .unwrap_or(shell.default_clock_period)
                    .to_string(),
            });
        }
        if target == Target::XilinxSlash {
            return Err(CliError::InvalidArg(format!(
                "`--platform {platform}` is not a SLASH shell; the `xilinx-slash` target \
                 requires {}",
                required_slash_platforms(),
            )));
        }
        let resolved = resolve_platform_dir(platform).ok_or_else(|| {
            CliError::InvalidArg(format!(
                "cannot find the specified platform `{platform}`; are you sure it has \
                 been installed, e.g., in `/opt/xilinx/platforms`?",
            ))
        })?;
        let info = xilinx_parse_device_info(&resolved, part_override, clock_override)?;
        ClockPeriod::from_nanoseconds_str(&info.clock_period).map_err(|error| {
            CliError::InvalidArg(format!(
                "platform `{platform}` provides an invalid target: {error}"
            ))
        })?;
        return Ok(info);
    }

    if target == Target::XilinxSlash {
        return Err(CliError::InvalidArg(format!(
            "the `xilinx-slash` target requires {}",
            required_slash_platforms(),
        )));
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

/// The `--platform` spellings the `xilinx-slash` target accepts, quoted
/// for an error message.
fn required_slash_platforms() -> String {
    SLASH_PLATFORMS
        .iter()
        .map(|shell| format!("`--platform {}`", shell.name))
        .collect::<Vec<_>>()
        .join(" or ")
}

pub fn resolve_platform_dir(platform: &str) -> Option<Utf8PathBuf> {
    let raw = Path::new(platform);
    let parent = raw.parent().map(Path::to_path_buf).unwrap_or_default();
    let basename = raw.file_name().map_or_else(
        || platform.to_string(),
        |s| s.to_string_lossy().into_owned(),
    );
    let normalized = basename.replace([':', '.'], "_");
    let direct = if parent.as_os_str().is_empty() {
        Utf8PathBuf::from(&normalized)
    } else {
        crate::util::utf8(parent.join(&normalized))
    };
    if direct.is_dir() {
        return Some(direct);
    }
    // Alveo shells install under `platforms/`; Vitis ships its own base
    // platforms (VCK190, VEK280, ...) under `base_platforms/`.
    for root in platform_roots() {
        for dir in ["platforms", "base_platforms"] {
            let candidate = root.join(dir).join(&normalized);
            if candidate.is_dir() {
                return Some(candidate);
            }
        }
    }
    None
}

fn platform_roots() -> Vec<Utf8PathBuf> {
    let mut out = vec![Utf8PathBuf::from("/opt/xilinx")];
    if let Ok(p) = std::env::var("XILINX_VITIS") {
        out.push(Utf8PathBuf::from(p));
    }
    if let Ok(p) = std::env::var("XILINX_SDX") {
        out.push(Utf8PathBuf::from(p));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    use clap::Parser;

    const V80_PART: &str = "xcv80-lsva4737-2MHP-e-S";

    fn parse_synth(extra: &[&str]) -> SynthArgs {
        let mut argv = vec!["synth"];
        argv.extend_from_slice(extra);
        SynthArgs::try_parse_from(argv).expect("parse synth args")
    }

    #[test]
    fn part_num_without_clock_errors() {
        let args = parse_synth(&["--part-num", "xcvu37p"]);
        let err = resolve_device_info(&args, Target::XilinxVitis).expect_err("missing clock");
        assert!(matches!(err, CliError::InvalidArg(ref m) if m.contains("clock period")));
    }

    #[test]
    fn part_num_and_clock_resolve_without_platform() {
        let args = parse_synth(&[
            "--part-num",
            "xcvu37p-fsvh2892-2L-e",
            "--clock-period",
            "3.33",
        ]);
        let info = resolve_device_info(&args, Target::XilinxVitis).expect("must resolve");
        assert_eq!(info.part_num, "xcvu37p-fsvh2892-2L-e");
        assert_eq!(info.clock_period, "3.33");
    }

    #[test]
    fn invalid_clock_period_is_rejected_by_clap() {
        for value in ["0", "-1", "NaN", "inf"] {
            let clock_arg = format!("--clock-period={value}");
            let error = SynthArgs::try_parse_from([
                "synth",
                "--part-num",
                "xcvu37p-fsvh2892-2L-e",
                &clock_arg,
            ])
            .expect_err("invalid clock period");
            assert!(error.to_string().contains("clock period"), "{error}");
        }
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

    #[test]
    fn slash_platform_on_slash_target_takes_the_table_defaults() {
        let args = parse_synth(&["--platform", "slash_v80_service"]);
        let info = resolve_device_info(&args, Target::XilinxSlash).expect("must resolve");
        assert_eq!(info.part_num, V80_PART);
        assert_eq!(info.clock_period, "3.33");
    }

    #[test]
    fn slash_platform_clock_override_wins_over_the_table_default() {
        let args = parse_synth(&["--platform", "slash_v80_service", "--clock-period", "5"]);
        let info = resolve_device_info(&args, Target::XilinxSlash).expect("must resolve");
        assert_eq!(info.clock_period, "5");
    }

    #[test]
    fn slash_platform_accepts_only_the_table_part() {
        let args = parse_synth(&["--platform", "slash_v80_service", "--part-num", V80_PART]);
        let info = resolve_device_info(&args, Target::XilinxSlash).expect("must resolve");
        assert_eq!(info.part_num, V80_PART);

        let args = parse_synth(&["--platform", "slash_v80_service", "--part-num", "xcu280"]);
        let err = resolve_device_info(&args, Target::XilinxSlash).expect_err("part clash");
        assert!(
            matches!(err, CliError::InvalidArg(ref m) if m.contains(V80_PART)),
            "{err}"
        );
    }

    #[test]
    fn slash_platform_rejects_every_other_target() {
        for target in [Target::XilinxVitis, Target::XilinxHls] {
            let args = parse_synth(&["--platform", "slash_v80_service"]);
            let err = resolve_device_info(&args, target).expect_err("target clash");
            assert!(
                matches!(err, CliError::InvalidArg(ref m)
                    if m.contains("xilinx-slash") && m.contains(target.as_str())),
                "{err}"
            );
        }
    }

    #[test]
    fn slash_target_rejects_non_slash_and_missing_platforms() {
        // A non-SLASH `--platform` must not fall through to the directory
        // search on the SLASH target.
        let args = parse_synth(&["--platform", "xilinx_u250_gen3x16_xdma_4_1_202210_1"]);
        let err = resolve_device_info(&args, Target::XilinxSlash).expect_err("wrong shell");
        assert!(
            matches!(err, CliError::InvalidArg(ref m)
                if m.contains("not a SLASH shell") && m.contains("slash_v80_service")),
            "{err}"
        );

        let args = parse_synth(&["--part-num", V80_PART, "--clock-period", "3.33"]);
        let err = resolve_device_info(&args, Target::XilinxSlash).expect_err("no shell");
        assert!(
            matches!(err, CliError::InvalidArg(ref m) if m.contains("slash_v80_service")),
            "{err}"
        );
    }
}
