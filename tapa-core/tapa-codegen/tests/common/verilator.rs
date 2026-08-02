//! Shared Verilator cosim scaffold for the integration cosim tests:
//! probe for a `verilator` binary, stage the embedded RTL assets and a
//! C++ testbench in a tempdir, run `verilator --cc --exe --build`, and
//! assert the simulator prints PASS. The warning suppressions are the
//! union of the per-test lists (`-Wno-fatal` already keeps warnings
//! non-fatal, so extra suppressions only silence stderr noise).

use std::process::Command;

/// Is `verilator` runnable on `PATH`? Cosim tests skip cleanly without it.
pub fn available() -> bool {
    Command::new("verilator")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
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
        eprintln!("skipping {top} cosim: `verilator` not found on PATH");
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

    let build = Command::new("verilator")
        .current_dir(root)
        .args(&args)
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
