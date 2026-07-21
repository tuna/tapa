//! `--bitstream-script` emission.
//!
//! Emits an executable Bash helper that downstream users can run to
//! drive `v++ --link` against the packaged `.xo`.

use std::fs;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use crate::error::Result;

/// Render the `#!/bin/bash` v++ script with an absolute `.xo` path.
///
/// When `floorplan_xdc` is set, the pblock constraints are sourced as an
/// `OPT_DESIGN.TCL.PRE` hook so v++ applies the floorplan during
/// implementation; `connectivity_ini` adds a `--config` for memory
/// connectivity.
#[must_use]
pub(super) fn render_vitis_script(
    top: &str,
    output_file: &Path,
    platform: Option<&str>,
    clock_period: Option<&str>,
    floorplan_xdc: Option<&Path>,
    connectivity_ini: Option<&Path>,
) -> String {
    let xo = absolutize_lexical(output_file).display().to_string();
    let floorplan_xdc = floorplan_xdc.map(|p| absolutize_lexical(p).display().to_string());
    let connectivity_ini = connectivity_ini.map(|p| absolutize_lexical(p).display().to_string());
    let target_frequency = clock_period.and_then(|clock| {
        clock.parse::<f64>().ok().map(|period| {
            #[allow(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "uses `round(1000 / float(clock_period))` → int; \
                          the i64 roundtrip mirrors that truncation"
            )]
            let target = (1000.0_f64 / period).round() as i64;
            target.to_string()
        })
    });

    format!(
        "#!/bin/bash\n{}",
        crate::util::render_template(
            "vitis_script",
            include_str!("templates/vitis_script.sh.j2"),
            minijinja::context! {
                top,
                xo,
                target_frequency,
                platform,
                floorplan_xdc,
                connectivity_ini,
            },
        )
    )
}

/// Write the script to `dest`, making it executable on Unix
/// (`chmod 0o755`).
pub(super) fn write_vitis_script(
    dest: &Path,
    top: &str,
    output_file: &Path,
    platform: Option<&str>,
    clock_period: Option<&str>,
    floorplan_xdc: Option<&Path>,
    connectivity_ini: Option<&Path>,
) -> Result<()> {
    if let Some(parent) = dest.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    let body = render_vitis_script(
        top,
        output_file,
        platform,
        clock_period,
        floorplan_xdc,
        connectivity_ini,
    );
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

/// Absolute paths stay; relative paths are resolved against
/// `std::env::current_dir()` with no symlink resolution. We
/// intentionally do not use `std::fs::canonicalize` because the
/// target `.xo` may not yet exist when the script is emitted.
fn absolutize_lexical(p: &Path) -> PathBuf {
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
        let script =
            render_vitis_script("VecAdd", Path::new("/tmp/out.xo"), None, None, None, None);
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

    #[test]
    fn default_platform_warning_emitted() {
        let script = render_vitis_script("Top", Path::new("/tmp/a.xo"), None, None, None, None);
        assert!(script.contains("PLATFORM=\"\""));
        assert!(script.contains("Please edit this file and set a valid PLATFORM"));
    }

    #[test]
    fn invalid_clock_period_is_ignored() {
        let script = render_vitis_script(
            "Top",
            Path::new("/tmp/a.xo"),
            None,
            Some("fast"),
            None,
            None,
        );
        assert!(!script.contains("TARGET_FREQUENCY"));
        assert!(!script.contains("--kernel_frequency"));
    }

    #[test]
    fn absolutize_lexical_keeps_absolute_paths() {
        let abs = Path::new("/tmp/out.xo");
        assert_eq!(absolutize_lexical(abs), abs.to_path_buf());
    }

    #[test]
    fn floorplan_xdc_is_sourced_as_opt_design_hook() {
        let script = render_vitis_script(
            "Top",
            Path::new("/tmp/a.xo"),
            Some("plat"),
            Some("3.33"),
            Some(Path::new("/work/floorplan.xdc")),
            None,
        );
        assert!(
            script
                .contains("--vivado.prop=run.impl_1.STEPS.OPT_DESIGN.TCL.PRE=/work/floorplan.xdc"),
            "floorplanned script must source the pblock XDC, got:\n{script}"
        );
    }

    #[test]
    fn all_optional_args_stay_on_one_vpp_command() {
        // Regression: a blank line between `--config` and `--kernel_frequency`
        // once broke the `\` continuation, orphaning `--kernel_frequency` as
        // its own (failing) command. Every arg must continue the v++ command.
        let script = render_vitis_script(
            "Top",
            Path::new("/tmp/a.xo"),
            Some("plat"),
            Some("3.33"),
            Some(Path::new("/work/floorplan.xdc")),
            Some(Path::new("/work/link_config.ini")),
        );
        let lines: Vec<&str> = script.lines().collect();
        let kf = lines
            .iter()
            .position(|l| l.contains("--kernel_frequency"))
            .expect("script sets --kernel_frequency");
        assert!(
            kf > 0 && lines[kf - 1].trim_end().ends_with('\\'),
            "the line before --kernel_frequency must end with `\\` (continuation), \
             got:\n{:?}\n{:?}",
            lines[kf - 1],
            lines[kf],
        );
        // And there is no stray blank line amid the continued args.
        assert!(
            !script.contains("\\\n\n  --"),
            "a continued arg line is followed by a blank line, breaking v++:\n{script}"
        );
    }

    #[test]
    fn connectivity_ini_is_added_as_config() {
        let script = render_vitis_script(
            "Top",
            Path::new("/tmp/a.xo"),
            Some("plat"),
            Some("3.33"),
            None,
            Some(Path::new("/work/link_config.ini")),
        );
        assert!(
            script.contains("--config /work/link_config.ini"),
            "connectivity ini must be sourced as a v++ --config, got:\n{script}"
        );
    }

    #[test]
    fn no_config_without_connectivity() {
        let script = render_vitis_script(
            "Top",
            Path::new("/tmp/a.xo"),
            Some("plat"),
            None,
            None,
            None,
        );
        assert!(
            !script.contains("--config "),
            "a script with no connectivity must not emit a bare --config",
        );
    }

    #[test]
    fn no_floorplan_hook_without_xdc() {
        let script = render_vitis_script(
            "Top",
            Path::new("/tmp/a.xo"),
            Some("plat"),
            None,
            None,
            None,
        );
        assert!(
            !script.contains("OPT_DESIGN.TCL.PRE"),
            "unfloorplanned script must not reference a pblock XDC",
        );
    }
}
