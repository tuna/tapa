//! `--bitstream-script` emission: port of
//! `tapa/steps/pack.py::get_vitis_script`.
//!
//! Emits a `#!/bin/bash` helper that downstream users can run to
//! drive `v++ --link` against the just-packaged `.xo`. The script is
//! a literal transliteration of the Python template (retired in
//! but is preserved here for parity with historical build recipes
//! that call into it).

use std::fs;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use crate::error::Result;

const VITIS_COMMAND_BASIC: &[&str] = &[
    "v++ ${DEBUG} \\",
    "  --link \\",
    "  --output \"${OUTPUT_DIR}/${TOP}_${PLATFORM}.xclbin\" \\",
    "  --kernel ${TOP} \\",
    "  --platform ${PLATFORM} \\",
    "  --target ${TARGET} \\",
    "  --report_level 2 \\",
    "  --temp_dir \"${OUTPUT_DIR}/${TOP}_${PLATFORM}.temp\" \\",
    "  --optimize 3 \\",
    "  --connectivity.nk ${TOP}:1:${TOP} \\",
    "  --save-temps \\",
    "  \"${XO}\" \\",
    "  --vivado.synth.jobs ${MAX_SYNTH_JOBS} \\",
    "  --vivado.prop=run.impl_1.STEPS.PHYS_OPT_DESIGN.IS_ENABLED=1 \\",
    "  --vivado.prop=run.impl_1.STEPS.OPT_DESIGN.ARGS.DIRECTIVE=$STRATEGY \\",
    "  --vivado.prop=run.impl_1.STEPS.PLACE_DESIGN.ARGS.DIRECTIVE=$PLACEMENT_STRATEGY \\",
    "  --vivado.prop=run.impl_1.STEPS.PHYS_OPT_DESIGN.ARGS.DIRECTIVE=$STRATEGY \\",
    "  --vivado.prop=run.impl_1.STEPS.ROUTE_DESIGN.ARGS.DIRECTIVE=$STRATEGY \\",
];
const CONFIG_OPTION: &str = "  --config \"${CONFIG_FILE}\" \\";
const CLOCK_OPTION: &str = "  --kernel_frequency ${TARGET_FREQUENCY} \\";

/// Render the `#!/bin/bash` v++ script mirroring Python's
/// `get_vitis_script`. `output_file` is absolutised exactly as Python
/// did via `os.path.abspath`.
#[must_use]
pub(super) fn render_vitis_script(
    top: &str,
    output_file: &Path,
    platform: Option<&str>,
    clock_period: Option<&str>,
    connectivity: Option<&Path>,
) -> String {
    let mut lines: Vec<String> = vec![
        "#!/bin/bash".to_string(),
        "TARGET=hw".to_string(),
        "# TARGET=hw_emu".to_string(),
        "# DEBUG=-g".to_string(),
        String::new(),
        format!("TOP={top}"),
        format!("XO='{}'", absolutize(output_file).display()),
    ];

    let mut vitis_command: Vec<String> =
        VITIS_COMMAND_BASIC.iter().map(|s| (*s).to_string()).collect();

    if let Some(conn) = connectivity {
        lines.push(format!("CONFIG_FILE='{}'", absolutize(conn).display()));
        vitis_command.push(CONFIG_OPTION.to_string());
    }

    if let Some(clock) = clock_period {
        if let Ok(period) = clock.parse::<f64>() {
            #[allow(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "Python uses `round(1000 / float(clock_period))` → int; \
                          the i64 roundtrip mirrors that truncation"
            )]
            let target = (1000.0_f64 / period).round() as i64;
            lines.push(format!("TARGET_FREQUENCY={target}"));
            vitis_command.push(CLOCK_OPTION.to_string());
        }
    } else {
        lines.push(
            r#">&2 echo "Using the default clock target of the platform.""#.to_string(),
        );
    }

    if let Some(p) = platform {
        lines.push(format!("PLATFORM={p}"));
    } else {
        lines.push("PLATFORM=\"\"".to_string());
        lines.push(
            "if [ -z $PLATFORM ]; then echo 'Please edit this file and set a valid \
             PLATFORM= on line \"${LINENO}\"'; exit; fi"
                .to_string(),
        );
        lines.push(String::new());
    }

    lines.push("OUTPUT_DIR=\"$(pwd)/vitis_run_${TARGET}\"".to_string());
    lines.push(String::new());
    lines.push("MAX_SYNTH_JOBS=8".to_string());
    lines.push("STRATEGY=\"Explore\"".to_string());
    lines.push("PLACEMENT_STRATEGY=\"EarlyBlockPlacement\"".to_string());
    lines.push(String::new());
    lines.extend(vitis_command);
    lines.push(String::new());

    lines.join("\n")
}

