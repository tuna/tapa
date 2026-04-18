//! Verilog text emission (Display implementations) for all AST types.

use std::fmt;

use crate::builder::{
    AlwaysBlock, CommentPragma, ContinuousAssign, Expr, ModuleInstance, ParamArg, PortArg,
    Sensitivity, Statement,
};
use crate::port::{Direction, Port, Width};
use crate::signal::{Signal, SignalKind};

// ── Helpers ──────────────────────────────────────────────────────────

fn write_indent(f: &mut fmt::Formatter<'_>, level: usize) -> fmt::Result {
    for _ in 0..level {
        f.write_str("  ")?;
    }
    Ok(())
}

fn write_statements(
    f: &mut fmt::Formatter<'_>,
    stmts: &[Statement],
    indent: usize,
) -> fmt::Result {
    for stmt in stmts {
        write_statement(f, stmt, indent)?;
    }
    Ok(())
}

fn write_statement(f: &mut fmt::Formatter<'_>, stmt: &Statement, indent: usize) -> fmt::Result {
    match stmt {
        Statement::NonblockingAssign { lhs, rhs } => {
            write_indent(f, indent)?;
            writeln!(f, "{lhs} <= {rhs};")
        }
        Statement::BlockingAssign { lhs, rhs } => {
            write_indent(f, indent)?;
            writeln!(f, "{lhs} = {rhs};")
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
        Statement::Block(stmts) => {
            write_indent(f, indent)?;
            writeln!(f, "begin")?;
            write_statements(f, stmts, indent + 1)?;
            write_indent(f, indent)?;
            writeln!(f, "end")
        }
    }
}

// ── Expression Display ───────────────────────────────────────────────

impl fmt::Display for Expr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ident(name) => f.write_str(name),
            Self::Lit(val) => f.write_str(val),
            Self::BinOp { lhs, op, rhs } => write!(f, "({lhs} {op} {rhs})"),
            Self::Ternary {
                cond,
                then_val,
                else_val,
            } => write!(f, "({cond} ? {then_val} : {else_val})"),
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
            Self::Replicate { count, expr } => write!(f, "{{{count}{{{expr}}}}}"),
        }
    }
}

// ── Width Display ────────────────────────────────────────────────────

impl fmt::Display for Width {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let msb: String = self.msb.iter().map(|t| t.repr.as_str()).collect::<Vec<_>>().join("");
        let lsb: String = self.lsb.iter().map(|t| t.repr.as_str()).collect::<Vec<_>>().join("");
        write!(f, "[{msb}:{lsb}]")
    }
}

// ── Direction Display ────────────────────────────────────────────────

impl fmt::Display for Direction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Input => f.write_str("input"),
            Self::Output => f.write_str("output"),
            Self::Inout => f.write_str("inout"),
        }
    }
}

// ── Port Display ─────────────────────────────────────────────────────

impl fmt::Display for Port {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} wire", self.direction)?;
        if let Some(w) = &self.width {
            write!(f, " {w}")?;
        }
        write!(f, " {}", self.name)
    }
}

// ── Signal Display ───────────────────────────────────────────────────

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
        write!(f, "{}", self.kind)?;
        if let Some(w) = &self.width {
            write!(f, " {w}")?;
        }
        write!(f, " {};", self.name)
    }
}

// ── PortArg / ParamArg Display ───────────────────────────────────────

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

// ── ModuleInstance Display ───────────────────────────────────────────

impl fmt::Display for ModuleInstance {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.module_name)?;

        if !self.params.is_empty() {
            writeln!(f, " #(")?;
            for (i, p) in self.params.iter().enumerate() {
                let comma = if i + 1 < self.params.len() { "," } else { "" };
                writeln!(f, "  {p}{comma}")?;
            }
            f.write_str(")")?;
        }

        writeln!(f, " {} (", self.instance_name)?;
        for (i, p) in self.ports.iter().enumerate() {
            let comma = if i + 1 < self.ports.len() { "," } else { "" };
            writeln!(f, "  {p}{comma}")?;
        }
        writeln!(f, ");")
    }
}

