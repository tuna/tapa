//! Hybrid mutation API for `VerilogModule`.
//!
//! Preserves the parsed interface (ports, signals, params, pragmas) as
//! structured AST while keeping the module body as raw text. Supports
//! adding/removing ports, signals, instances, and logic by manipulating
//! both the structured interface and the raw body text.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use regex::Regex;
use std::sync::LazyLock;

use crate::builder::{AlwaysBlock, ContinuousAssign, ModuleInstance};
use crate::error::BuilderError;
use crate::expression::expression_source;
use crate::port::{Direction, Port, Width};
use crate::signal::{Signal, SignalKind};
use crate::VerilogModule;

/// Regex patterns for cleanup: HLS-generated artifacts to remove.
static REGSLICE_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^.*_regslice_both\b.*$\n?").unwrap());
static AP_BLOCK_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^.*ap_ST_fsm_state\d+_blk\b.*$\n?").unwrap());
/// Remove HLS FSM parameter declarations.
static FSM_PARAM_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^.*ap_ST_fsm_state\d+\b.*$\n?").unwrap());
/// Remove `ap_CS_fsm` and `ap_NS_fsm` declarations and assignments.
static CS_NS_FSM_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^.*\bap_[CN]S_fsm\b.*$\n?").unwrap());
/// Remove initial blocks (power-on initialization).
static INITIAL_BLOCK_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?ms)^initial begin\n.*?end\n?").unwrap());
/// Remove `ap_ce_reg` declarations.
static AP_CE_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^.*\bap_ce_reg\b.*$\n?").unwrap());
/// Remove HLS-internal inverted-reset declarations and assignments.
static RST_INV_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(r"(?m)^.*\b{}\b.*$\n?", tapa_protocol::HLS_RST_INV)).unwrap()
});
/// Remove placeholder top-level handshake assigns from the HLS wrapper.
static AP_HANDSHAKE_ASSIGN_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^\s*assign\s+ap_(?:done|ready|idle)\s*=.*;\s*\n?").unwrap());

/// A mutable view of a `VerilogModule` that tracks additions and
/// body text modifications.
///
/// Ports and signals are mutated in place on `inner` (mirroring how
/// `cleanup_hls_artifacts`/`demote_*` already operate). Only append-only
/// fragments with no parsed original — instances, assigns, always blocks,
/// and header pragmas — are tracked separately.
pub struct MutableModule {
    /// The parsed module; its `ports`/`signals`/`parameters` are mutated
    /// in place as the upper-task instrumentation adds/removes them.
    pub inner: VerilogModule,
    /// Raw body text between port declarations and `endmodule`.
    pub body_text: String,
    /// New module instances to append to the body.
    added_instances: Vec<ModuleInstance>,
    /// New continuous assigns to append.
    added_assigns: Vec<ContinuousAssign>,
    /// New always blocks to append.
    added_always: Vec<AlwaysBlock>,
    /// New comment pragmas to prepend to the module header.
    added_comments: Vec<String>,
}

impl MutableModule {
    /// Create a mutable module from a parsed `VerilogModule`.
    ///
    /// Extracts the body text from the source by finding content after
    /// the last port/signal declaration and before `endmodule`.
    pub fn from_parsed(module: VerilogModule) -> Self {
        let body_text = extract_body_text(&module.source);
        Self {
            inner: module,
            body_text,
            added_instances: Vec::new(),
            added_assigns: Vec::new(),
            added_always: Vec::new(),
            added_comments: Vec::new(),
        }
    }

    /// Add a port, rejecting duplicates.
    pub fn add_port(&mut self, port: Port) -> Result<(), BuilderError> {
        if port.name.is_empty() {
            return Err(BuilderError::EmptyName);
        }
        if self.has_port(&port.name) {
            return Err(BuilderError::DuplicatePort(port.name));
        }
        self.inner.ports.push(port);
        Ok(())
    }

