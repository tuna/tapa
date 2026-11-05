//! Conformance tests for `tapa-rtl` against real HLS-generated Verilog.

use tapa_rtl::module::VerilogModule;
use tapa_rtl::port::Direction;
use tapa_rtl::signal::SignalKind;

fn fixture(name: &str) -> String {
    let path = format!("{}/../testdata/rtl/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"))
}

// ── Leaf task parsing ───────────────────────────────────────────────

#[test]
fn parse_lower_level_task() {
    let m = VerilogModule::parse(&fixture("LowerLevelTask.v")).expect("parse leaf");
    assert_eq!(m.name, "LowerLevelTask", "module name");
    assert_eq!(m.ports.len(), 17, "port count");
    assert_eq!(m.parameters.len(), 1, "parameter count");
}

#[test]
fn lower_level_port_directions() {
    let m = VerilogModule::parse(&fixture("LowerLevelTask.v")).expect("parse");
    let clk = m.find_port("ap_clk").expect("ap_clk");
    assert_eq!(clk.direction, Direction::Input);

    let done = m.find_port("ap_done").expect("ap_done");
    assert_eq!(done.direction, Direction::Output);

    let istream_dout = m.find_port("istream_s_dout").expect("istream_s_dout");
    assert_eq!(istream_dout.direction, Direction::Input);
    assert!(istream_dout.width.is_some(), "istream_s_dout has width");
}

#[test]
fn lower_level_parameter_defaults() {
    let m = VerilogModule::parse(&fixture("LowerLevelTask.v")).expect("parse");
    assert_eq!(m.parameters[0].name, "ap_ST_fsm_state1");
    assert!(!m.parameters[0].default.is_empty(), "has default tokens");
}

#[test]
fn lower_level_signals() {
    let m = VerilogModule::parse(&fixture("LowerLevelTask.v")).expect("parse");
    assert!(m.signals.len() >= 2, "at least 2 signals");
    let ap_done_sig = m.signals.iter().find(|s| s.name == "ap_done");
    assert!(ap_done_sig.is_some(), "ap_done signal exists");
    assert_eq!(ap_done_sig.unwrap().kind, SignalKind::Reg);
}

#[test]
fn lower_level_source_preserved() {
    let src = fixture("LowerLevelTask.v");
    let m = VerilogModule::parse(&src).expect("parse");
    assert_eq!(m.source, src, "source preserved verbatim");
}

// ── Upper task parsing ──────────────────────────────────────────────

#[test]
fn parse_upper_level_task() {
    let m = VerilogModule::parse(&fixture("UpperLevelTask.v")).expect("parse upper");
    assert_eq!(m.name, "UpperLevelTask", "module name");
    assert_eq!(m.ports.len(), 17, "port count");
    assert_eq!(m.parameters.len(), 2, "parameter count");
}

#[test]
fn upper_level_has_stream_ports() {
    let m = VerilogModule::parse(&fixture("UpperLevelTask.v")).expect("parse");
    let istream = m.find_port("istream_s_dout").expect("istream data port");
    assert_eq!(istream.direction, Direction::Input);

    let ostream = m.find_port("ostreams_s_din").expect("ostream data port");
    assert_eq!(ostream.direction, Direction::Output);
}

// ── Port lookup ─────────────────────────────────────────────────────

#[test]
fn find_port_by_affixes() {
    let m = VerilogModule::parse(&fixture("LowerLevelTask.v")).expect("parse");
    let found = m.find_port_by_affixes("istream_s", "_dout");
    assert!(found.is_some(), "find istream data port");
    assert_eq!(found.unwrap().name, "istream_s_dout");
}

// ── Width expressions ───────────────────────────────────────────────

#[test]
fn width_as_token_sequence() {
    let m = VerilogModule::parse(&fixture("LowerLevelTask.v")).expect("parse");
    let istream = m.find_port("istream_s_dout").expect("port");
    let w = istream.width.as_ref().expect("has width");
    // Width is [64:0]
    assert!(!w.msb.is_empty(), "msb has tokens");
    assert!(!w.lsb.is_empty(), "lsb has tokens");
    assert_eq!(w.msb[0], "64");
    assert_eq!(w.lsb[0], "0");
}

// ── Negative tests ──────────────────────────────────────────────────

#[test]
fn empty_input_returns_error() {
    let err = VerilogModule::parse("").unwrap_err();
    assert!(err.to_string().contains("empty"), "error: {err}");
}

#[test]
fn non_verilog_returns_error() {
    let err = VerilogModule::parse("this is not verilog code").unwrap_err();
    assert!(!err.to_string().is_empty(), "error: {err}");
}

#[test]
fn malformed_port_declaration_rejected() {
    let src = "module Broken (a);\ninput [31:0 a\nendmodule\n";
    let err = VerilogModule::parse(src).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("Broken"), "error includes module name: {msg}");
    assert!(
        msg.contains("malformed port"),
        "error describes malformed port: {msg}"
    );
}

#[test]
fn malformed_parameter_rejected() {
    let src = "module BadParam (a);\nparameter = ;\ninput a;\nendmodule\n";
    let err = VerilogModule::parse(src).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("BadParam"),
        "error includes module name: {msg}"
    );
    assert!(
        msg.contains("malformed parameter"),
        "error describes malformed param: {msg}"
    );
}

#[test]
fn undeclared_port_rejected() {
    let src = "module Missing (a, b);\ninput a;\nendmodule\n";
    let err = VerilogModule::parse(src).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains('b'), "error mentions undeclared port: {msg}");
}

#[test]
fn header_parse_error_includes_module_name() {
    let src = "module Broken (a\n";
    let err = VerilogModule::parse(src).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("Broken"), "error includes module name: {msg}");
}