/// Write the script to `dest`, making it executable on Unix
/// (`chmod 0o755`). Mirrors Python's `open(...).write(script)` plus
/// the implicit `+x` that the shell-script-emission recipe expects.
pub(super) fn write_vitis_script(
    dest: &Path,
    top: &str,
    output_file: &Path,
    platform: Option<&str>,
    clock_period: Option<&str>,
    connectivity: Option<&Path>,
) -> Result<()> {
    if let Some(parent) = dest.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    let body = render_vitis_script(top, output_file, platform, clock_period, connectivity);
    fs::write(dest, body)?;
    set_executable(dest)?;
    Ok(())
}

#[cfg(unix)]
fn set_executable(dest: &Path) -> Result<()> {
    let mut perms = fs::metadata(dest)?.permissions();
    perms.set_mode(0o755);
    fs::set_permissions(dest, perms)?;
    Ok(())
}

#[cfg(not(unix))]
fn set_executable(_dest: &Path) -> Result<()> {
    Ok(())
}

/// Match Python's `os.path.abspath`: absolute paths stay, relative
/// paths are resolved against `std::env::current_dir()` with no
/// symlink resolution. We intentionally do not use
/// `std::fs::canonicalize` because the target `.xo` may not yet
/// exist when the script is emitted.
fn absolutize(p: &Path) -> PathBuf {
    if p.is_absolute() {
        return p.to_path_buf();
    }
    std::env::current_dir().map_or_else(|_| p.to_path_buf(), |cwd| cwd.join(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_minimum_script_skeleton() {
        let script = render_vitis_script(
            "VecAdd",
            Path::new("/tmp/out.xo"),
            None,
            None,
            None,
        );
        assert!(script.starts_with("#!/bin/bash"));
        assert!(script.contains("TOP=VecAdd"));
        assert!(script.contains("XO='/tmp/out.xo'"));
        assert!(script.contains("v++ ${DEBUG}"));
    }

    #[test]
    fn includes_platform_when_provided() {
        let script = render_vitis_script(
            "Top",
            Path::new("/tmp/a.xo"),
            Some("xilinx_u250_gen3x16_xdma_4_1_202210_1"),
            None,
            None,
        );
        assert!(script.contains("PLATFORM=xilinx_u250_gen3x16_xdma_4_1_202210_1"));
    }

    #[test]
    fn emits_target_frequency_from_clock_period() {
        let script = render_vitis_script(
            "Top",
            Path::new("/tmp/a.xo"),
            None,
            Some("3.33"),
            None,
        );
        assert!(
            script.contains("TARGET_FREQUENCY=300"),
            "expected round(1000/3.33)=300, got: {script}",
        );
        assert!(script.contains("--kernel_frequency"));
    }

    #[test]
    fn pack_bitstream_script_writes_shell() {
        let dir = tempfile::tempdir().expect("tempdir");
        let out = dir.path().join("run.sh");
        write_vitis_script(
            &out,
            "VecAdd",
            Path::new("/tmp/a.xo"),
            Some("plat"),
            Some("3.33"),
            None,
        )
        .expect("write script");

        assert!(out.is_file(), "script must exist");
        let body = fs::read_to_string(&out).expect("read");
        assert!(body.contains("#!/bin/bash"));
        assert!(body.contains("v++"));
        assert!(body.contains("--kernel_frequency"));

        #[cfg(unix)]
        {
            let mode = fs::metadata(&out).expect("stat").permissions().mode();
            assert!(
                mode & 0o111 != 0,
                "bitstream script must be executable; got mode {mode:o}",
            );
        }
    }
}