    /// Add a signal, rejecting duplicates.
    pub fn add_signal(&mut self, signal: Signal) -> Result<(), BuilderError> {
        if signal.name.is_empty() {
            return Err(BuilderError::EmptyName);
        }
        if self.has_signal(&signal.name) {
            return Err(BuilderError::DuplicateSignal(signal.name));
        }
        self.inner.signals.push(signal);
        Ok(())
    }

    /// Add a module instance to the body.
    pub fn add_instance(&mut self, instance: ModuleInstance) {
        self.added_instances.push(instance);
    }

    /// Add a continuous assign to the body.
    pub fn add_assign(&mut self, assign: ContinuousAssign) {
        self.added_assigns.push(assign);
    }

    /// Add an always block to the body.
    pub fn add_always(&mut self, always: AlwaysBlock) {
        self.added_always.push(always);
    }

    /// Add a comment pragma line at the module header.
    pub fn add_comment(&mut self, text: impl Into<String>) {
        self.added_comments.push(text.into());
    }

    /// Mark a port for removal by name.
    pub fn remove_port(&mut self, name: &str) {
        self.inner.ports.retain(|p| p.name != name);
    }

    /// Drop stale port-named reg declarations for output ports.
    ///
    /// Upper-task instrumentation replaces the original HLS body with
    /// child/FSM wiring. Output ports that were regs in the removed body
    /// are then driven by continuous assignments or submodule outputs and
    /// must be emitted as nets.
    pub fn demote_output_port_regs_to_wires(&mut self) {
        let output_ports: BTreeSet<_> = self
            .inner
            .ports
            .iter()
            .filter(|p| p.direction == Direction::Output)
            .map(|p| p.name.clone())
            .collect();
        self.inner
            .signals
            .retain(|s| !(s.kind == SignalKind::Reg && output_ports.contains(s.name.as_str())));
    }

    /// Convert selected parsed reg signals into wires.
    pub fn demote_signal_regs_to_wires(&mut self, names: &[&str]) {
        let names: BTreeSet<_> = names.iter().copied().collect();
        for signal in &mut self.inner.signals {
            if names.contains(signal.name.as_str()) && signal.kind == SignalKind::Reg {
                signal.kind = SignalKind::Wire;
            }
        }
    }

    /// Ensure a 1-bit signal exists with the requested kind.
    pub fn ensure_signal_kind(&mut self, name: &str, kind: SignalKind) {
        if let Some(signal) = self.inner.signals.iter_mut().find(|s| s.name == name) {
            signal.kind = kind;
            return;
        }
        self.inner.signals.push(Signal {
            name: name.to_owned(),
            kind,
            width: None,
        });
    }

    /// Clean up HLS-generated artifacts from the body text:
    /// - Remove `_regslice_both` instances
    /// - Remove `ap_ST_fsm_stateN_blk` signals
    /// - Remove FSM parameter declarations (`ap_ST_fsm_stateN`)
    /// - Remove `ap_CS_fsm` / `ap_NS_fsm` declarations and assignments
    /// - Remove `initial begin` blocks (power-on initialization)
    /// - Remove `ap_ce_reg` declarations
    /// - Remove HLS-internal inverted-reset declarations and assignments
    pub fn cleanup_hls_artifacts(&mut self) {
        // Use Cow to avoid allocation when pattern has no matches
        for pattern in [
            &*REGSLICE_PATTERN,
            &*AP_BLOCK_PATTERN,
            &*FSM_PARAM_PATTERN,
            &*CS_NS_FSM_PATTERN,
            &*INITIAL_BLOCK_PATTERN,
            &*AP_CE_PATTERN,
            &*RST_INV_PATTERN,
            &*AP_HANDSHAKE_ASSIGN_PATTERN,
        ] {
            let result = pattern.replace_all(&self.body_text, "");
            if let std::borrow::Cow::Owned(s) = result {
                self.body_text = s;
            }
        }

        // Also remove FSM-related signals from the parsed interface
        self.inner.signals.retain(|s| {
            !s.name.starts_with("ap_CS_fsm")
                && !s.name.starts_with("ap_NS_fsm")
                && !s.name.starts_with("ap_ST_fsm")
                && !s.name.contains("_blk")
                && s.name != "ap_ce_reg"
                && s.name != tapa_protocol::HLS_RST_INV
        });

        // Remove FSM-related parameters
        self.inner
            .parameters
            .retain(|p| !p.name.starts_with("ap_ST_fsm"));
    }

