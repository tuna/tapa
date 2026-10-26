//! `--bitstream-script` emission.
//!
//! Emits an executable Bash helper that downstream users can run to
//! drive `v++ --link` against the packaged `.xo`.

use std::fs;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use tapa_ir::ClockPeriod;

use crate::error::{CliError, Result};
use crate::util::shell_quote;

#[derive(Debug, Clone, Copy)]
pub(super) struct EmbeddedPackage<'a> {
    pub kernel_image: &'a Path,
    pub rootfs: &'a Path,
    pub sd_files: &'a [PathBuf],
}

/// The optional inputs a link takes besides the `.xo`.
#[derive(Debug, Default, Clone, Copy)]
pub(super) struct LinkExtras<'a> {
    /// Pblock constraints, sourced as an `OPT_DESIGN.TCL.PRE` hook.
    pub floorplan_xdc: Option<&'a Path>,
    /// Memory connectivity, added as a `--config`.
    pub connectivity_ini: Option<&'a Path>,
    /// The AI Engine graph (`libadf.a`) and its PLIO connectivity.
    pub aie: Option<&'a (PathBuf, PathBuf)>,
    pub embedded: Option<EmbeddedPackage<'a>>,
}

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
    clock_period: Option<ClockPeriod>,
    extras: LinkExtras<'_>,
) -> Result<String> {
    #[derive(askama::Template)]
    #[template(path = "vitis_script.sh.j2", escape = "none")]
    struct Template<'a> {
        top: &'a str,
        xo: &'a str,
        target_frequency: Option<&'a str>,
        platform: Option<&'a str>,
        floorplan_xdc: Option<&'a str>,
        connectivity_ini: Option<&'a str>,
        /// The AI Engine graph and its link connectivity.
        aie: Option<(String, String)>,
        embedded: Option<(String, String, Vec<String>)>,
    }
    let LinkExtras {
        floorplan_xdc,
        connectivity_ini,
        aie,
        embedded,
    } = extras;
    if embedded.is_some() && aie.is_none() {
        return Err(CliError::InvalidArg(
            "embedded SD packaging requires an AI Engine graph".to_string(),
        ));
    }
    let quoted = |p: &Path| shell_quote(&absolutize_lexical(p).display().to_string());
    let aie = aie.map(|(libadf, cfg)| (quoted(libadf), quoted(cfg)));
    let embedded = embedded.map(|package| {
        (
            quoted(package.kernel_image),
            quoted(package.rootfs),
            package.sd_files.iter().map(|p| quoted(p)).collect(),
        )
    });
    let top = shell_quote(top);
    let xo = quoted(output_file);
    let platform = platform.filter(|value| !value.is_empty()).map(shell_quote);
    let floorplan_xdc = floorplan_xdc.map(quoted);
    let connectivity_ini = connectivity_ini.map(quoted);
    let target_frequency = clock_period
        .map(|clock| {
            let frequency = if aie.is_some() {
                1_000_000_000_000_u64
                    .checked_div(clock.picoseconds())
                    .filter(|&frequency| frequency > 0)
                    .ok_or_else(|| {
                        format!("clock period `{clock}` ns produces no usable frequency in Hz")
                    })
            } else {
                tapa_xilinx::target_frequency_mhz(clock.nanoseconds())
                    .map(u64::from)
                    .map_err(|error| error.to_string())
            };
            frequency
                .map(|frequency| frequency.to_string())
                .map_err(|error| {
                    CliError::InvalidArg(format!(
                        "cannot emit bitstream script for clock period `{clock}` ns: {error}"
                    ))
                })
        })
        .transpose()?;

    let rendered = format!(
        "#!/bin/bash\n{}",
        askama::Template::render(&Template {
            top: &top,
            xo: &xo,
            target_frequency: target_frequency.as_deref(),
            platform: platform.as_deref(),
            floorplan_xdc: floorplan_xdc.as_deref(),
            connectivity_ini: connectivity_ini.as_deref(),
            aie,
            embedded,
        })
        .expect("render succeeds")
    );
    Ok(rendered)
}

