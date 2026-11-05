//! Verilog text emission (Display implementations) for all AST types.

use std::fmt;

use crate::builder::{
    AlwaysBlock, ContinuousAssign, Expr, ModuleInstance, ParamArg, PortArg, Statement,
};
use crate::expression::expression_source;
use crate::port::{Direction, Port, Width};
use crate::signal::{Signal, SignalKind};

// ── Helpers ──────────────────────────────────────────────────────────

fn write_indent(f: &mut fmt::Formatter<'_>, level: usize) -> fmt::Result {
    for _ in 0..level {
        f.write_str("  ")?;
    }
    Ok(())
}

fn write_statements(f: &mut fmt::Formatter<'_>, stmts: &[Statement], indent: usize) -> fmt::Result {
    for stmt in stmts {
        write_statement(f, stmt, indent)?;
    }
    Ok(())
}

/// One argument per line, comma-separated (the last line carries no comma).
fn write_arg_lines(f: &mut fmt::Formatter<'_>, args: &[impl fmt::Display]) -> fmt::Result {
    for (i, arg) in args.iter().enumerate() {
        let comma = if i + 1 < args.len() { "," } else { "" };
        writeln!(f, "  {arg}{comma}")?;
    }
    Ok(())
}

fn write_statement(f: &mut fmt::Formatter<'_>, stmt: &Statement, indent: usize) -> fmt::Result {
    match stmt {
        Statement::NonblockingAssign { lhs, rhs } => {
            write_indent(f, indent)?;
            writeln!(f, "{lhs} <= {rhs};")
        }
        Statement::If {
            cond,
            then_body,
            else_body,
        } => {
            write_indent(f, indent)?;
            writeln!(f, "if ({cond}) begin")?;
            write_statements(f, then_body, indent + 1)?;
            if !else_body.is_empty() {
                write_indent(f, indent)?;
                writeln!(f, "end else begin")?;
                write_statements(f, else_body, indent + 1)?;
            }
            write_indent(f, indent)?;
            writeln!(f, "end")
        }
        Statement::Case {
            expr,
            items,
            default,
        } => {
            write_indent(f, indent)?;
            writeln!(f, "case ({expr})")?;
            for item in items {
                write_indent(f, indent + 1)?;
                writeln!(f, "{}: begin", item.value)?;
                write_statements(f, &item.body, indent + 2)?;
                write_indent(f, indent + 1)?;
                writeln!(f, "end")?;
            }
            if !default.is_empty() {
                write_indent(f, indent + 1)?;
                writeln!(f, "default: begin")?;
                write_statements(f, default, indent + 2)?;
                write_indent(f, indent + 1)?;
                writeln!(f, "end")?;
            }
            write_indent(f, indent)?;
            writeln!(f, "endcase")
        }
    }
}

impl fmt::Display for Expr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ident(name) => f.write_str(name),
            Self::Lit(val) => f.write_str(val),
            Self::BinOp { lhs, op, rhs } => write!(f, "({lhs} {op} {rhs})"),
            Self::Index { base, index } => write!(f, "{base}[{index}]"),
            Self::Range { base, msb, lsb } => write!(f, "{base}[{msb}:{lsb}]"),
            Self::Concat(exprs) => {
                f.write_str("{")?;
                for (i, e) in exprs.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{e}")?;
                }
                f.write_str("}")
            }
            Self::Not(inner) => write!(f, "!{inner}"),
        }
    }
}

impl fmt::Display for Width {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let msb = expression_source(&self.msb);
        let lsb = expression_source(&self.lsb);
        write!(f, "[{msb}:{lsb}]")
    }
}

impl fmt::Display for Direction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Input => f.write_str("input"),
            Self::Output => f.write_str("output"),
            Self::Inout => f.write_str("inout"),
        }
    }
}

impl fmt::Display for Port {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} wire", self.direction)?;
        if let Some(w) = &self.width {
            write!(f, " {w}")?;
        }
        write!(f, " {}", self.name)
    }
}

impl fmt::Display for SignalKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Wire => f.write_str("wire"),
            Self::Reg => f.write_str("reg"),
        }
    }
}

impl fmt::Display for Signal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(attribute) = &self.attribute {
            write!(f, "(* {attribute} *) ")?;
        }
        write!(f, "{}", self.kind)?;
        if let Some(w) = &self.width {
            write!(f, " {w}")?;
        }
        write!(f, " {};", self.name)
    }
}

impl fmt::Display for PortArg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, ".{}({})", self.port_name, self.connection)
    }
}

impl fmt::Display for ParamArg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, ".{}({})", self.param_name, self.value)
    }
}

impl fmt::Display for ModuleInstance {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.module_name)?;

        if !self.params.is_empty() {
            writeln!(f, " #(")?;
            write_arg_lines(f, &self.params)?;
            f.write_str(")")?;
        }

        writeln!(f, " {} (", self.instance_name)?;
        write_arg_lines(f, &self.ports)?;
        writeln!(f, ");")
    }
}

impl fmt::Display for ContinuousAssign {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "assign {} = {};", self.lhs, self.rhs)
    }
}

