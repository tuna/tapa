//! Shared Verilator cosim scaffold for the integration cosim tests:
//! probe for a `verilator` binary, stage the embedded RTL assets and a
//! C++ testbench in a tempdir, run `verilator --cc --exe --build`, and
//! assert the simulator prints PASS. The warning suppressions are the
//! union of the per-test lists (`-Wno-fatal` already keeps warnings
//! non-fatal, so extra suppressions only silence stderr noise).

use std::ffi::OsString;
use std::process::Command;

/// Verilator executable: `VERILATOR_BIN` when set (the Bazel-managed
/// hermetic binary staged in runfiles), else `verilator` from `PATH`.
fn verilator_bin() -> OsString {
    std::env::var_os("VERILATOR_BIN").unwrap_or_else(|| "verilator".into())
}

/// `VERILATOR_ROOT` for the hermetic binary: the @verilator module's
/// executable bakes in a placeholder root, so derive the real one from
/// the binary's location (`<root>/bin/verilator`). `None` for system
/// installs, which self-root.
fn verilator_root() -> Option<std::path::PathBuf> {
    let bin = std::path::PathBuf::from(verilator_bin());
    let root = bin.parent()?.parent()?;
    root.join("include")
        .join("verilated.mk")
        .exists()
        .then(|| root.to_path_buf())
}

/// Is verilator runnable? Cosim tests skip cleanly without it — unless
/// `VERILATOR_BIN` is explicitly configured, in which case a broken
/// binary is a hard failure rather than a silent green skip (the
/// fail-closed contract for Bazel runs, mirroring frt's probe).
///
/// `VERILATOR_BIN` is removed from every spawned environment: Verilator's
/// own Perl frontend consumes that variable to pick its backend binary,
/// so leaving it pointed at a wrapper would make the wrapper re-exec
/// itself forever.
pub fn available() -> bool {
    let configured = std::env::var_os("VERILATOR_BIN").is_some();
    let ok = Command::new(verilator_bin())
        .arg("--version")
        .env_remove("VERILATOR_BIN")
        .output()
        .is_ok_and(|output| output.status.success());
    if !ok && configured {
        let bin = std::path::PathBuf::from(verilator_bin());
        panic!("VERILATOR_BIN is set but not runnable: {}", bin.display());
    }
    ok
}

/// Embedded Verilog asset source by file name.
pub fn asset_source(name: &str) -> Vec<u8> {
    tapa_codegen::support_assets::VerilogAssets::get(name)
        .unwrap_or_else(|| panic!("{name} is an embedded asset"))
        .data
        .into_owned()
}

/// Build the `top` module from the embedded RTL `assets` (written under
/// their asset names), the `extra` inline `(file, contents)` sources,
/// and the C++ `testbench`, with `-Gname=value` generic overrides from
/// `gparams`; then run the simulator and assert it exits zero after
/// printing PASS. Skips with a note when `verilator` is not installed.
pub fn run_cosim(
    top: &str,
    gparams: &[(&str, u32)],
    assets: &[&str],
    extra: &[(&str, &str)],
    testbench: &str,
) {
    if !available() {
        eprintln!("skipping {top} cosim: verilator not found via VERILATOR_BIN or PATH");
        return;
    }

    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let mut sources: Vec<String> = Vec::new();
    for name in assets {
        std::fs::write(root.join(name), asset_source(name)).expect("write embedded RTL");
        sources.push((*name).to_string());
    }
    for (name, contents) in extra {
        std::fs::write(root.join(name), contents).expect("write inline source");
        sources.push((*name).to_string());
    }
    std::fs::write(root.join("tb.cpp"), testbench).expect("write testbench");

    let mut args: Vec<String> = vec![
        "--cc".into(),
        "--exe".into(),
        "--build".into(),
        "--top-module".into(),
        top.to_string(),
    ];
    args.extend(
        gparams
            .iter()
            .map(|(name, value)| format!("-G{name}={value}")),
    );
    for flag in [
        "-Wno-WIDTH",
        "-Wno-UNUSEDSIGNAL",
        "-Wno-UNOPTFLAT",
        "-Wno-CASEINCOMPLETE",
        "-Wno-fatal",
        "--Mdir",
        "obj_dir",
        "-o",
        "sim",
    ] {
        args.push(flag.to_string());
    }
    args.extend(sources);
    args.push("tb.cpp".to_string());

    let build = Command::new(verilator_bin())
        .current_dir(root)
        .args(&args)
        .env_remove("VERILATOR_BIN")
        .envs(verilator_root().map(|r| ("VERILATOR_ROOT", r)))
        .output()
        .expect("spawn verilator");
    assert!(
        build.status.success(),
        "verilator build failed ({top}, {gparams:?}):\n{}\n{}",
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr),
    );

    let run = Command::new(root.join("obj_dir").join("sim"))
        .output()
        .expect("run simulator");
    assert!(
        run.status.success() && String::from_utf8_lossy(&run.stdout).contains("PASS"),
        "{top} cosim failed ({gparams:?}):\n{}\n{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr),
    );
}