/// Write the script to `dest`, making it executable on Unix
/// (`chmod 0o755`).
pub(super) fn write_vitis_script(
    dest: &Path,
    top: &str,
    output_file: &Path,
    platform: Option<&str>,
    clock_period: Option<ClockPeriod>,
    extras: LinkExtras<'_>,
) -> Result<()> {
    if let Some(parent) = dest.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    let body = render_vitis_script(top, output_file, platform, clock_period, extras)?;
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
        clock_period: Option<ClockPeriod>,
        floorplan_xdc: Option<&Path>,
        connectivity_ini: Option<&Path>,
    ) -> String {
        render_vitis_script(
            top,
            output_file,
            platform,
            clock_period,
            LinkExtras {
                floorplan_xdc,
                connectivity_ini,
                aie: None,
                ..LinkExtras::default()
            },
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
            Option<ClockPeriod>,
            Option<&'a str>,
            Option<&'a str>,
        );
        type Row<'a> = (&'a str, Flags<'a>, &'a [&'a str], &'a [&'a str]);

        let mhz_clock = ClockPeriod::from_nanoseconds(1000.0_f64 / 300.75).expect("period");
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
                (None, Some(ClockPeriod::from_picoseconds(3330)), None, None),
                &["TARGET_FREQUENCY=300", "--kernel_frequency"],
                &[],
            ),
            (
                "frequency-never-rounds-up",
                (None, Some(mhz_clock), None, None),
                &["TARGET_FREQUENCY=300"],
                &["TARGET_FREQUENCY=301"],
            ),
            (
                "xdc-hook",
                (
                    Some("plat"),
                    Some(ClockPeriod::from_picoseconds(3330)),
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
                    Some(ClockPeriod::from_picoseconds(3330)),
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
                "XO=/tmp/out.xo",
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
            Some(ClockPeriod::from_picoseconds(3330)),
            LinkExtras::default(),
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

    /// With an AI Engine graph the link takes `libadf.a` and the PLIO
    /// connectivity and writes an `.xsa`, which a second `v++` packages.
    #[test]
    fn aie_script_links_the_graph_then_packages_the_xsa() {
        let dir = tempfile::tempdir().expect("tempdir");
        let script = dir.path().join("run.sh");
        let aie = (dir.path().join("k.libadf.a"), dir.path().join("k.aie.cfg"));
        write_vitis_script(
            &script,
            "Top",
            &dir.path().join("k.xo"),
            Some("vck"),
            Some(ClockPeriod::from_picoseconds(3200)),
            LinkExtras {
                aie: Some(&aie),
                ..LinkExtras::default()
            },
        )
        .expect("write script");
        let output = std::process::Command::new("bash")
            .args([
                "-c",
                r#"v++() { printf '%s\0' "$@"; printf '\n'; }
source "$1""#,
                "capture-vitis-script",
            ])
            .arg(&script)
            .current_dir(dir.path())
            .output()
            .expect("execute rendered Bash script");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let captured = String::from_utf8(output.stdout).expect("UTF-8 arguments");
        let runs: Vec<Vec<&str>> = captured
            .lines()
            .map(|line| line.split_terminator('\0').collect())
            .collect();
        let [link, package] = runs.as_slice() else {
            panic!("two v++ runs expected: {runs:?}")
        };
        let libadf = aie.0.to_str().expect("UTF-8");
        assert!(
            link.contains(&"--link") && link.contains(&libadf),
            "{link:?}"
        );
        assert!(
            link.windows(2)
                .any(|pair| pair == ["--config", aie.1.to_str().expect("UTF-8")]),
            "{link:?}"
        );
        assert!(!link.contains(&"--kernel"), "{link:?}");
        assert!(
            link.windows(2)
                .any(|pair| pair == ["--clock.default_freqhz", "312500000"]),
            "{link:?}"
        );
        assert!(!link.contains(&"--kernel_frequency"), "{link:?}");
        assert!(
            link.iter().any(|arg| arg.ends_with("Top_vck.xsa")),
            "{link:?}"
        );
        assert!(
            package.contains(&"--package") && package.contains(&libadf),
            "{package:?}"
        );
        assert!(
            package.iter().any(|arg| arg.ends_with("Top_vck.xclbin")),
            "{package:?}"
        );
    }

    #[test]
    fn aie_script_packages_embedded_sd_inputs() {
        let dir = tempfile::tempdir().expect("tempdir");
        let script = dir.path().join("run.sh");
        let aie = (dir.path().join("k.libadf.a"), dir.path().join("k.aie.cfg"));
        let kernel_image = dir.path().join("boot files/Image");
        let rootfs = dir.path().join("boot files/rootfs.ext4");
        let sd_files = vec![dir.path().join("host app"), dir.path().join("input.dat")];
        write_vitis_script(
            &script,
            "Top",
            &dir.path().join("k.xo"),
            Some("vck"),
            None,
            LinkExtras {
                aie: Some(&aie),
                embedded: Some(EmbeddedPackage {
                    kernel_image: &kernel_image,
                    rootfs: &rootfs,
                    sd_files: &sd_files,
                }),
                ..LinkExtras::default()
            },
        )
        .expect("write script");
        let output = std::process::Command::new("bash")
            .args([
                "-c",
                r#"v++() { if [ "$1" = --package ]; then printf '%s\0' "$@"; fi; }
source "$1""#,
                "capture-vitis-script",
            ])
            .arg(&script)
            .current_dir(dir.path())
            .output()
            .expect("execute rendered Bash script");
        assert!(output.status.success());
        let captured = String::from_utf8(output.stdout).expect("UTF-8 arguments");
        let package: Vec<&str> = captured.split_terminator('\0').collect();
        for arg in [
            "--package.boot_mode=sd".to_string(),
            format!("--package.kernel_image={}", kernel_image.display()),
            format!("--package.rootfs={}", rootfs.display()),
            format!("--package.sd_file={}", sd_files[0].display()),
            format!("--package.sd_file={}", sd_files[1].display()),
            "--package.defer_aie_run".to_string(),
        ] {
            assert!(package.contains(&arg.as_str()), "{package:?}");
        }
    }

    #[test]
    fn aie_script_stops_when_link_fails() {
        let dir = tempfile::tempdir().expect("tempdir");
        let script = dir.path().join("run.sh");
        let aie = (dir.path().join("k.libadf.a"), dir.path().join("k.aie.cfg"));
        write_vitis_script(
            &script,
            "Top",
            &dir.path().join("k.xo"),
            Some("vck"),
            None,
            LinkExtras {
                aie: Some(&aie),
                ..LinkExtras::default()
            },
        )
        .expect("write script");
        let output = std::process::Command::new("bash")
            .args([
                "-c",
                r#"v++() { printf '%s\0' "$@"; printf '\n'; if [ "$1" = --link ]; then return 1; fi; }
source "$1""#,
                "failing-vitis-script",
            ])
            .arg(&script)
            .current_dir(dir.path())
            .output()
            .expect("execute rendered Bash script");
        assert!(
            !output.status.success(),
            "a failed link must fail the script"
        );
        let captured = String::from_utf8(output.stdout).expect("UTF-8 arguments");
        assert!(captured.contains("--link"), "{captured}");
        assert!(!captured.contains("--package"), "{captured}");
    }

    #[test]
    fn rendered_script_passes_paths_as_literal_vpp_arguments() {
        let dir = tempfile::tempdir().expect("tempdir");
        for directory in [
            "build files",
            "designer's files",
            "shell $TAPA_PATH_FRAGMENT `printf expanded` * ?",
        ] {
            let work = dir.path().join(directory);
            let xo = work.join("design.xo");
            let xdc = work.join("floorplan.xdc");
            let ini = work.join("link_config.ini");
            let platform = work.join("board.xpfm");
            let script = dir.path().join("run.sh");
            write_vitis_script(
                &script,
                "Top",
                &xo,
                platform.to_str(),
                Some(ClockPeriod::from_picoseconds(3330)),
                LinkExtras {
                    floorplan_xdc: Some(&xdc),
                    connectivity_ini: Some(&ini),
                    aie: None,
                    ..LinkExtras::default()
                },
            )
            .expect("write script");
            let output = std::process::Command::new("bash")
                .args([
                    "-c",
                    r#"v++() { printf '%s\0' "$@"; }
source "$1""#,
                    "capture-vitis-script",
                ])
                .arg(&script)
                .env("TAPA_PATH_FRAGMENT", "expanded")
                .current_dir(dir.path())
                .output()
                .expect("execute rendered Bash script");
            assert!(
                output.status.success(),
                "script failed for {directory:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let captured = String::from_utf8(output.stdout).expect("UTF-8 arguments");
            let args: Vec<_> = captured.split_terminator('\0').collect();
            assert!(
                args.contains(&xo.to_str().expect("UTF-8 XO path")),
                "{args:?}"
            );
            assert!(
                args.windows(2).any(|pair| {
                    pair == ["--config", ini.to_str().expect("UTF-8 connectivity path")]
                }),
                "connectivity path was changed or split: {args:?}"
            );
            let xdc_arg = format!(
                "--vivado.prop=run.impl_1.STEPS.OPT_DESIGN.TCL.PRE={}",
                xdc.display()
            );
            assert!(args.contains(&xdc_arg.as_str()), "{args:?}");
            assert!(
                args.windows(2).any(|pair| {
                    pair == [
                        "--platform",
                        platform.to_str().expect("UTF-8 platform path"),
                    ]
                }),
                "platform path was changed or split: {args:?}"
            );
            assert!(
                args.windows(2)
                    .any(|pair| pair == ["--kernel_frequency", "300"]),
                "{args:?}"
            );
            assert!(
                args.windows(2).any(|pair| pair == ["--kernel", "Top"]),
                "{args:?}"
            );
            assert!(!args.contains(&"--clock.default_freqhz"), "{args:?}");
        }
    }

    /// Non-numeric, negative, and infinite periods cannot reach here — they
    /// never become a [`ClockPeriod`]. Zero and an absurdly long period
    /// still can, and neither yields a usable target frequency.
    #[test]
    fn a_period_with_no_usable_frequency_is_rejected() {
        for picoseconds in [0, u64::MAX] {
            let clock = ClockPeriod::from_picoseconds(picoseconds);
            let error = render_vitis_script(
                "Top",
                Path::new("/tmp/a.xo"),
                None,
                Some(clock),
                LinkExtras::default(),
            )
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
            Some(ClockPeriod::from_picoseconds(3330)),
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