impl fmt::Display for AlwaysBlock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "always @(posedge {}) begin", self.clk)?;
        write_statements(f, &self.body, 1)?;
        writeln!(f, "end")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::CaseItem;

    #[test]
    fn expr_ident() {
        assert_eq!(Expr::ident("foo").to_string(), "foo");
    }

    #[test]
    fn expr_int_const() {
        assert_eq!(Expr::int_const(32, 15).to_string(), "32'd15");
    }

    #[test]
    fn expr_binop() {
        let e = Expr::eq(Expr::ident("state"), Expr::int_const(2, 0));
        assert_eq!(e.to_string(), "(state == 2'd0)");
    }

    #[test]
    fn expr_concat() {
        let e = Expr::concat(vec![Expr::ident("a"), Expr::ident("b")]);
        assert_eq!(e.to_string(), "{a, b}");
    }

    #[test]
    fn expr_index() {
        let e = Expr::index(Expr::ident("data"), Expr::int(3));
        assert_eq!(e.to_string(), "data[3]");
    }

    #[test]
    fn expr_not() {
        assert_eq!(Expr::logical_not(Expr::ident("x")).to_string(), "!x");
    }

    #[test]
    fn width_display() {
        let w = Width {
            msb: vec!["31".to_owned()],
            lsb: vec!["0".to_owned()],
        };
        assert_eq!(w.to_string(), "[31:0]");
    }

    #[test]
    fn port_display() {
        let p = Port {
            name: "data_in".to_owned(),
            direction: Direction::Input,
            width: Some(Width {
                msb: vec!["31".to_owned()],
                lsb: vec!["0".to_owned()],
            }),
        };
        assert_eq!(p.to_string(), "input wire [31:0] data_in");
    }

    #[test]
    fn signal_display() {
        let s = Signal {
            name: "count".to_owned(),
            kind: SignalKind::Reg,
            width: None,
            attribute: None,
        };
        assert_eq!(s.to_string(), "reg count;");
    }

    #[test]
    fn signal_display_with_attribute() {
        let s = Signal {
            name: "ap_rst".to_owned(),
            kind: SignalKind::Wire,
            width: None,
            attribute: Some("max_fanout = 256".to_owned()),
        };
        assert_eq!(s.to_string(), "(* max_fanout = 256 *) wire ap_rst;");
    }

    #[test]
    fn module_instance_simple() {
        let inst = ModuleInstance::new("fifo", "fifo_0").with_ports(vec![
            PortArg::new("clk", Expr::ident("ap_clk")),
            PortArg::new("reset", Expr::ident("ap_rst_n")),
        ]);
        let text = inst.to_string();
        assert!(text.contains("fifo fifo_0 ("), "got: {text}");
        assert!(text.contains(".clk(ap_clk)"), "got: {text}");
        assert!(text.contains(".reset(ap_rst_n)"), "got: {text}");
    }

    #[test]
    fn module_instance_with_params() {
        let inst = ModuleInstance::new("fifo", "fifo_0")
            .with_params(vec![ParamArg::new("WIDTH", Expr::int(32))])
            .with_ports(vec![PortArg::new("clk", Expr::ident("ap_clk"))]);
        let text = inst.to_string();
        assert!(text.contains("#("), "got: {text}");
        assert!(text.contains(".WIDTH(32)"), "got: {text}");
    }

    #[test]
    fn continuous_assign_display() {
        let a = ContinuousAssign::new(Expr::ident("out"), Expr::ident("in"));
        assert_eq!(a.to_string(), "assign out = in;");
    }

    #[test]
    fn always_posedge() {
        let block = AlwaysBlock::posedge(
            "ap_clk",
            vec![Statement::NonblockingAssign {
                lhs: Expr::ident("q"),
                rhs: Expr::ident("d"),
            }],
        );
        let text = block.to_string();
        assert!(
            text.contains("always @(posedge ap_clk) begin"),
            "got: {text}"
        );
        assert!(text.contains("q <= d;"), "got: {text}");
        assert!(text.contains("end"), "got: {text}");
    }

    #[test]
    fn case_statement() {
        let block = AlwaysBlock::posedge(
            "clk",
            vec![Statement::Case {
                expr: Expr::ident("state"),
                items: vec![
                    CaseItem::new(
                        Expr::int_const(2, 0),
                        vec![Statement::NonblockingAssign {
                            lhs: Expr::ident("state"),
                            rhs: Expr::int_const(2, 1),
                        }],
                    ),
                    CaseItem::new(
                        Expr::int_const(2, 1),
                        vec![Statement::NonblockingAssign {
                            lhs: Expr::ident("state"),
                            rhs: Expr::int_const(2, 2),
                        }],
                    ),
                ],
                default: vec![Statement::NonblockingAssign {
                    lhs: Expr::ident("state"),
                    rhs: Expr::int_const(2, 0),
                }],
            }],
        );
        let text = block.to_string();
        assert!(text.contains("case (state)"), "got: {text}");
        assert!(text.contains("2'd0: begin"), "got: {text}");
        assert!(text.contains("default: begin"), "got: {text}");
        assert!(text.contains("endcase"), "got: {text}");
    }
}
