//! Per-instance child FSM blocks and their start/done assigns.

use crate::instance_signals::InstanceSignals;
use tapa_protocol::{HANDSHAKE_CLK, HANDSHAKE_RST, HANDSHAKE_START};
use tapa_rtl::builder::{AlwaysBlock, CaseItem, ContinuousAssign, Expr, Sensitivity, Statement};

// The FSM state encodings live in `crate::program` (the single
// FSM-encoding authority for both the global and child FSMs); re-exported
// here under the historical `STATE_*` names.
pub use crate::program::{
    CHILD_STATE_DONE as STATE_DONE, CHILD_STATE_IDLE as STATE_IDLE,
    CHILD_STATE_RUNNING as STATE_RUNNING, CHILD_STATE_WAITING as STATE_WAITING,
};

/// Generate the 4-state FSM always block for a non-autorun child instance.
///
/// States: IDLE(00) -> RUNNING(01) -> WAITING(11) or DONE(10) -> IDLE(00)
///
/// `start_input`: the signal that triggers IDLE->RUNNING transition.
/// `done_release`: the signal that releases `STATE_DONE` back to IDLE
/// (from the global done pipeline, so all children hold done until
/// the program FSM acknowledges completion).
pub fn generate_child_fsm(
    sig: &InstanceSignals,
    start_input: Expr,
    done_release: Expr,
) -> AlwaysBlock {
    let state = sig.state_expr();
    let done = sig.done_expr();
    let ready = Expr::ident(sig.ready_name());

    AlwaysBlock::posedge(
        HANDSHAKE_CLK,
        vec![Statement::If {
            cond: Expr::ident(HANDSHAKE_RST),
            then_body: vec![sig.set_state(Expr::lit(STATE_IDLE))],
            else_body: vec![Statement::Case {
                expr: state,
                items: vec![
                    // IDLE -> RUNNING when global start pipeline asserts
                    CaseItem::new(
                        Expr::lit(STATE_IDLE),
                        vec![Statement::If {
                            cond: start_input,
                            then_body: vec![sig.set_state(Expr::lit(STATE_RUNNING))],
                            else_body: vec![],
                        }],
                    ),
                    // RUNNING -> DONE if ready&done, WAITING if ready&!done
                    CaseItem::new(
                        Expr::lit(STATE_RUNNING),
                        vec![Statement::If {
                            cond: Expr::logical_and(ready.clone(), done.clone()),
                            then_body: vec![sig.set_state(Expr::lit(STATE_DONE))],
                            else_body: vec![Statement::If {
                                cond: ready,
                                then_body: vec![sig.set_state(Expr::lit(STATE_WAITING))],
                                else_body: vec![],
                            }],
                        }],
                    ),
                    // WAITING -> DONE when done
                    CaseItem::new(
                        Expr::lit(STATE_WAITING),
                        vec![Statement::If {
                            cond: done,
                            then_body: vec![sig.set_state(Expr::lit(STATE_DONE))],
                            else_body: vec![],
                        }],
                    ),
                    // DONE -> IDLE only when global done pipeline releases
                    CaseItem::new(
                        Expr::lit(STATE_DONE),
                        vec![Statement::If {
                            cond: done_release,
                            then_body: vec![sig.set_state(Expr::lit(STATE_IDLE))],
                            else_body: vec![],
                        }],
                    ),
                ],
                default: vec![sig.set_state(Expr::lit(STATE_IDLE))],
            }],
        }],
    )
}

/// Generate an `__is_done` assign inside the FSM module.
///
/// `assign is_done = (state == STATE_DONE)`
pub fn generate_is_done_assign(sig: &InstanceSignals) -> ContinuousAssign {
    ContinuousAssign::new(
        Expr::ident(sig.is_done_name()),
        sig.is_state(Expr::lit(STATE_DONE)),
    )
}

/// Generate the start logic for an autorun child instance.
///
/// Autorun instances latch their start signal when the global `ap_start`
/// is first asserted and keep it high until reset.
pub fn generate_autorun_start(sig: &InstanceSignals) -> AlwaysBlock {
    AlwaysBlock::new(
        Sensitivity::Posedge(HANDSHAKE_CLK.into()),
        vec![Statement::If {
            cond: Expr::ident(HANDSHAKE_RST),
            then_body: vec![Statement::NonblockingAssign {
                lhs: sig.start_expr(),
                rhs: Expr::lit("1'b0"),
            }],
            else_body: vec![Statement::If {
                cond: Expr::ident(HANDSHAKE_START),
                then_body: vec![Statement::NonblockingAssign {
                    lhs: sig.start_expr(),
                    rhs: Expr::lit("1'b1"),
                }],
                else_body: vec![],
            }],
        }],
    )
}

/// Generate the combinational start assign for a non-autorun instance.
///
/// `instance_start = (state == STATE_RUNNING)`
pub fn generate_start_assign(sig: &InstanceSignals) -> ContinuousAssign {
    ContinuousAssign::new(sig.start_expr(), sig.is_state(Expr::lit(STATE_RUNNING)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn child_fsm_has_four_states_with_done_hold() {
        let sig = InstanceSignals::new("child_0", false);
        let start_input = Expr::ident("global_start_q");
        let done_release = Expr::ident("global_done_q");
        let block = generate_child_fsm(&sig, start_input, done_release);
        let text = block.to_string();
        assert!(text.contains("case (child_0__state)"), "got:\n{text}");
        assert!(text.contains(STATE_IDLE), "got:\n{text}");
        assert!(text.contains(STATE_RUNNING), "got:\n{text}");
        assert!(text.contains(STATE_DONE), "got:\n{text}");
        // IDLE->RUNNING uses global_start_q
        assert!(
            text.contains("global_start_q"),
            "should use start input, got:\n{text}"
        );
        // DONE->IDLE gated by global_done_q (not unconditional)
        assert!(
            text.contains("global_done_q"),
            "DONE->IDLE should be gated by done_release, got:\n{text}"
        );
    }

    #[test]
    fn autorun_start_latches_until_reset() {
        let sig = InstanceSignals::new("auto_inst", true);
        let block = generate_autorun_start(&sig);
        let text = block.to_string();
        assert!(
            text.contains("if (ap_start)")
                && text.contains("auto_inst__ap_start <= 1'b1")
                && text.contains("auto_inst__ap_start <= 1'b0"),
            "got:\n{text}"
        );
        assert!(
            !text.contains("auto_inst__ap_start <= ap_start"),
            "autorun start must not deassert with the host start pulse:\n{text}"
        );
    }

    #[test]
    fn start_assign_checks_running_state() {
        let sig = InstanceSignals::new("child_0", false);
        let assign = generate_start_assign(&sig);
        let text = assign.to_string();
        assert!(
            text.contains("child_0__ap_start") && text.contains(STATE_RUNNING),
            "got: {text}"
        );
    }
}