    /// Check if a port name exists.
    fn has_port(&self, name: &str) -> bool {
        self.inner.ports.iter().any(|p| p.name == name)
    }

    /// Check if a signal name exists.
    fn has_signal(&self, name: &str) -> bool {
        self.inner.signals.iter().any(|s| s.name == name)
    }

    /// Emit the complete module as Verilog text.
    pub fn emit(&self) -> String {
        let mut out = String::new();

        // Vivado accepts RapidStream pragmas as line comments. The attribute
        // form `(* RS ... *)` is invalid because `RS <tag>` is not an
        // attribute name.
        for comment in &self.added_comments {
            let _ = writeln!(out, "// pragma RS {comment}");
        }

        // Module declaration — emit parameters in the `#(parameter ...)`
        // header block BEFORE the port list so port widths that reference
        // a parameter (e.g. `[C_S_AXI_CONTROL_ADDR_WIDTH-1:0]`) are
        // resolvable at parse time. Vivado's HDL Parser rejects modules
        // that emit parameters inside the body after the port list.
        if self.inner.parameters.is_empty() {
            let _ = writeln!(out, "module {} (", self.inner.name);
        } else {
            let _ = writeln!(out, "module {} #(", self.inner.name);
            for (i, param) in self.inner.parameters.iter().enumerate() {
                let _ = write!(out, "  parameter ");
                if let Some(w) = &param.width {
                    let _ = write!(out, "{w} ");
                }
                let _ = write!(out, "{}", param.name);
                if !param.default.is_empty() {
                    let default_str = expression_source(&param.default);
                    let _ = write!(out, " = {default_str}");
                }
                let comma = if i + 1 < self.inner.parameters.len() {
                    ","
                } else {
                    ""
                };
                let _ = writeln!(out, "{comma}");
            }
            let _ = writeln!(out, ") (");
        }

        // Collect all ports (parsed + added are already unified on inner).
        let all_ports: Vec<&Port> = self.inner.ports.iter().collect();

        let signal_kinds: BTreeMap<_, _> = self
            .inner
            .signals
            .iter()
            .map(|s| (s.name.as_str(), s.kind))
            .collect();
        let port_names: BTreeSet<_> = all_ports.iter().map(|p| p.name.as_str()).collect();

        for (i, port) in all_ports.iter().enumerate() {
            let comma = if i + 1 < all_ports.len() { "," } else { "" };
            if port.direction == Direction::Output
                && signal_kinds.get(port.name.as_str()) == Some(&SignalKind::Reg)
            {
                let _ = write!(out, "  output reg");
                if let Some(w) = &port.width {
                    let _ = write!(out, " {w}");
                }
                let _ = writeln!(out, " {}{comma}", port.name);
            } else {
                let _ = writeln!(out, "  {port}{comma}");
            }
        }
        let _ = writeln!(out, ");");
        let _ = writeln!(out);

        // Signals (skipping any that shadow a port name).
        for sig in &self.inner.signals {
            if !port_names.contains(sig.name.as_str()) {
                let _ = writeln!(out, "{sig}");
            }
        }
        let _ = writeln!(out);

        // Original body text (may have been cleaned up)
        if !self.body_text.trim().is_empty() {
            let _ = writeln!(out, "{}", self.body_text.trim());
            let _ = writeln!(out);
        }

        // Added instances
        for inst in &self.added_instances {
            let _ = writeln!(out, "{inst}");
        }

        // Added assigns
        for assign in &self.added_assigns {
            let _ = writeln!(out, "{assign}");
        }

        // Added always blocks
        for always in &self.added_always {
            let _ = writeln!(out, "{always}");
        }

        let _ = writeln!(out, "endmodule //{}", self.inner.name);
        out
    }
}

