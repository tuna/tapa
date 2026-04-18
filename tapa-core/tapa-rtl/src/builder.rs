//! AST node types for Verilog code generation.
//!
//! These types support programmatic construction of Verilog fragments.
//! They are used by tapa-codegen to build new modules (FSM, crossbar)
//! and to generate code snippets appended to existing HLS modules.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::error::BuilderError;

// ── Expressions ──────────────────────────────────────────────────────

/// A typed expression for code generation (richer than token-level).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Expr {
    /// Named identifier: `foo`, `ap_clk`.
    Ident(String),
    /// Integer literal: `0`, `32'd15`.
    Lit(String),
    /// Binary operation: `lhs op rhs`.
    BinOp {
        lhs: Box<Self>,
        op: BinOperator,
        rhs: Box<Self>,
    },
    /// Ternary: `cond ? then_val : else_val`.
    Ternary {
        cond: Box<Self>,
        then_val: Box<Self>,
        else_val: Box<Self>,
    },
    /// Bit index: `base[index]`.
    Index {
        base: Box<Self>,
        index: Box<Self>,
    },
    /// Bit range: `base[msb:lsb]`.
    Range {
        base: Box<Self>,
        msb: Box<Self>,
        lsb: Box<Self>,
    },
    /// Concatenation: `{exprs[0], exprs[1], ...}`.
    Concat(Vec<Self>),
    /// Unary not: `!expr` or `~expr`.
    Not(Box<Self>),
    /// Replication: `{count{expr}}`.
    Replicate {
        count: Box<Self>,
        expr: Box<Self>,
    },
}

/// Binary operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BinOperator {
    Eq,
    Ne,
    Plus,
    Minus,
    And,
    Or,
    BitAnd,
    BitOr,
    Shl,
    Shr,
}

impl fmt::Display for BinOperator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Eq => "==",
            Self::Ne => "!=",
            Self::Plus => "+",
            Self::Minus => "-",
            Self::And => "&&",
            Self::Or => "||",
            Self::BitAnd => "&",
            Self::BitOr => "|",
            Self::Shl => "<<",
            Self::Shr => ">>",
        };
        f.write_str(s)
    }
}

// ── Expression constructors ──────────────────────────────────────────

impl Expr {
    pub fn ident(name: impl Into<String>) -> Self {
        Self::Ident(name.into())
    }

    pub fn lit(val: impl Into<String>) -> Self {
        Self::Lit(val.into())
    }

    pub fn int(val: u64) -> Self {
        Self::Lit(val.to_string())
    }

    pub fn int_const(width: u32, val: u64) -> Self {
        Self::Lit(format!("{width}'d{val}"))
    }

    pub fn hex_const(width: u32, val: u64) -> Self {
        Self::Lit(format!("{width}'h{val:x}"))
    }

    pub fn bin_op(lhs: Self, op: BinOperator, rhs: Self) -> Self {
        Self::BinOp {
            lhs: Box::new(lhs),
            op,
            rhs: Box::new(rhs),
        }
    }

    pub fn eq(lhs: Self, rhs: Self) -> Self {
        Self::bin_op(lhs, BinOperator::Eq, rhs)
    }

    pub fn ne(lhs: Self, rhs: Self) -> Self {
        Self::bin_op(lhs, BinOperator::Ne, rhs)
    }

    pub fn plus(lhs: Self, rhs: Self) -> Self {
        Self::bin_op(lhs, BinOperator::Plus, rhs)
    }

    pub fn minus(lhs: Self, rhs: Self) -> Self {
        Self::bin_op(lhs, BinOperator::Minus, rhs)
    }

    pub fn logical_and(lhs: Self, rhs: Self) -> Self {
        Self::bin_op(lhs, BinOperator::And, rhs)
    }

    pub fn logical_or(lhs: Self, rhs: Self) -> Self {
        Self::bin_op(lhs, BinOperator::Or, rhs)
    }

    pub fn bit_and(lhs: Self, rhs: Self) -> Self {
        Self::bin_op(lhs, BinOperator::BitAnd, rhs)
    }

    pub fn bit_or(lhs: Self, rhs: Self) -> Self {
        Self::bin_op(lhs, BinOperator::BitOr, rhs)
    }

    pub fn logical_not(inner: Self) -> Self {
        Self::Not(Box::new(inner))
    }

    pub fn ternary(cond: Self, then_val: Self, else_val: Self) -> Self {
        Self::Ternary {
            cond: Box::new(cond),
            then_val: Box::new(then_val),
            else_val: Box::new(else_val),
        }
    }

    pub fn index(base: Self, idx: Self) -> Self {
        Self::Index {
            base: Box::new(base),
            index: Box::new(idx),
        }
    }

    pub fn range(base: Self, msb: Self, lsb: Self) -> Self {
        Self::Range {
            base: Box::new(base),
            msb: Box::new(msb),
            lsb: Box::new(lsb),
        }
    }

