//! Tests for the `tapa compile` composite command wiring. Split out of
//! `mod.rs` to keep the production module under the 450-LOC soft budget.

#![allow(
    clippy::similar_names,
    reason = "args/argv pair matches the production naming"
)]

use super::*;

#[test]
fn compile_args_round_trip_via_clap() {
    let args = CompileArgs::try_parse_from([
        "compile",
        "--input",
        "vadd.cpp",
        "--top",
        "VecAdd",
        "--platform",
        "xilinx_u250",
        "--output",
        "vadd.xo",
    ])
    .expect("compile args parse");
    assert_eq!(args.analyze.input_files.len(), 1);
    assert_eq!(args.analyze.top, "VecAdd");
    assert_eq!(args.synth.platform.as_deref(), Some("xilinx_u250"));
    assert_eq!(
        args.pack.output.as_ref().map(|p| p.display().to_string()),
        Some("vadd.xo".to_string()),
    );
}
