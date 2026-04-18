//! Instance handshake signal generation.
//!
//! Ports `tapa/codegen/instance_signals.py`: generates state, start,
//! done, idle, ready signals for task instances.

use tapa_rtl::builder::{Expr, PortArg};
use tapa_rtl::mutation::{reg, simple_port, wide_reg, wire};
use tapa_rtl::port::Direction;
use tapa_rtl::signal::Signal;

/// Handshake signals for a single task instance.
pub struct InstanceSignals {
    instance_name: String,
    is_autorun: bool,
}

impl InstanceSignals {
    /// Create signals for an instance.
    ///
    /// `is_autorun`: true if the instance has a negative step (auto-start).
    ///
    /// # Panics
    ///
    /// Panics if `instance_name` is empty.
    pub fn new(instance_name: impl Into<String>, is_autorun: bool) -> Self {
        let instance_name = instance_name.into();
        assert!(!instance_name.is_empty(), "instance name must not be empty");
        Self {
            instance_name,
            is_autorun,
        }
    }

    /// Instance state register name: `{instance}__state`.
    pub fn state_name(&self) -> String {
        format!("{}__state", self.instance_name)
    }

    /// Instance start signal name: `{instance}__ap_start`.
    pub fn start_name(&self) -> String {
        format!("{}__ap_start", self.instance_name)
    }

    /// Instance done signal name: `{instance}__ap_done`.
    pub fn done_name(&self) -> String {
        format!("{}__ap_done", self.instance_name)
    }

    /// Instance `is_done` signal name: `{instance}__is_done`.
    pub fn is_done_name(&self) -> String {
        format!("{}__is_done", self.instance_name)
    }

    /// Instance idle signal name: `{instance}__ap_idle`.
    pub fn idle_name(&self) -> String {
        format!("{}__ap_idle", self.instance_name)
    }

    /// Instance ready signal name: `{instance}__ap_ready`.
    pub fn ready_name(&self) -> String {
        format!("{}__ap_ready", self.instance_name)
    }

    /// Expression for the state register.
    pub fn state_expr(&self) -> Expr {
        Expr::ident(self.state_name())
    }

    /// Expression for the start signal.
    pub fn start_expr(&self) -> Expr {
        Expr::ident(self.start_name())
    }

    /// Expression for the done signal.
    pub fn done_expr(&self) -> Expr {
        Expr::ident(self.done_name())
    }

    /// `set_state(new_state)` -> nonblocking assign: `{instance}__state <= new_state`.
    pub fn set_state(&self, new_state: Expr) -> tapa_rtl::builder::Statement {
        tapa_rtl::builder::Statement::NonblockingAssign {
            lhs: self.state_expr(),
            rhs: new_state,
        }
    }

    /// `is_state(target)` -> equality comparison: `{instance}__state == target`.
    pub fn is_state(&self, target: Expr) -> Expr {
        Expr::eq(self.state_expr(), target)
    }

    /// Generate all handshake signal declarations for this instance.
    pub fn all_signals(&self) -> Vec<Signal> {
        if self.is_autorun {
            vec![reg(self.start_name())]
        } else {
            vec![
                wide_reg(self.state_name(), "1", "0"),
                wire(self.start_name()),
                wire(self.done_name()),
                wire(self.is_done_name()),
                wire(self.idle_name()),
                wire(self.ready_name()),
            ]
        }
    }

    /// Generate public handshake port arguments for connecting to the child instance.
    pub fn instance_portargs(&self) -> Vec<PortArg> {
        let mut args = vec![PortArg::new("ap_start", self.start_expr())];
        if !self.is_autorun {
            args.extend([
                PortArg::new("ap_done", self.done_expr()),
                PortArg::new("ap_idle", Expr::ident(self.idle_name())),
                PortArg::new("ap_ready", Expr::ident(self.ready_name())),
            ]);
        }
        args
    }

    /// Generate public handshake ports for the FSM module interface.
    pub fn fsm_ports(&self) -> Vec<tapa_rtl::port::Port> {
        if self.is_autorun {
            vec![simple_port(self.start_name(), Direction::Output)]
        } else {
            vec![
                simple_port(self.start_name(), Direction::Output),
                simple_port(self.ready_name(), Direction::Input),
                simple_port(self.done_name(), Direction::Input),
                simple_port(self.idle_name(), Direction::Input),
            ]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_autorun_signals() {
        let sig = InstanceSignals::new("child_0", false);
        let signals = sig.all_signals();
        assert_eq!(signals.len(), 6, "should have state, start, done, is_done, idle, ready");
        assert_eq!(signals[0].name, "child_0__state");
        assert_eq!(signals[1].name, "child_0__ap_start");
    }

    #[test]
    fn autorun_signals() {
        let sig = InstanceSignals::new("auto_inst", true);
        let signals = sig.all_signals();
        assert_eq!(signals.len(), 1, "autorun should only have start reg");
    }

    #[test]
    fn set_state_produces_nonblocking_assign() {
        let sig = InstanceSignals::new("inst", false);
        let stmt = sig.set_state(Expr::int_const(2, 1));
        let text = format!("{}", tapa_rtl::builder::AlwaysBlock::posedge("clk", vec![stmt]));
        assert!(text.contains("inst__state <= 2'd1"), "got: {text}");
    }

    #[test]
    fn is_state_produces_equality() {
        let sig = InstanceSignals::new("inst", false);
        let expr = sig.is_state(Expr::int_const(2, 0));
        assert_eq!(expr.to_string(), "(inst__state == 2'd0)");
    }

    #[test]
    fn instance_portargs_non_autorun() {
        let sig = InstanceSignals::new("child", false);
        let args = sig.instance_portargs();
        assert_eq!(args.len(), 4, "should have start, done, idle, ready");
    }

    #[test]
    fn instance_portargs_autorun() {
        let sig = InstanceSignals::new("child", true);
        let args = sig.instance_portargs();
        assert_eq!(args.len(), 1, "autorun should only have start");
    }

    #[test]
    #[should_panic(expected = "instance name must not be empty")]
    fn empty_name_rejected() {
        let _sig = InstanceSignals::new("", false);
    }

    #[test]
    fn fsm_ports_non_autorun() {
        let sig = InstanceSignals::new("child", false);
        let ports = sig.fsm_ports();
        assert_eq!(ports.len(), 4, "should have start, ready, done, idle");
    }

    #[test]
    fn fsm_ports_autorun() {
        let sig = InstanceSignals::new("child", true);
        let ports = sig.fsm_ports();
        assert_eq!(ports.len(), 1, "autorun should only have start port");
    }
}
