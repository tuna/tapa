//! Per-instance child FSM blocks and their start/done assigns.

use crate::instance_signals::InstanceSignals;
use tapa_protocol::{HANDSHAKE_CLK, HANDSHAKE_RST};
use tapa_rtl::builder::{AlwaysBlock, CaseItem, ContinuousAssign, Expr, Statement};

use crate::program::{
    CHILD_STATE_DONE, CHILD_STATE_IDLE, CHILD_STATE_RUNNING, CHILD_STATE_WAITING,
};

/// Generate the 4-state FSM always block for a non-autorun child instance.
///
/// States: IDLE(00) -> RUNNING(01) -> WAITING(11) or DONE(10) -> IDLE(00)
///
/// `start_input`: the signal that triggers IDLE->RUNNING transition.
/// `done_release`: the signal that releases `CHILD_STATE_DONE` back to IDLE
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
            then_body: vec![sig.set_state(Expr::lit(CHILD_STATE_IDLE))],
            else_body: vec![Statement::Case {
                expr: state,
                items: vec![
                    // IDLE -> RUNNING when global start pipeline asserts
                    CaseItem::new(
                        Expr::lit(CHILD_STATE_IDLE),
                        vec![Statement::If {
                            cond: start_input,
                            then_body: vec![sig.set_state(Expr::lit(CHILD_STATE_RUNNING))],
                            else_body: vec![],
                        }],
                    ),
                    // RUNNING -> DONE if ready&done, WAITING if ready&!done
                    CaseItem::new(
                        Expr::lit(CHILD_STATE_RUNNING),
                        vec![Statement::If {
                            cond: Expr::logical_and(ready.clone(), done.clone()),
                            then_body: vec![sig.set_state(Expr::lit(CHILD_STATE_DONE))],
                            else_body: vec![Statement::If {
                                cond: ready,
                                then_body: vec![sig.set_state(Expr::lit(CHILD_STATE_WAITING))],
                                else_body: vec![],
                            }],
                        }],
                    ),
                    // WAITING -> DONE when done
                    CaseItem::new(
                        Expr::lit(CHILD_STATE_WAITING),
                        vec![Statement::If {
                            cond: done,
                            then_body: vec![sig.set_state(Expr::lit(CHILD_STATE_DONE))],
                            else_body: vec![],
                        }],
                    ),
                    // DONE -> IDLE only when global done pipeline releases
                    CaseItem::new(
                        Expr::lit(CHILD_STATE_DONE),
                        vec![Statement::If {
                            cond: done_release,
                            then_body: vec![sig.set_state(Expr::lit(CHILD_STATE_IDLE))],
                            else_body: vec![],
                        }],
                    ),
                ],
                default: vec![sig.set_state(Expr::lit(CHILD_STATE_IDLE))],
            }],
        }],
    )
}

/// Generate an `__is_done` assign inside the FSM module.
///
/// `assign is_done = (state == CHILD_STATE_DONE)`
pub fn generate_is_done_assign(sig: &InstanceSignals) -> ContinuousAssign {
    ContinuousAssign::new(
        Expr::ident(sig.is_done_name()),
        sig.is_state(Expr::lit(CHILD_STATE_DONE)),
    )
}

/// Generate the start logic for an autorun (detached) child instance.
///
/// A detached child starts once per parent invocation: `launch` is high for
/// the one cycle the parent accepts its start, which raises the child's
/// start and holds it until the child takes it with `ap_ready`. A launch
/// that arrives while the start is still held is queued in
/// `{instance}__ap_start_pending` and raises the start again right after
/// that handshake, so a finite child still running from the last
/// invocation runs once more rather than missing this one. One launch
/// queues at most: a further launch before the handshake merges with it. A
/// child that never returns never takes its start, which then stays high
/// until reset.
pub fn generate_autorun_start(sig: &InstanceSignals, launch: Expr) -> AlwaysBlock {
    let start = sig.start_expr();
    let pending = Expr::ident(sig.start_pending_name());
    let assign = |lhs: &Expr, rhs: Expr| Statement::NonblockingAssign {
        lhs: lhs.clone(),
        rhs,
    };
    AlwaysBlock::posedge(
        HANDSHAKE_CLK,
        vec![Statement::If {
            cond: Expr::ident(HANDSHAKE_RST),
            then_body: vec![
                assign(&start, Expr::lit("1'b0")),
                assign(&pending, Expr::lit("1'b0")),
            ],
            else_body: vec![Statement::If {
                // The child takes the held start: a launch in the same cycle
                // keeps it held, else a queued launch does.
                cond: Expr::logical_and(start.clone(), Expr::ident(sig.ready_name())),
                then_body: vec![Statement::If {
                    cond: launch.clone(),
                    then_body: vec![assign(&start, Expr::lit("1'b1"))],
                    else_body: vec![
                        assign(&start, pending.clone()),
                        assign(&pending, Expr::lit("1'b0")),
                    ],
                }],
                else_body: vec![Statement::If {
                    cond: launch,
                    then_body: vec![
                        assign(&start, Expr::lit("1'b1")),
                        assign(&pending, start.clone()),
                    ],
                    else_body: vec![],
                }],
            }],
        }],
    )
}

/// Generate the combinational start assign for a non-autorun instance.
///
/// `instance_start = (state == CHILD_STATE_RUNNING)`
pub fn generate_start_assign(sig: &InstanceSignals) -> ContinuousAssign {
    ContinuousAssign::new(
        sig.start_expr(),
        sig.is_state(Expr::lit(CHILD_STATE_RUNNING)),
    )
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
        assert!(text.contains(CHILD_STATE_IDLE), "got:\n{text}");
        assert!(text.contains(CHILD_STATE_RUNNING), "got:\n{text}");
        assert!(text.contains(CHILD_STATE_DONE), "got:\n{text}");
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
    fn autorun_start_is_held_until_ready_and_queues_one_launch() {
        let sig = InstanceSignals::new("auto_inst", true);
        let block = generate_autorun_start(&sig, Expr::ident("launch"));
        let text = block.to_string();
        for needle in [
            "auto_inst__ap_start <= 1'b0",
            "auto_inst__ap_start_pending <= 1'b0",
            "if ((auto_inst__ap_start && auto_inst__ap_ready))",
            "if (launch)",
            "auto_inst__ap_start <= 1'b1",
            "auto_inst__ap_start <= auto_inst__ap_start_pending",
            "auto_inst__ap_start_pending <= auto_inst__ap_start",
        ] {
            assert!(text.contains(needle), "missing `{needle}`:\n{text}");
        }
        assert!(
            !text.contains("if (ap_start)"),
            "a detached start follows the parent's launch, not its held start:\n{text}"
        );
    }

    #[test]
    fn start_assign_checks_running_state() {
        let sig = InstanceSignals::new("child_0", false);
        let assign = generate_start_assign(&sig);
        let text = assign.to_string();
        assert!(
            text.contains("child_0__ap_start") && text.contains(CHILD_STATE_RUNNING),
            "got: {text}"
        );
    }
}
