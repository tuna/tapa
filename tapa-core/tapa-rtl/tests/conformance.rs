//! Conformance tests for `tapa-rtl` against real HLS-generated Verilog.

use tapa_rtl::classify::{classify_port, classify_ports, HandshakeRole, PortClass};
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

// ── Port classification ─────────────────────────────────────────────

#[test]
fn classify_lower_level_ports() {
    let m = VerilogModule::parse(&fixture("LowerLevelTask.v")).expect("parse");
    let classified = classify_ports(&m.ports);

    // Handshake ports
    let clk = classified.iter().find(|(n, _)| n == "ap_clk").unwrap();
    assert_eq!(
        clk.1,
        PortClass::Handshake {
            role: HandshakeRole::Clock
        }
    );

    // Input stream
    let istream_dout = classified
        .iter()
        .find(|(n, _)| n == "istream_s_dout")
        .unwrap();
    match &istream_dout.1 {
        PortClass::IStream { base, suffix } => {
            assert_eq!(base, "istream_s");
            assert_eq!(suffix, "_dout");
        }
        other @ (PortClass::Handshake { .. }
        | PortClass::MAxi { .. }
        | PortClass::OStream { .. }
        | PortClass::Unclassified) => panic!("expected IStream, got {other:?}"),
    }

    // Output stream
    let ostream_din = classified
        .iter()
        .find(|(n, _)| n == "ostreams_s_din")
        .unwrap();
    match &ostream_din.1 {
        PortClass::OStream { base, suffix } => {
            assert_eq!(base, "ostreams_s");
            assert_eq!(suffix, "_din");
        }
        other @ (PortClass::Handshake { .. }
        | PortClass::MAxi { .. }
        | PortClass::IStream { .. }
        | PortClass::Unclassified) => panic!("expected OStream, got {other:?}"),
    }

    // Scalar
    let scalar = classified.iter().find(|(n, _)| n == "scalar").unwrap();
    assert_eq!(scalar.1, PortClass::Unclassified);
}

#[test]
fn classify_handshake_ports_complete() {
    let m = VerilogModule::parse(&fixture("LowerLevelTask.v")).expect("parse");
    let clk = classify_port(m.find_port("ap_clk").unwrap());
    let rst = classify_port(m.find_port("ap_rst_n").unwrap());
    let start = classify_port(m.find_port("ap_start").unwrap());
    let done = classify_port(m.find_port("ap_done").unwrap());
    let idle = classify_port(m.find_port("ap_idle").unwrap());
    let ready = classify_port(m.find_port("ap_ready").unwrap());

    assert_eq!(clk, PortClass::Handshake { role: HandshakeRole::Clock });
    assert_eq!(rst, PortClass::Handshake { role: HandshakeRole::ResetN });
    assert_eq!(start, PortClass::Handshake { role: HandshakeRole::Start });
    assert_eq!(done, PortClass::Handshake { role: HandshakeRole::Done });
    assert_eq!(idle, PortClass::Handshake { role: HandshakeRole::Idle });
    assert_eq!(ready, PortClass::Handshake { role: HandshakeRole::Ready });
}

#[test]
fn find_port_by_affixes() {
    let m = VerilogModule::parse(&fixture("LowerLevelTask.v")).expect("parse");
    let found = m.find_port_by_affixes("istream_s", "_dout");
    assert!(found.is_some(), "find istream data port");
    assert_eq!(found.unwrap().name, "istream_s_dout");
}

// ── Pragma extraction ───────────────────────────────────────────────

#[test]
fn attribute_attached_to_following_port() {
    let src = "module M (a);\n(* RS_CLK *)\ninput a;\nendmodule\n";
    let m = VerilogModule::parse(src).expect("parse");
    let port = m.find_port("a").expect("port a");
    assert!(port.pragma.is_some(), "pragma attached to port");
    assert_eq!(port.pragma.as_ref().unwrap().key, "RS_CLK");
}

#[test]
fn pragmas_extracted() {
    let m = VerilogModule::parse(&fixture("LowerLevelTask.v")).expect("parse");
    assert!(!m.pragmas.is_empty(), "has pragmas");
    // Should have CORE_GENERATION_INFO and fsm_encoding pragmas
    let core_info = m.pragmas.iter().find(|p| p.key == "CORE_GENERATION_INFO");
    assert!(core_info.is_some(), "has CORE_GENERATION_INFO pragma");
}

#[test]
fn pragma_fsm_encoding() {
    let m = VerilogModule::parse(&fixture("UpperLevelTask.v")).expect("parse");
    let fsm = m.pragmas.iter().find(|p| p.key == "fsm_encoding");
    assert!(fsm.is_some(), "has fsm_encoding pragma");
    assert_eq!(fsm.unwrap().value.as_deref(), Some("none"));
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
    assert_eq!(w.msb[0].repr, "64");
    assert_eq!(w.lsb[0].repr, "0");
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
    assert!(msg.contains("malformed port"), "error describes malformed port: {msg}");
}

#[test]
fn malformed_parameter_rejected() {
    let src = "module BadParam (a);\nparameter = ;\ninput a;\nendmodule\n";
    let err = VerilogModule::parse(src).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("BadParam"), "error includes module name: {msg}");
    assert!(msg.contains("malformed parameter"), "error describes malformed param: {msg}");
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

#[test]
fn body_parse_error_includes_module_name() {
    let src = "module BadBody (a);\ninput [31:0 a\nendmodule\n";
    let err = VerilogModule::parse(src).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("BadBody"), "error includes module name: {msg}");
}

#[test]
fn malformed_leading_pragma_preserved_as_raw() {
    let src = "(* BROKEN = raw_value *)\nmodule M (a);\ninput a;\nendmodule\n";
    let m = VerilogModule::parse(src).expect("parse with malformed pragma");
    assert!(!m.pragmas.is_empty(), "malformed pragma preserved");
    let raw = &m.pragmas[0];
    assert!(raw.raw_line.contains("BROKEN"), "raw_line has original text");
}

#[test]
fn malformed_body_pragma_preserved_as_raw() {
    let src = "module M (a);\n(* BAD = unquoted *)\ninput a;\nendmodule\n";
    let m = VerilogModule::parse(src).expect("parse with body malformed pragma");
    let bad = m.pragmas.iter().find(|p| p.raw_line.contains("BAD"));
    assert!(bad.is_some(), "malformed body pragma preserved");
}