    pub fn concat(exprs: Vec<Self>) -> Self {
        Self::Concat(exprs)
    }

    pub fn replicate(count: Self, expr: Self) -> Self {
        Self::Replicate {
            count: Box::new(count),
            expr: Box::new(expr),
        }
    }
}

// ── Port/Instance arguments ──────────────────────────────────────────

/// Named port connection: `.port_name(signal_expr)`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortArg {
    pub port_name: String,
    pub connection: Expr,
}

impl PortArg {
    pub fn new(port_name: impl Into<String>, connection: Expr) -> Self {
        Self {
            port_name: port_name.into(),
            connection,
        }
    }
}

/// Named parameter connection: `.param_name(value)`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParamArg {
    pub param_name: String,
    pub value: Expr,
}

impl ParamArg {
    pub fn new(param_name: impl Into<String>, value: Expr) -> Self {
        Self {
            param_name: param_name.into(),
            value,
        }
    }
}

// ── Module instantiation ─────────────────────────────────────────────

/// A module instantiation: `module_name #(...) instance_name (...);`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleInstance {
    pub module_name: String,
    pub instance_name: String,
    pub params: Vec<ParamArg>,
    pub ports: Vec<PortArg>,
}

impl ModuleInstance {
    pub fn new(
        module_name: impl Into<String>,
        instance_name: impl Into<String>,
    ) -> Self {
        Self {
            module_name: module_name.into(),
            instance_name: instance_name.into(),
            params: Vec::new(),
            ports: Vec::new(),
        }
    }

    /// Validated constructor that rejects empty names.
    pub fn try_new(
        module_name: impl Into<String>,
        instance_name: impl Into<String>,
    ) -> Result<Self, BuilderError> {
        let module_name = module_name.into();
        let instance_name = instance_name.into();
        if module_name.is_empty() || instance_name.is_empty() {
            return Err(BuilderError::EmptyName);
        }
        Ok(Self {
            module_name,
            instance_name,
            params: Vec::new(),
            ports: Vec::new(),
        })
    }

    #[must_use]
    pub fn with_params(mut self, params: Vec<ParamArg>) -> Self {
        self.params = params;
        self
    }

    #[must_use]
    pub fn with_ports(mut self, ports: Vec<PortArg>) -> Self {
        self.ports = ports;
        self
    }

    /// Validate that the instance has at least one port connection.
    pub fn validate(&self) -> Result<(), BuilderError> {
        if self.module_name.is_empty() || self.instance_name.is_empty() {
            return Err(BuilderError::EmptyName);
        }
        if self.ports.is_empty() {
            return Err(BuilderError::NoPortConnections);
        }
        Ok(())
    }
}

// ── Continuous assignment ────────────────────────────────────────────

/// `assign lhs = rhs;`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContinuousAssign {
    pub lhs: Expr,
    pub rhs: Expr,
}

impl ContinuousAssign {
    pub fn new(lhs: Expr, rhs: Expr) -> Self {
        Self { lhs, rhs }
    }
}

// ── Procedural blocks ────────────────────────────────────────────────

/// Sensitivity list for always blocks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Sensitivity {
    /// `always @(posedge clk)`
    Posedge(String),
    /// `always @(*)` — combinational.
    Star,
}

/// A statement inside a procedural block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Statement {
    /// `lhs <= rhs;`
    NonblockingAssign { lhs: Expr, rhs: Expr },
    /// `lhs = rhs;`
    BlockingAssign { lhs: Expr, rhs: Expr },
    /// `if (cond) begin ... end [else begin ... end]`
    If {
        cond: Expr,
        then_body: Vec<Self>,
        else_body: Vec<Self>,
    },
    /// `case (expr) ... endcase`
    Case {
        expr: Expr,
        items: Vec<CaseItem>,
        default: Vec<Self>,
    },
    /// `begin ... end` block (for grouping).
    Block(Vec<Self>),
}

/// A single case item: `value: begin ... end`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaseItem {
    pub value: Expr,
    pub body: Vec<Statement>,
}

impl CaseItem {
    pub fn new(value: Expr, body: Vec<Statement>) -> Self {
        Self { value, body }
    }
}

/// `always @(...) begin ... end`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlwaysBlock {
    pub sensitivity: Sensitivity,
    pub body: Vec<Statement>,
}

impl AlwaysBlock {
    pub fn new(sensitivity: Sensitivity, body: Vec<Statement>) -> Self {
        Self { sensitivity, body }
    }

    pub fn posedge(clk: impl Into<String>, body: Vec<Statement>) -> Self {
        Self::new(Sensitivity::Posedge(clk.into()), body)
    }

    pub fn combinational(body: Vec<Statement>) -> Self {
        Self::new(Sensitivity::Star, body)
    }
}

// ── Comment pragma ───────────────────────────────────────────────────

/// A comment line to insert: `(* text *)`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommentPragma {
    pub text: String,
}

impl CommentPragma {
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }
}
