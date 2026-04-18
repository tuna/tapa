//! Top-level program RTL assembly and global FSM.
//!
//! Ports `tapa/program_codegen/program.py`: global FSM generation,
//! upper-task orchestration, template task output.

use tapa_protocol::{HANDSHAKE_DONE, HANDSHAKE_IDLE, HANDSHAKE_READY};
use tapa_rtl::builder::{
    AlwaysBlock, CaseItem, ContinuousAssign, Expr, Statement,
};
use tapa_rtl::mutation::{wide_reg, wire, MutableModule};

/// FSM state constants for the global program FSM (2-bit).
pub const GLOBAL_STATE_IDLE: &str = "2'b00";
pub const GLOBAL_STATE_RUNNING: &str = "2'b01";
pub const GLOBAL_STATE_DONE: &str = "2'b10";

/// Generate the global 3-state FSM for a top-level program.
///
/// States: IDLE(00) -> RUNNING(01) -> DONE(10) -> IDLE(00)
/// Transition from RUNNING to DONE is gated by all child `is_done` signals.
///
/// Returns the always block for the FSM, plus continuous assigns for
/// handshake output signals.
pub fn generate_global_fsm(
    is_done_signal_names: &[String],
) -> GlobalFsmOutput {
    let state = Expr::ident("__tapa_state");

    // Build the RUNNING->DONE transition condition
    let all_done_cond = if is_done_signal_names.is_empty() {
        Expr::lit("1'b1") // No children = always done
    } else {
        is_done_signal_names
            .iter()
            .map(|name| Expr::ident(name.clone()))
            .reduce(Expr::logical_and)
            .unwrap_or_else(|| Expr::lit("1'b1"))
    };

    // FSM always block
    let fsm_block = AlwaysBlock::posedge(
        "ap_clk",
        vec![Statement::If {
            cond: Expr::ident("ap_rst"),
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
                            cond: Expr::ident("ap_start"),
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
    );

    // Handshake output assigns
    let idle_assign = ContinuousAssign::new(
        Expr::ident(HANDSHAKE_IDLE),
        Expr::eq(
            Expr::ident("__tapa_state"),
            Expr::lit(GLOBAL_STATE_IDLE),
        ),
    );

    GlobalFsmOutput {
        fsm_block,
        idle_assign,
    }
}

/// Output of global FSM generation.
pub struct GlobalFsmOutput {
    pub fsm_block: AlwaysBlock,
    pub idle_assign: ContinuousAssign,
}

/// Pipeline signal names for start and done.
pub const START_Q: &str = "__tapa_start_q";
pub const DONE_Q: &str = "__tapa_done_q";

/// Apply global FSM signals and logic to an FSM module.
///
/// Creates `start_q` and `done_q` pipeline signals:
/// - `start_q[0] = ap_start` (immediate from handshake)
/// - `done_q[0] = (state == STATE_DONE)` (combinational from FSM)
/// - `ap_idle = (state == STATE_IDLE)`
/// - `ap_done = done_q` (pipelined output)
/// - `ap_ready = done_q` (immediate completion signal)
///
/// Children use `start_q` for IDLE->RUNNING and `done_q` for DONE->IDLE.
pub fn apply_global_fsm(
    fsm_module: &mut MutableModule,
    is_done_signal_names: &[String],
) {
    // Add top-level handshake ports to FSM module interface
    let _ = fsm_module.add_port(tapa_rtl::mutation::simple_port(
        "ap_start",
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
    let _ = fsm_module.add_signal(wire("ap_rst"));

    // Add pipeline signals
    let _ = fsm_module.add_signal(wire(START_Q));
    let _ = fsm_module.add_signal(wire(DONE_Q));

    // assign ap_rst = ~ap_rst_n
    fsm_module.add_assign(ContinuousAssign::new(
        Expr::ident("ap_rst"),
        Expr::logical_not(Expr::ident("ap_rst_n")),
    ));

    // start_q = ap_start (pipeline stage 0 = raw input)
    fsm_module.add_assign(ContinuousAssign::new(
        Expr::ident(START_Q),
        Expr::ident("ap_start"),
    ));

    // Generate and apply FSM
    let output = generate_global_fsm(is_done_signal_names);
    fsm_module.add_always(output.fsm_block);
    fsm_module.add_assign(output.idle_assign);

    // done_q = (state == STATE_DONE) — combinational from FSM
    fsm_module.add_assign(ContinuousAssign::new(
        Expr::ident(DONE_Q),
        Expr::eq(
            Expr::ident("__tapa_state"),
            Expr::lit(GLOBAL_STATE_DONE),
        ),
    ));

    // ap_done driven from done_q (pipelined output)
    fsm_module.add_assign(ContinuousAssign::new(
        Expr::ident(HANDSHAKE_DONE),
        Expr::ident(DONE_Q),
    ));
    // ap_ready driven from done_q (immediate)
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
    fn global_fsm_has_three_states() {
        let output = generate_global_fsm(&["child_0__is_done".into()]);
        let text = output.fsm_block.to_string();
        assert!(text.contains(GLOBAL_STATE_IDLE), "got:\n{text}");
        assert!(text.contains(GLOBAL_STATE_RUNNING), "got:\n{text}");
        assert!(text.contains(GLOBAL_STATE_DONE), "got:\n{text}");
    }

    #[test]
    fn global_fsm_gates_on_is_done() {
        let output = generate_global_fsm(&[
            "child_a__is_done".into(),
            "child_b__is_done".into(),
        ]);
        let text = output.fsm_block.to_string();
        assert!(text.contains("child_a__is_done"), "got:\n{text}");
        assert!(text.contains("child_b__is_done"), "got:\n{text}");
    }

    #[test]
    fn global_fsm_idle_assign() {
        let output = generate_global_fsm(&[]);
        let text = output.idle_assign.to_string();
        assert!(text.contains("ap_idle"), "got: {text}");
        assert!(text.contains(GLOBAL_STATE_IDLE), "got: {text}");
    }

    #[test]
    fn apply_global_fsm_to_module() {
        let source = "module top_fsm (\n  input wire ap_clk,\n  input wire ap_rst_n\n);\nendmodule";
        let module = VerilogModule::parse(source).unwrap();
        let mut mm = MutableModule::from_parsed(module);
        apply_global_fsm(&mut mm, &["child__is_done".into()]);
        let emitted = mm.emit();
        assert!(emitted.contains("__tapa_state"), "got:\n{emitted}");
        assert!(emitted.contains("ap_idle"), "got:\n{emitted}");
        assert!(emitted.contains("always @(posedge ap_clk)"), "got:\n{emitted}");
        // Pipeline signals
        assert!(emitted.contains(START_Q), "should have start_q pipeline, got:\n{emitted}");
        assert!(emitted.contains(DONE_Q), "should have done_q pipeline, got:\n{emitted}");
        // ap_done driven from done_q, NOT directly from state
        assert!(
            emitted.contains(&format!("assign {HANDSHAKE_DONE} = {DONE_Q}")),
            "ap_done should be driven from done_q, got:\n{emitted}"
        );
    }

    #[test]
    fn no_children_always_done() {
        let output = generate_global_fsm(&[]);
        let text = output.fsm_block.to_string();
        // With no children, should transition immediately from RUNNING to DONE
        assert!(text.contains("1'b1"), "should have unconditional done, got:\n{text}");
    }
}