/// Extract body text from raw Verilog source.
///
/// Everything after the last port/signal/parameter declaration (and
/// its terminating `);` if present) up to `endmodule`. The `);` line
/// that closes the ANSI port list is treated as part of the header,
/// not the body — otherwise re-emitted modules accumulate a stray
/// closing paren right before the signal declarations, tripping
/// Vivado's HDL parser.
fn extract_body_text(source: &str) -> String {
    let endmodule_pos = source.rfind("endmodule").unwrap_or(source.len());

    // Track byte offset alongside line iteration (O(n), not O(n²)).
    // Skip exactly the module header plus the contiguous declaration block.
    // After the first body statement, a bare `);` closes an instance, not
    // the module header.
    let mut body_start = 0;
    let mut byte_offset = 0;
    let mut header_closed = false;
    for line in source[..endmodule_pos].lines() {
        let line_end = byte_offset + line.len() + 1; // +1 for newline
        let trimmed = line.trim();

        if !header_closed {
            body_start = line_end.min(endmodule_pos);
            if trimmed == ");" || (trimmed.starts_with("module ") && trimmed.ends_with(");")) {
                header_closed = true;
            }
            byte_offset = line_end;
            continue;
        }

        let is_declaration_line = trimmed.is_empty()
            || starts_with_decl_keyword(trimmed, "input")
            || starts_with_decl_keyword(trimmed, "output")
            || starts_with_decl_keyword(trimmed, "inout")
            || starts_with_decl_keyword(trimmed, "wire")
            || starts_with_decl_keyword(trimmed, "reg")
            || starts_with_decl_keyword(trimmed, "parameter")
            || starts_with_decl_keyword(trimmed, "localparam")
            || trimmed.starts_with("(* ")
            // Line comments (incl. RapidStream `// pragma ...`) belong
            // to the header, not the body.
            || trimmed.starts_with("//");
        if is_declaration_line {
            body_start = line_end.min(endmodule_pos);
        } else {
            break;
        }
        byte_offset = line_end;
    }

    if body_start >= endmodule_pos {
        String::new()
    } else {
        source[body_start..endmodule_pos].to_owned()
    }
}

fn starts_with_decl_keyword(line: &str, keyword: &str) -> bool {
    let Some(rest) = line.strip_prefix(keyword) else {
        return false;
    };
    rest.chars()
        .next()
        .is_some_and(|c| c.is_ascii_whitespace() || c == '[')
}

/// Helper to create a simple 1-bit port.
pub fn simple_port(name: impl Into<String>, direction: Direction) -> Port {
    Port {
        name: name.into(),
        direction,
        width: None,
        pragma: None,
    }
}

/// Helper to create a port with width.
pub fn wide_port(name: impl Into<String>, direction: Direction, msb: &str, lsb: &str) -> Port {
    use crate::expression::tokenize_expression;
    Port {
        name: name.into(),
        direction,
        width: Some(Width {
            msb: tokenize_expression(msb),
            lsb: tokenize_expression(lsb),
        }),
        pragma: None,
    }
}

/// Helper to create a simple 1-bit wire.
pub fn wire(name: impl Into<String>) -> Signal {
    Signal {
        name: name.into(),
        kind: SignalKind::Wire,
        width: None,
    }
}

/// Helper to create a simple 1-bit reg.
pub fn reg(name: impl Into<String>) -> Signal {
    Signal {
        name: name.into(),
        kind: SignalKind::Reg,
        width: None,
    }
}