// ── ContinuousAssign Display ─────────────────────────────────────────

impl fmt::Display for ContinuousAssign {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "assign {} = {};", self.lhs, self.rhs)
    }
}

// ── AlwaysBlock Display ──────────────────────────────────────────────

impl fmt::Display for AlwaysBlock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.sensitivity {
            Sensitivity::Posedge(clk) => writeln!(f, "always @(posedge {clk}) begin")?,
            Sensitivity::Star => writeln!(f, "always @(*) begin")?,
        }
        write_statements(f, &self.body, 1)?;
        writeln!(f, "end")
    }
}

// ── CommentPragma Display ────────────────────────────────────────────

impl fmt::Display for CommentPragma {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "(* {} *)", self.text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::CaseItem;
    use crate::expression::{Token, TokenKind};

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
    fn expr_ternary() {
        let e = Expr::ternary(Expr::ident("sel"), Expr::ident("a"), Expr::ident("b"));
        assert_eq!(e.to_string(), "(sel ? a : b)");
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
            msb: vec![Token {
                kind: TokenKind::Literal,
                repr: "31".to_owned(),
            }],
            lsb: vec![Token {
                kind: TokenKind::Literal,
                repr: "0".to_owned(),
            }],
        };
        assert_eq!(w.to_string(), "[31:0]");
    }

    #[test]
    fn port_display() {
        let p = Port {
            name: "data_in".to_owned(),
            direction: Direction::Input,
            width: Some(Width {
                msb: vec![Token {
                    kind: TokenKind::Literal,
                    repr: "31".to_owned(),
                }],
                lsb: vec![Token {
                    kind: TokenKind::Literal,
                    repr: "0".to_owned(),
                }],
            }),
            pragma: None,
        };
        assert_eq!(p.to_string(), "input wire [31:0] data_in");
    }

    #[test]
    fn signal_display() {
        let s = Signal {
            name: "count".to_owned(),
            kind: SignalKind::Reg,
            width: None,
        };
        assert_eq!(s.to_string(), "reg count;");
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
        assert!(text.contains("always @(posedge ap_clk) begin"), "got: {text}");
        assert!(text.contains("q <= d;"), "got: {text}");
        assert!(text.contains("end"), "got: {text}");
    }

    #[test]
    fn always_combinational_with_if() {
        let block = AlwaysBlock::combinational(vec![Statement::If {
            cond: Expr::ident("sel"),
            then_body: vec![Statement::BlockingAssign {
                lhs: Expr::ident("out"),
                rhs: Expr::ident("a"),
            }],
            else_body: vec![Statement::BlockingAssign {
                lhs: Expr::ident("out"),
                rhs: Expr::ident("b"),
            }],
        }]);
        let text = block.to_string();
        assert!(text.contains("always @(*) begin"), "got: {text}");
        assert!(text.contains("if (sel) begin"), "got: {text}");
        assert!(text.contains("end else begin"), "got: {text}");
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

    #[test]
    fn comment_pragma() {
        let p = CommentPragma::new("clk port=ap_clk");
        assert_eq!(p.to_string(), "(* clk port=ap_clk *)");
    }

    // ── Validation tests (negative paths) ────────────────────────────

    #[test]
    fn try_new_rejects_empty_module_name() {
        ModuleInstance::try_new("", "inst").unwrap_err();
    }

    #[test]
    fn try_new_rejects_empty_instance_name() {
        ModuleInstance::try_new("mod", "").unwrap_err();
    }

    #[test]
    fn validate_rejects_zero_port_instance() {
        let inst = ModuleInstance::new("mod", "inst");
        inst.validate().unwrap_err();
    }

    #[test]
    fn validate_accepts_instance_with_ports() {
        let inst = ModuleInstance::new("mod", "inst")
            .with_ports(vec![PortArg::new("clk", Expr::ident("ap_clk"))]);
        inst.validate().unwrap();
    }
}
