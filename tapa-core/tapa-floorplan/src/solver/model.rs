//! A small mixed-integer linear-program model: variables, a linear objective,
//! and linear constraints. Portable across ILP tools — [`crate::solver`]
//! writes it to CPLEX-LP text and a backend solves it.

/// A handle to a variable in an [`LpModel`]; its `u32` is the variable's
/// index, which is also its LP name `x{index}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LpVar(pub u32);

/// The domain of a variable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VarKind {
    /// `{0, 1}`.
    Binary,
    /// Integer within its bounds.
    Integer,
    /// Real within its bounds.
    Continuous,
}

/// Whether the objective is minimized or maximized.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sense {
    Minimize,
    Maximize,
}

/// A constraint's comparison operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Comparison {
    /// `≤`
    Le,
    /// `=`
    Eq,
    /// `≥`
    Ge,
}

/// A linear expression: `Σ coef·var + constant`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LinExpr {
    /// The `(coefficient, variable)` terms, in insertion order.
    pub terms: Vec<(f64, LpVar)>,
    /// The constant offset.
    pub constant: f64,
}

impl LinExpr {
    /// An empty expression (value `0`).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Build an expression from `(coefficient, variable)` terms.
    #[must_use]
    pub fn sum(terms: impl IntoIterator<Item = (f64, LpVar)>) -> Self {
        Self {
            terms: terms.into_iter().collect(),
            constant: 0.0,
        }
    }

    /// Append one `coef·var` term.
    #[must_use]
    pub fn term(mut self, coef: f64, var: LpVar) -> Self {
        self.terms.push((coef, var));
        self
    }

    /// Set the constant offset.
    #[must_use]
    pub fn plus_constant(mut self, constant: f64) -> Self {
        self.constant = constant;
        self
    }
}

/// Internal variable record.
#[derive(Debug, Clone)]
pub(crate) struct VarDef {
    /// Optional human-readable label, emitted as an LP comment for debugging.
    pub label: String,
    pub kind: VarKind,
    pub lower: f64,
    pub upper: f64,
}

/// Internal constraint record.
#[derive(Debug, Clone)]
pub(crate) struct ConstraintDef {
    pub name: String,
    pub expr: LinExpr,
    pub op: Comparison,
    pub rhs: f64,
}

/// A mixed-integer linear program: variables, an objective, and constraints.
///
/// Variables are added with [`LpModel::add_binary`] / [`LpModel::add_integer`]
/// / [`LpModel::add_continuous`], which return an [`LpVar`] handle. The
/// objective is a [`LinExpr`]; constraints are `expr op rhs`.
#[derive(Debug, Clone)]
pub struct LpModel {
    pub(crate) sense: Sense,
    pub(crate) objective: LinExpr,
    pub(crate) vars: Vec<VarDef>,
    pub(crate) constraints: Vec<ConstraintDef>,
}

impl LpModel {
    /// A fresh model with the given optimization sense and an empty objective.
    #[must_use]
    pub fn new(sense: Sense) -> Self {
        Self {
            sense,
            objective: LinExpr::new(),
            vars: Vec::new(),
            constraints: Vec::new(),
        }
    }

    fn add_var(
        &mut self,
        label: impl Into<String>,
        kind: VarKind,
        lower: f64,
        upper: f64,
    ) -> LpVar {
        let index = u32::try_from(self.vars.len()).expect("variable count fits u32");
        self.vars.push(VarDef {
            label: label.into(),
            kind,
            lower,
            upper,
        });
        LpVar(index)
    }

    /// Add a `{0, 1}` variable.
    pub fn add_binary(&mut self, label: impl Into<String>) -> LpVar {
        self.add_var(label, VarKind::Binary, 0.0, 1.0)
    }

    /// Add an integer variable bounded by `[lower, upper]`.
    pub fn add_integer(&mut self, label: impl Into<String>, lower: f64, upper: f64) -> LpVar {
        self.add_var(label, VarKind::Integer, lower, upper)
    }

    /// Add a continuous variable bounded by `[lower, upper]` (use
    /// [`f64::INFINITY`] for an open upper bound).
    pub fn add_continuous(&mut self, label: impl Into<String>, lower: f64, upper: f64) -> LpVar {
        self.add_var(label, VarKind::Continuous, lower, upper)
    }

    /// Replace the objective expression.
    pub fn set_objective(&mut self, objective: LinExpr) {
        self.objective = objective;
    }

    /// Add a named constraint `expr op rhs`.
    pub fn add_constraint(
        &mut self,
        name: impl Into<String>,
        expr: LinExpr,
        op: Comparison,
        rhs: f64,
    ) {
        self.constraints.push(ConstraintDef {
            name: name.into(),
            expr,
            op,
            rhs,
        });
    }

    /// The number of variables.
    #[must_use]
    pub fn num_vars(&self) -> usize {
        self.vars.len()
    }

    /// The number of constraints.
    #[must_use]
    pub fn num_constraints(&self) -> usize {
        self.constraints.len()
    }
}