/// Helper to create a wide wire.
pub fn wide_wire(name: impl Into<String>, msb: &str, lsb: &str) -> Signal {
    use crate::expression::tokenize_expression;
    Signal {
        name: name.into(),
        kind: SignalKind::Wire,
        width: Some(Width {
            msb: tokenize_expression(msb),
            lsb: tokenize_expression(lsb),
        }),
    }
}

/// Helper to create a wide reg.
pub fn wide_reg(name: impl Into<String>, msb: &str, lsb: &str) -> Signal {
    use crate::expression::tokenize_expression;
    Signal {
        name: name.into(),
        kind: SignalKind::Reg,
        width: Some(Width {
            msb: tokenize_expression(msb),
            lsb: tokenize_expression(lsb),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::{Expr, PortArg};

    fn parse_fixture(name: &str) -> VerilogModule {
        let path = format!(
            "{}/testdata/rtl/{name}",
            env!("CARGO_MANIFEST_DIR").replace("/tapa-rtl", "")
        );
        let source =
            std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("failed to read {path}: {e}"));
        VerilogModule::parse(&source).unwrap_or_else(|e| panic!("failed to parse {path}: {e}"))
    }

    #[test]
    fn mutable_module_preserves_interface() {
        let module = parse_fixture("LowerLevelTask.v");
        assert!(!module.ports.is_empty(), "should have ports");
        let mm = MutableModule::from_parsed(module);
        assert!(!mm.inner.ports.is_empty());
        assert!(!mm.body_text.is_empty(), "should have body text");
    }

    #[test]
    fn add_port_rejects_duplicate() {
        let module = parse_fixture("LowerLevelTask.v");
        let mut mm = MutableModule::from_parsed(module);
        let result = mm.add_port(simple_port("ap_clk", Direction::Input));
        assert!(
            matches!(result, Err(BuilderError::DuplicatePort(_))),
            "should reject duplicate port, got: {result:?}"
        );
    }

    #[test]
    fn add_port_rejects_empty_name() {
        let module = parse_fixture("LowerLevelTask.v");
        let mut mm = MutableModule::from_parsed(module);
        let result = mm.add_port(simple_port("", Direction::Input));
        assert!(
            matches!(result, Err(BuilderError::EmptyName)),
            "should reject empty name"
        );
    }

    #[test]
    fn add_signal_rejects_duplicate() {
        let module = parse_fixture("LowerLevelTask.v");
        let mut mm = MutableModule::from_parsed(module);
        // ap_done is already a signal in LowerLevelTask.v
        let result = mm.add_signal(reg("ap_done"));
        assert!(
            matches!(result, Err(BuilderError::DuplicateSignal(_))),
            "should reject duplicate signal, got: {result:?}"
        );
    }

    #[test]
    fn emit_produces_valid_module() {
        let module = parse_fixture("LowerLevelTask.v");
        let mm = MutableModule::from_parsed(module);
        let emitted = mm.emit();
        assert!(emitted.contains("module LowerLevelTask"), "got:\n{emitted}");
        assert!(emitted.contains("endmodule"), "got:\n{emitted}");
        assert!(emitted.contains("ap_clk"), "should contain port ap_clk");
    }

    #[test]
    fn emit_preserves_instances_before_procedural_body() {
        let source = "
module HasAdapter (
    ap_clk,
    out
);
input ap_clk;
output out;
wire internal;

adapter #(
    .WIDTH(1))
adapter_U(
    .I(ap_clk),
    .O(internal)
);

always @(*) begin
end

assign out = internal;
endmodule
";
        let module = VerilogModule::parse(source).unwrap();
        let mm = MutableModule::from_parsed(module);
        let emitted = mm.emit();
        assert!(
            emitted.contains("adapter_U"),
            "body extraction must not drop instances closed by a bare `);`:\n{emitted}"
        );
        assert!(
            emitted.contains("assign out = internal;"),
            "body after the instance should also be preserved:\n{emitted}"
        );
    }

    #[test]
    fn emit_folds_duplicate_output_reg_port_declarations() {
        let source = "
module HlsStyle (
    ap_done,
    ap_ready
);
output ap_done;
output ap_ready;
reg ap_done;
wire ap_ready;

always @(*) begin
    ap_done = 1'b1;
end

assign ap_ready = ap_done;
endmodule
";
        let module = VerilogModule::parse(source).unwrap();
        let mm = MutableModule::from_parsed(module);
        let emitted = mm.emit();
        assert!(
            emitted.contains("output reg ap_done"),
            "procedurally assigned output reg should be folded into the ANSI port:\n{emitted}"
        );
        assert!(
            !emitted.contains("\nreg ap_done;"),
            "duplicate port-name reg declaration should be skipped:\n{emitted}"
        );
        assert!(
            !emitted.contains("\nwire ap_ready;"),
            "duplicate port-name wire declaration should be skipped:\n{emitted}"
        );
    }

    #[test]
    fn body_extraction_skips_compact_reg_declarations() {
        let source = "
module CompactHlsStyle (
    out
);
output out;
reg[3:0] out;
wire[3:0] tmp;

assign tmp = out;
endmodule
";
        let module = VerilogModule::parse(source).unwrap();
        let mm = MutableModule::from_parsed(module);

        assert!(
            !mm.body_text.contains("reg[3:0] out;"),
            "compact reg declarations are part of the declaration block, not the body"
        );
        assert!(
            !mm.body_text.contains("wire[3:0] tmp;"),
            "compact wire declarations are part of the declaration block, not the body"
        );
        assert!(
            mm.body_text.contains("assign tmp = out;"),
            "non-declaration body statements must still be preserved"
        );
    }

    #[test]
    fn emit_folds_added_output_reg_port_declarations() {
        let module = VerilogModule::parse("module Generated();\nendmodule").unwrap();
        let mut mm = MutableModule::from_parsed(module);
        mm.add_port(simple_port("child_ap_start", Direction::Output))
            .unwrap();
        mm.add_signal(reg("child_ap_start")).unwrap();
        let emitted = mm.emit();
        assert!(
            emitted.contains("output reg child_ap_start"),
            "added output reg should be folded into the ANSI port:\n{emitted}"
        );
        assert!(
            !emitted.contains("\nreg child_ap_start;"),
            "duplicate added reg declaration should be skipped:\n{emitted}"
        );
    }

    #[test]
    fn demote_output_port_regs_emits_net_ports() {
        let source = "
module Upper (
    out_read
);
output out_read;
reg out_read;
endmodule
";
        let module = VerilogModule::parse(source).unwrap();
        let mut mm = MutableModule::from_parsed(module);
        mm.demote_output_port_regs_to_wires();

        let emitted = mm.emit();
        assert!(
            emitted.contains("output wire out_read"),
            "demoted output should be emitted as a net:\n{emitted}"
        );
        assert!(
            !emitted.contains("output reg out_read"),
            "stale output reg should not remain:\n{emitted}"
        );
    }

    #[test]
    fn demote_signal_regs_to_wires_emits_net_signals() {
        let source = "
module VitisTop (
    ap_clk
);
input ap_clk;
reg ap_done;
reg ap_idle;
reg keep_reg;
endmodule
";
        let module = VerilogModule::parse(source).unwrap();
        let mut mm = MutableModule::from_parsed(module);
        mm.demote_signal_regs_to_wires(&["ap_done", "ap_idle"]);

        let emitted = mm.emit();
        assert!(emitted.contains("wire ap_done;"), "got:\n{emitted}");
        assert!(emitted.contains("wire ap_idle;"), "got:\n{emitted}");
        assert!(emitted.contains("reg keep_reg;"), "got:\n{emitted}");
    }

    #[test]
    fn ensure_signal_kind_promotes_existing_signal() {
        let src = "module Generated(output wire child_ap_start);\nwire child_ap_start;\nendmodule";
        let mut mm = MutableModule::from_parsed(VerilogModule::parse(src).unwrap());

        mm.ensure_signal_kind("child_ap_start", SignalKind::Reg);

        let emitted = mm.emit();
        assert!(
            emitted.contains("output reg child_ap_start"),
            "got:\n{emitted}"
        );
        assert!(
            !emitted.contains("\nreg child_ap_start;"),
            "port reg should not be redeclared:\n{emitted}"
        );
    }

    #[test]
    fn emit_with_additions() {
        let module = parse_fixture("LowerLevelTask.v");
        let mut mm = MutableModule::from_parsed(module);
        mm.add_port(simple_port("new_port", Direction::Output))
            .unwrap();
        mm.add_signal(wire("new_wire")).unwrap();
        mm.add_instance(
            ModuleInstance::new("sub_mod", "sub_inst")
                .with_ports(vec![PortArg::new("clk", Expr::ident("ap_clk"))]),
        );
        mm.add_assign(ContinuousAssign::new(
            Expr::ident("new_wire"),
            Expr::ident("ap_clk"),
        ));

        let emitted = mm.emit();
        assert!(emitted.contains("new_port"), "should contain added port");
        assert!(
            emitted.contains("wire new_wire;"),
            "should contain added signal"
        );
        assert!(
            emitted.contains("sub_mod sub_inst"),
            "should contain added instance"
        );
        assert!(
            emitted.contains("assign new_wire = ap_clk;"),
            "should contain added assign"
        );
    }

    #[test]
    fn cleanup_removes_hls_artifacts() {
        let module = parse_fixture("LowerLevelTask.v");
        let mut mm = MutableModule::from_parsed(module);
        let had_blk = mm.body_text.contains("ap_ST_fsm_state1_blk");
        mm.cleanup_hls_artifacts();
        if had_blk {
            assert!(
                !mm.body_text.contains("ap_ST_fsm_state1_blk"),
                "cleanup should remove _blk signals"
            );
        }
    }

    #[test]
    fn remove_port_excludes_from_declaration() {
        let module = parse_fixture("LowerLevelTask.v");
        let mut mm = MutableModule::from_parsed(module);
        mm.remove_port("ostreams_peek");
        let emitted = mm.emit();
        // Port should not appear in the port declaration list
        // (it may still appear in body text as raw assign references)
        let decl_section = emitted.split(");").next().unwrap_or("");
        assert!(
            !decl_section.contains("ostreams_peek"),
            "removed port should not appear in port declaration section"
        );
    }

    #[test]
    fn emit_then_reparse() {
        let module = parse_fixture("LowerLevelTask.v");
        let mut mm = MutableModule::from_parsed(module);
        mm.add_port(simple_port("test_out", Direction::Output))
            .unwrap();
        mm.add_signal(wire("test_wire")).unwrap();

        let emitted = mm.emit();
        let reparsed = VerilogModule::parse(&emitted);
        assert!(
            reparsed.is_ok(),
            "emitted module should reparse successfully, error: {:?}\nemitted:\n{emitted}",
            reparsed.err()
        );
        let reparsed = reparsed.unwrap();
        assert!(
            reparsed.find_port("test_out").is_some(),
            "should find added port"
        );
    }

    #[test]
    fn upper_level_task_parse_and_emit() {
        let module = parse_fixture("UpperLevelTask.v");
        let mut mm = MutableModule::from_parsed(module);
        mm.add_port(simple_port("extra_out", Direction::Output))
            .unwrap();
        let emitted = mm.emit();
        let reparsed = VerilogModule::parse(&emitted);
        assert!(
            reparsed.is_ok(),
            "UpperLevelTask emit should reparse, error: {:?}\nemitted:\n{emitted}",
            reparsed.err()
        );
    }

    #[test]
    fn cleanup_removes_fsm_params_and_signals() {
        let module = parse_fixture("LowerLevelTask.v");
        let mut mm = MutableModule::from_parsed(module);
        mm.cleanup_hls_artifacts();
        // FSM parameter should be removed
        assert!(
            !mm.inner
                .parameters
                .iter()
                .any(|p| p.name.starts_with("ap_ST_fsm")),
            "FSM parameters should be removed by cleanup"
        );
        // _blk signal should be removed
        assert!(
            !mm.inner.signals.iter().any(|s| s.name.contains("_blk")),
            "_blk signals should be removed by cleanup"
        );
        // ap_CS_fsm* should be removed
        assert!(
            !mm.inner
                .signals
                .iter()
                .any(|s| s.name.starts_with("ap_CS_fsm")),
            "ap_CS_fsm signals should be removed by cleanup"
        );
        // Body should not contain initial blocks
        assert!(
            !mm.body_text.contains("initial begin"),
            "initial blocks should be removed"
        );
    }

    /// Regression for a `vadd_xo` seed failure: Vivado's HDL
    /// Parser rejected the native top-level RTL because parameters
    /// like `C_S_AXI_CONTROL_ADDR_WIDTH` were emitted *after* the
    /// port list used them. `emit()` must now hoist every parameter
    /// into a `#(parameter ...)` header block before the port list,
    /// and the resulting module must still re-parse cleanly.
    #[test]
    fn emit_hoists_parameters_into_header_before_ports() {
        use crate::expression::tokenize_expression;
        use crate::param::Parameter;

        let module = parse_fixture("UpperLevelTask.v");
        let mut mm = MutableModule::from_parsed(module);
        mm.inner.parameters.push(Parameter {
            name: "C_S_AXI_CONTROL_ADDR_WIDTH".to_string(),
            default: tokenize_expression("6"),
            width: None,
        });
        mm.add_port(wide_port(
            "s_axi_control_AWADDR",
            Direction::Input,
            "C_S_AXI_CONTROL_ADDR_WIDTH-1",
            "0",
        ))
        .unwrap();

        let emitted = mm.emit();
        let header_idx = emitted.find("module ").expect("module keyword present");
        let param_idx = emitted
            .find("parameter C_S_AXI_CONTROL_ADDR_WIDTH")
            .expect("parameter must be emitted");
        let port_idx = emitted
            .find("s_axi_control_AWADDR")
            .expect("port using parameter must be emitted");
        assert!(
            param_idx > header_idx && param_idx < port_idx,
            "parameter must appear between `module` and the port that \
             references it; got param={param_idx} port={port_idx}:\n{emitted}",
        );
        assert!(
            emitted.contains("#("),
            "parameters must live in the ANSI `#(...)` header block:\n{emitted}",
        );

        // Round-trip: the emitted module must still re-parse cleanly.
        let reparsed = VerilogModule::parse(&emitted);
        assert!(
            reparsed.is_ok(),
            "hoisted-parameter emit must reparse, error: {:?}\nemitted:\n{emitted}",
            reparsed.err(),
        );
    }

    #[test]
    fn cleanup_then_emit_then_reparse() {
        let module = parse_fixture("LowerLevelTask.v");
        let mut mm = MutableModule::from_parsed(module);
        mm.cleanup_hls_artifacts();
        mm.add_port(simple_port("new_sig", Direction::Output))
            .unwrap();
        mm.add_signal(wire("added_wire")).unwrap();
        let emitted = mm.emit();
        let reparsed = VerilogModule::parse(&emitted);
        assert!(
            reparsed.is_ok(),
            "cleanup + mutate + emit should reparse, error: {:?}\nemitted:\n{emitted}",
            reparsed.err()
        );
        let reparsed = reparsed.unwrap();
        assert!(reparsed.find_port("new_sig").is_some());
        assert!(reparsed.signals.iter().any(|s| s.name == "added_wire"));
    }
}
