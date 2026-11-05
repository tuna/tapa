//! FSM state encodings and the top-level global FSM for the design root.
//!
//! This module is the single authority for the state encodings of BOTH
//! FSM families in the generated RTL:
//!
//! - `GLOBAL_STATE_*`: the 3-state global program FSM, generated here as
//!   the IDLE -> RUNNING -> DONE controller that drives the ap-ctrl
//!   handshake and gates on all child `is_done` signals.
//! - `CHILD_STATE_*`: the 4-state per-instance child FSM, consumed by
//!   `crate::passes::children::fsm`, which re-exports them under the
//!   historical `STATE_*` names.

use tapa_protocol::{
    HANDSHAKE_CLK, HANDSHAKE_DONE, HANDSHAKE_IDLE, HANDSHAKE_READY, HANDSHAKE_RST, HANDSHAKE_RST_N,
    HANDSHAKE_START,
};
use tapa_rtl::builder::{AlwaysBlock, CaseItem, ContinuousAssign, Expr, Statement};
use tapa_rtl::mutation::{wide_reg, wire, MutableModule};

use crate::passes::logical_reduction;

/// FSM state constants for the global program FSM (2-bit).
pub const GLOBAL_STATE_IDLE: &str = "2'b00";
pub const GLOBAL_STATE_RUNNING: &str = "2'b01";
pub const GLOBAL_STATE_DONE: &str = "2'b10";

/// FSM state constants for the per-instance child (non-autorun) FSM
/// (2-bit encoding).
///
/// IDLE (`2'b00`), RUNNING (`2'b01`), and DONE (`2'b10`) hold the same
/// values as [`GLOBAL_STATE_IDLE`], [`GLOBAL_STATE_RUNNING`], and
/// [`GLOBAL_STATE_DONE`] — a uniformity for waveform inspectability, not a
/// correctness requirement: the two FSMs compose through the `is_done` /
/// `start_q` / `done_q` nets, never through encoding equality. WAITING
/// (`2'b11`) exists only in the child FSM — a non-autorun child parks
/// there when `ready` arrives before `done`; the global FSM has no fourth
/// state.
pub const CHILD_STATE_IDLE: &str = "2'b00";
pub const CHILD_STATE_RUNNING: &str = "2'b01";
pub const CHILD_STATE_WAITING: &str = "2'b11";
pub const CHILD_STATE_DONE: &str = "2'b10";

/// Wire names for the start and done handshake nets.
pub const START_Q: &str = "__tapa_start_q";
pub const DONE_Q: &str = "__tapa_done_q";

