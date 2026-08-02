//! `--bitstream-script` emission.
//!
//! Emits an executable Bash helper that downstream users can run to
//! drive `v++ --link` against the packaged `.xo`.

use std::fs;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use crate::error::{CliError, Result};

/// Render the `#!/bin/bash` v++ script with an absolute `.xo` path.
///
/// When `floorplan_xdc` is set, the pblock constraints are sourced as an
/// `OPT_DESIGN.TCL.PRE` hook so v++ applies the floorplan during
/// implementation; `connectivity_ini` adds a `--config` for memory
/// connectivity.
pub(super) fn render_vitis_script(
    top: &str,
    output_file: &Path,
    platform: Option<&str>,
    clock_period: Option<&str>,
    floorplan_xdc: Option<&Path>,
    connectivity_ini: Option<&Path>,
) -> Result<String> {
    let xo = absolutize_lexical(output_file).display().to_string();
    let floorplan_xdc = floorplan_xdc.map(|p| absolutize_lexical(p).display().to_string());
    let connectivity_ini = connectivity_ini.map(|p| absolutize_lexical(p).display().to_string());
    let target_frequency = clock_period
        .map(|clock| {
            let period = crate::util::parse_clock_period_ns(clock).map_err(|message| {
                CliError::InvalidArg(format!("cannot emit bitstream script: invalid {message}"))
            })?;
            tapa_xilinx::target_frequency_mhz(period)
                .map(|frequency| frequency.to_string())
                .map_err(|error| {
                    CliError::InvalidArg(format!(
                        "cannot emit bitstream script for clock period `{clock}` ns: {error}"
                    ))
                })
        })
        .transpose()?;

    Ok(format!(
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
    ))
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
    )?;
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

    fn render(
        top: &str,
        output_file: &Path,
        platform: Option<&str>,
        clock_period: Option<&str>,
        floorplan_xdc: Option<&Path>,
        connectivity_ini: Option<&Path>,
    ) -> String {
        render_vitis_script(
            top,
            output_file,
            platform,
            clock_period,
            floorplan_xdc,
            connectivity_ini,
        )
        .expect("valid script inputs")
    }

    /// A rendered script must keep its shebang and every expected needle,
    /// and never emit a forbidden one; `name` keys failure messages.
    fn assert_script(script: &str, contains: &[&str], not_contains: &[&str], name: &str) {
        assert!(
            script.starts_with("#!/bin/bash"),
            "[{name}] missing shebang:\n{script}"
        );
        for (expected, &needle) in contains
            .iter()
            .map(|n| (true, n))
            .chain(not_contains.iter().map(|n| (false, n)))
        {
            assert_eq!(
                script.contains(needle),
                expected,
                "[{name}] `{needle}`:\n{script}"
            );
        }
    }

    #[test]
    fn rendered_scripts_match_expected_substrings() {
        type Flags<'a> = (
            Option<&'a str>,
            Option<&'a str>,
            Option<&'a str>,
            Option<&'a str>,
        );
        type Row<'a> = (&'a str, Flags<'a>, &'a [&'a str], &'a [&'a str]);

        let mhz_clock = (1000.0_f64 / 300.75).to_string();
        let cases: &[Row] = &[
            (
                "platform",
                (
                    Some("xilinx_u250_gen3x16_xdma_4_1_202210_1"),
                    None,
                    None,
                    None,
                ),
                &["PLATFORM=xilinx_u250_gen3x16_xdma_4_1_202210_1"],
                &[],
            ),
            (
                "frequency",
                (None, Some("3.33"), None, None),
                &["TARGET_FREQUENCY=300", "--kernel_frequency"],
                &[],
            ),
            (
                "frequency-never-rounds-up",
                (None, Some(&mhz_clock), None, None),
                &["TARGET_FREQUENCY=300"],
                &["TARGET_FREQUENCY=301"],
            ),
            (
                "xdc-hook",
                (
                    Some("plat"),
                    Some("3.33"),
                    Some("/work/floorplan.xdc"),
                    None,
                ),
                &["--vivado.prop=run.impl_1.STEPS.OPT_DESIGN.TCL.PRE=/work/floorplan.xdc"],
                &[],
            ),
            (
                "ini-config",
                (
                    Some("plat"),
                    Some("3.33"),
                    None,
                    Some("/work/link_config.ini"),
                ),
                &["--config /work/link_config.ini"],
                &[],
            ),
            (
                "absent-hooks",
                (Some("plat"), None, None, None),
                &[],
                &["--config ", "OPT_DESIGN.TCL.PRE"],
            ),
        ];

        for &(name, flags, contains, not_contains) in cases {
            let (platform, clock, xdc, ini) = flags;
            let script = render(
                "Top",
                Path::new("/tmp/a.xo"),
                platform,
                clock,
                xdc.map(Path::new),
                ini.map(Path::new),
            );
            assert_script(&script, contains, not_contains, name);
        }

        // The bare render with an explicit top/xo binding also warns to
        // fill in the default platform.
        let skeleton = render("VecAdd", Path::new("/tmp/out.xo"), None, None, None, None);
        assert_script(
            &skeleton,
            &[
                "TOP=VecAdd",
                "XO='/tmp/out.xo'",
                "v++ ${DEBUG}",
                "PLATFORM=\"\"",
                "Please edit this file and set a valid PLATFORM",
            ],
            &[],
            "skeleton",
        );
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
    fn invalid_clock_period_is_rejected() {
        for clock in ["fast", "0", "-1", "NaN", "inf", "1e-300", "1e300"] {
            let error =
                render_vitis_script("Top", Path::new("/tmp/a.xo"), None, Some(clock), None, None)
                    .expect_err("invalid clock period");
            assert!(error.to_string().contains("bitstream script"), "{error}");
        }
    }

    #[test]
    fn absolutize_lexical_keeps_absolute_paths() {
        let abs = Path::new("/tmp/out.xo");
        assert_eq!(absolutize_lexical(abs), abs.to_path_buf());
    }

    #[test]
    fn all_optional_args_stay_on_one_vpp_command() {
        // Regression: a blank line between `--config` and `--kernel_frequency`
        // once broke the `\` continuation, orphaning `--kernel_frequency` as
        // its own (failing) command. Every arg must continue the v++ command.
        let script = render(
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
}
