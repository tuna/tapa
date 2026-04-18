//! Child task instantiation with FSM/port wiring.
//!
//! Ports `tapa/program_codegen/children.py`: handles per-instance FSM
//! generation, argument pipelines, handshake wiring, and portarg assembly.

use tapa_protocol::{ISTREAM_SUFFIXES, OSTREAM_SUFFIXES};
use tapa_rtl::builder::{
    AlwaysBlock, CaseItem, ContinuousAssign, Expr, ModuleInstance, PortArg, Sensitivity, Statement,
};
use tapa_task_graph::port::ArgCategory;
use tapa_topology::instance::ArgDesign;

use crate::instance_signals::InstanceSignals;

/// FSM state constants for non-autorun child instances (2-bit encoding).
pub const STATE_IDLE: &str = "2'b00";
pub const STATE_RUNNING: &str = "2'b01";
pub const STATE_WAITING: &str = "2'b11";
pub const STATE_DONE: &str = "2'b10";

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
        "ap_clk",
        vec![Statement::If {
            cond: Expr::ident("ap_rst"),
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
/// Autorun instances start when the global `ap_start` is asserted.
pub fn generate_autorun_start(sig: &InstanceSignals) -> AlwaysBlock {
    AlwaysBlock::new(
        Sensitivity::Posedge("ap_clk".into()),
        vec![Statement::If {
            cond: Expr::ident("ap_rst"),
            then_body: vec![Statement::NonblockingAssign {
                lhs: sig.start_expr(),
                rhs: Expr::lit("1'b0"),
            }],
            else_body: vec![Statement::NonblockingAssign {
                lhs: sig.start_expr(),
                rhs: Expr::ident("ap_start"),
            }],
        }],
    )
}

/// Generate the combinational start assign for a non-autorun instance.
///
/// `instance_start = (state == STATE_RUNNING)`
pub fn generate_start_assign(sig: &InstanceSignals) -> ContinuousAssign {
    ContinuousAssign::new(
        sig.start_expr(),
        sig.is_state(Expr::lit(STATE_RUNNING)),
    )
}

/// Build a child task `ModuleInstance` with all port argument bindings.
///
/// Connects handshake signals (from `InstanceSignals`), scalar arguments,
/// stream arguments (istream/ostream suffixes), and mmap offset arguments.
///
/// `mmap_slave_indices`: maps parent mmap arg name → crossbar slave index.
/// When present, child M-AXI ports bind to `m_axi_{arg}_{idx}_*` (downstream
/// crossbar wires) instead of `m_axi_{arg}_*` (upstream parent ports).
pub fn build_child_instance(
    child_task_name: &str,
    instance_name: &str,
    sig: &InstanceSignals,
    args: &std::collections::BTreeMap<String, ArgDesign>,
    mmap_slave_indices: &std::collections::BTreeMap<String, usize>,
) -> ModuleInstance {
    let mut port_args = Vec::new();

    // Clock and reset
    port_args.push(PortArg::new("ap_clk", Expr::ident("ap_clk")));
    port_args.push(PortArg::new("ap_rst_n", Expr::ident("ap_rst_n")));

    // Handshake signals from InstanceSignals
    port_args.extend(sig.instance_portargs());

    // Argument port bindings
    for (child_port, arg) in args {
        match arg.cat {
            ArgCategory::Scalar => {
                // Scalar: connect to per-instance pipeline signal
                let pipeline_name = format!("{instance_name}__{child_port}");
                port_args.push(PortArg::new(
                    child_port.as_str(),
                    Expr::ident(pipeline_name),
                ));
            }
            ArgCategory::Istream => {
                // Input stream: connect with ISTREAM_SUFFIXES
                for suffix in ISTREAM_SUFFIXES {
                    port_args.push(PortArg::new(
                        format!("{child_port}{suffix}"),
                        Expr::ident(format!("{}{suffix}", arg.arg)),
                    ));
                }
            }
            ArgCategory::Ostream => {
                // Output stream: connect with OSTREAM_SUFFIXES
                for suffix in OSTREAM_SUFFIXES {
                    port_args.push(PortArg::new(
                        format!("{child_port}{suffix}"),
                        Expr::ident(format!("{}{suffix}", arg.arg)),
                    ));
                }
            }
            ArgCategory::Mmap | ArgCategory::AsyncMmap => {
                // Connect to per-instance pipeline offset
                let offset_sig = format!("{instance_name}__{child_port}_offset");
                port_args.push(PortArg::new(
                    format!("{child_port}_offset"),
                    Expr::ident(offset_sig),
                ));
                // Bind M-AXI channel ports:
                // If crossbar exists (slave index present), bind to downstream wires
                // Otherwise bind directly to upstream parent m_axi signals
                let m_axi_wire_prefix = if let Some(slave_idx) = mmap_slave_indices.get(&arg.arg) {
                    format!("m_axi_{}_{slave_idx}", arg.arg)
                } else {
                    format!("m_axi_{}", arg.arg)
                };
                for suffix in tapa_protocol::M_AXI_SUFFIXES_COMPACT {
                    port_args.push(PortArg::new(
                        format!("m_axi_{child_port}{suffix}"),
                        Expr::ident(format!("{m_axi_wire_prefix}{suffix}")),
                    ));
                }
            }
            ArgCategory::Istreams
            | ArgCategory::Ostreams
            | ArgCategory::Immap
            | ArgCategory::Ommap => {
                // Other categories: direct connection
                port_args.push(PortArg::new(
                    child_port.as_str(),
                    Expr::ident(&arg.arg),
                ));
            }
        }
    }

    ModuleInstance::new(child_task_name, instance_name).with_ports(port_args)
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
        assert!(text.contains("global_start_q"), "should use start input, got:\n{text}");
        // DONE->IDLE gated by global_done_q (not unconditional)
        assert!(text.contains("global_done_q"), "DONE->IDLE should be gated by done_release, got:\n{text}");
    }

    #[test]
    fn autorun_start_uses_ap_start() {
        let sig = InstanceSignals::new("auto_inst", true);
        let block = generate_autorun_start(&sig);
        let text = block.to_string();
        assert!(text.contains("auto_inst__ap_start <= ap_start"), "got:\n{text}");
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

    #[test]
    fn build_child_instance_has_handshake_and_args() {
        use std::collections::BTreeMap;
        let sig = InstanceSignals::new("worker_0", false);
        let mut args = BTreeMap::new();
        args.insert(
            "data_in".to_owned(),
            ArgDesign {
                arg: "fifo_0".to_owned(),
                cat: ArgCategory::Istream,
                extra: BTreeMap::new(),
            },
        );
        args.insert(
            "size".to_owned(),
            ArgDesign {
                arg: "n".to_owned(),
                cat: ArgCategory::Scalar,
                extra: BTreeMap::new(),
            },
        );
        let inst = build_child_instance("worker", "worker_0", &sig, &args, &BTreeMap::new());
        let text = inst.to_string();
        // Should have module name and instance name
        assert!(text.contains("worker worker_0"), "got:\n{text}");
        // Should have handshake ports
        assert!(text.contains(".ap_start(worker_0__ap_start)"), "got:\n{text}");
        assert!(text.contains(".ap_done(worker_0__ap_done)"), "got:\n{text}");
        // Should have scalar arg connected to per-instance pipeline signal
        assert!(text.contains(".size(worker_0__size)"), "got:\n{text}");
        // Should have istream suffixes
        assert!(text.contains("data_in_dout"), "got:\n{text}");
    }
}
