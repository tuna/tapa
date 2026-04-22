//! `--bitstream-script` emission: port of
//! the implementation.
//!
//! Emits a `#!/bin/bash` helper that downstream users can run to
//! drive `v++ --link` against the just-packaged `.xo`. The script is
//! a literal transliteration of the template (unsupported in
//! but is preserved here for compatibility with historical build recipes
//! that call into it).

use std::fs;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use crate::error::Result;

/// Render the `#!/bin/bash` v++ script mirroring current
/// `get_vitis_script`. `output_file` is absolutised exactly as current
/// did via `os.path.abspath`.
#[must_use]
pub(super) fn render_vitis_script(
    top: &str,
    output_file: &Path,
    platform: Option<&str>,
    clock_period: Option<&str>,
    connectivity: Option<&Path>,
) -> String {
    let mut env = minijinja::Environment::new();
    env.add_template("vitis_script", include_str!("templates/vitis_script.sh.j2"))
        .expect("template parses");

    let xo = absolutize(output_file).display().to_string();
    let config_file = connectivity.map(|conn| absolutize(conn).display().to_string());
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

    let ctx = minijinja::context! {
        top,
        xo,
        config_file,
        target_frequency,
        platform,
    };
    format!(
        "#!/bin/bash\n{}",
        env.get_template("vitis_script")
            .expect("template exists")
            .render(ctx)
            .expect("render succeeds")
    )
}

/// Write the script to `dest`, making it executable on Unix
/// (`chmod 0o755`). Mirrors `open(...).write(script)` plus
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

/// Match `os.path.abspath`: absolute paths stay, relative
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
        let script = render_vitis_script("VecAdd", Path::new("/tmp/out.xo"), None, None, None);
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
        let script = render_vitis_script("Top", Path::new("/tmp/a.xo"), None, Some("3.33"), None);
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

    #[test]
    fn includes_connectivity_config_option() {
        let script = render_vitis_script(
            "Top",
            Path::new("/tmp/a.xo"),
            None,
            None,
            Some(Path::new("/tmp/conn.ini")),
        );
        assert!(script.contains("CONFIG_FILE='/tmp/conn.ini'"));
        assert!(script.contains("--config \"${CONFIG_FILE}\""));
    }

    #[test]
    fn default_platform_warning_emitted() {
        let script = render_vitis_script("Top", Path::new("/tmp/a.xo"), None, None, None);
        assert!(script.contains("PLATFORM=\"\""));
        assert!(script.contains("Please edit this file and set a valid PLATFORM"));
    }

    #[test]
    fn invalid_clock_period_is_ignored() {
        let script = render_vitis_script("Top", Path::new("/tmp/a.xo"), None, Some("fast"), None);
        assert!(!script.contains("TARGET_FREQUENCY"));
        assert!(!script.contains("--kernel_frequency"));
    }

    #[test]
    fn absolutize_keeps_absolute_paths() {
        let abs = Path::new("/tmp/out.xo");
        assert_eq!(absolutize(abs), abs.to_path_buf());
    }
}