/// Apply global FSM signals and logic to an FSM module.
///
/// The global FSM is a 3-state controller: IDLE(00) -> RUNNING(01) ->
/// DONE(10) -> IDLE(00). The RUNNING -> DONE transition is gated by all
/// child `is_done` signals; with no children it is unconditional.
///
/// Creates the `start_q` and `done_q` wires:
/// - `start_q = ap_start` (immediate from handshake)
/// - `done_q = (state == STATE_DONE)` (combinational from FSM)
/// - `ap_idle = (state == STATE_IDLE)`
/// - `ap_done = done_q`
/// - `ap_ready = done_q`
///
/// Children use `start_q` for IDLE->RUNNING and `done_q` for DONE->IDLE.
pub fn apply_global_fsm(fsm_module: &mut MutableModule, is_done_signal_names: &[String]) {
    // Add top-level handshake ports to FSM module interface
    let _ = fsm_module.add_port(tapa_rtl::mutation::simple_port(
        HANDSHAKE_START,
        tapa_rtl::port::Direction::Input,
    ));
    let _ = fsm_module.add_port(tapa_rtl::mutation::simple_port(
        HANDSHAKE_DONE,
        tapa_rtl::port::Direction::Output,
    ));
    let _ = fsm_module.add_port(tapa_rtl::mutation::simple_port(
        HANDSHAKE_IDLE,
        tapa_rtl::port::Direction::Output,
    ));
    let _ = fsm_module.add_port(tapa_rtl::mutation::simple_port(
        HANDSHAKE_READY,
        tapa_rtl::port::Direction::Output,
    ));

    // Add state register
    let _ = fsm_module.add_signal(wide_reg("__tapa_state", "1", "0"));

    // Add reset wire
    let _ = fsm_module.add_signal(wire(HANDSHAKE_RST));

    // Add start/done wires
    let _ = fsm_module.add_signal(wire(START_Q));
    let _ = fsm_module.add_signal(wire(DONE_Q));

    // assign ap_rst = ~ap_rst_n
    fsm_module.add_assign(ContinuousAssign::new(
        Expr::ident(HANDSHAKE_RST),
        Expr::logical_not(Expr::ident(HANDSHAKE_RST_N)),
    ));

    // start_q = ap_start (raw handshake input)
    fsm_module.add_assign(ContinuousAssign::new(
        Expr::ident(START_Q),
        Expr::ident(HANDSHAKE_START),
    ));

    // Build the RUNNING->DONE transition condition; no children = always done.
    let state = Expr::ident("__tapa_state");
    let all_done_cond = logical_reduction(is_done_signal_names.iter().map(Expr::ident));

    // FSM always block
    fsm_module.add_always(AlwaysBlock::posedge(
        HANDSHAKE_CLK,
        vec![Statement::If {
            cond: Expr::ident(HANDSHAKE_RST),
            then_body: vec![Statement::NonblockingAssign {
                lhs: state.clone(),
                rhs: Expr::lit(GLOBAL_STATE_IDLE),
            }],
            else_body: vec![Statement::Case {
                expr: state.clone(),
                items: vec![
                    // IDLE -> RUNNING on start
                    CaseItem::new(
                        Expr::lit(GLOBAL_STATE_IDLE),
                        vec![Statement::If {
                            cond: Expr::ident(HANDSHAKE_START),
                            then_body: vec![Statement::NonblockingAssign {
                                lhs: state.clone(),
                                rhs: Expr::lit(GLOBAL_STATE_RUNNING),
                            }],
                            else_body: vec![],
                        }],
                    ),
                    // RUNNING -> DONE when all children done
                    CaseItem::new(
                        Expr::lit(GLOBAL_STATE_RUNNING),
                        vec![Statement::If {
                            cond: all_done_cond,
                            then_body: vec![Statement::NonblockingAssign {
                                lhs: state.clone(),
                                rhs: Expr::lit(GLOBAL_STATE_DONE),
                            }],
                            else_body: vec![],
                        }],
                    ),
                    // DONE -> IDLE (unconditional)
                    CaseItem::new(
                        Expr::lit(GLOBAL_STATE_DONE),
                        vec![Statement::NonblockingAssign {
                            lhs: state,
                            rhs: Expr::lit(GLOBAL_STATE_IDLE),
                        }],
                    ),
                ],
                default: vec![],
            }],
        }],
    ));

    // ap_idle = (state == STATE_IDLE)
    fsm_module.add_assign(ContinuousAssign::new(
        Expr::ident(HANDSHAKE_IDLE),
        Expr::eq(Expr::ident("__tapa_state"), Expr::lit(GLOBAL_STATE_IDLE)),
    ));

    // done_q = (state == STATE_DONE) — combinational from FSM
    fsm_module.add_assign(ContinuousAssign::new(
        Expr::ident(DONE_Q),
        Expr::eq(Expr::ident("__tapa_state"), Expr::lit(GLOBAL_STATE_DONE)),
    ));

    // ap_done driven from done_q
    fsm_module.add_assign(ContinuousAssign::new(
        Expr::ident(HANDSHAKE_DONE),
        Expr::ident(DONE_Q),
    ));
    // ap_ready driven from done_q
    fsm_module.add_assign(ContinuousAssign::new(
        Expr::ident(HANDSHAKE_READY),
        Expr::ident(DONE_Q),
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    use tapa_rtl::VerilogModule;

    #[test]
    fn apply_global_fsm_to_module() {
        let source = "module top_fsm (\n  input wire ap_clk,\n  input wire ap_rst_n\n);\nendmodule";
        let module = VerilogModule::parse(source).unwrap();
        let mut mm = MutableModule::from_parsed(module);
        apply_global_fsm(
            &mut mm,
            &["child_a__is_done".into(), "child_b__is_done".into()],
        );
        let emitted = mm.emit();
        assert!(emitted.contains("__tapa_state"), "got:\n{emitted}");
        // All three FSM state encodings appear.
        assert!(emitted.contains(GLOBAL_STATE_IDLE), "got:\n{emitted}");
        assert!(emitted.contains(GLOBAL_STATE_RUNNING), "got:\n{emitted}");
        assert!(emitted.contains(GLOBAL_STATE_DONE), "got:\n{emitted}");
        // RUNNING -> DONE gates on every child is_done.
        assert!(emitted.contains("child_a__is_done"), "got:\n{emitted}");
        assert!(emitted.contains("child_b__is_done"), "got:\n{emitted}");
        // ap_idle = (state == STATE_IDLE)
        assert!(emitted.contains("ap_idle"), "got:\n{emitted}");
        assert!(
            emitted.contains("always @(posedge ap_clk)"),
            "got:\n{emitted}"
        );
        // Start/done wires
        assert!(
            emitted.contains(START_Q),
            "should have start_q wire, got:\n{emitted}"
        );
        assert!(
            emitted.contains(DONE_Q),
            "should have done_q wire, got:\n{emitted}"
        );
        // ap_done driven from done_q, NOT directly from state
        assert!(
            emitted.contains(&format!("assign {HANDSHAKE_DONE} = {DONE_Q}")),
            "ap_done should be driven from done_q, got:\n{emitted}"
        );

        // With no children, RUNNING -> DONE is unconditional.
        let module = VerilogModule::parse(source).unwrap();
        let mut mm = MutableModule::from_parsed(module);
        apply_global_fsm(&mut mm, &[]);
        let emitted = mm.emit();
        assert!(
            emitted.contains("1'b1"),
            "should have unconditional done, got:\n{emitted}"
        );
    }
}
