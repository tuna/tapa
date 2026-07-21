//! Rendering an [`LpModel`] to CPLEX-LP text — the format CBC reads.
//!
//! Variables are emitted by index as `x{index}`, which is LP-safe and unique
//! by construction; each variable's human label rides along as a `\` comment.
//! Binary variables go in the `Binaries` section, integers in `General`;
//! only non-default bounds are written.

use std::fmt::Write as _;

use crate::solver::model::{Comparison, LinExpr, LpModel, Sense, VarKind};

/// Render `model` as CPLEX-LP text, terminated by a trailing newline.
#[must_use]
pub fn write_cplex_lp(model: &LpModel) -> String {
    let mut lines: Vec<String> = Vec::new();

    // Variable labels as comments, for debuggable LP files.
    for (index, var) in model.vars.iter().enumerate() {
        if !var.label.is_empty() {
            lines.push(format!("\\ x{index}: {}", var.label));
        }
    }

    lines.push(
        match model.sense {
            Sense::Minimize => "Minimize",
            Sense::Maximize => "Maximize",
        }
        .to_string(),
    );
    lines.push(format!(" obj: {}", write_objective(&model.objective)));

    lines.push("Subject To".to_string());
    for constraint in &model.constraints {
        // Move the expression's constant to the right-hand side.
        let rhs = constraint.rhs - constraint.expr.constant;
        // A constraint with no variable terms is constant-only. CBC aborts on
        // such a line, and in this planner they are always vacuously true
        // (e.g. a cut with no crossing edges: `0 <= capacity`), so drop them.
        if constraint.expr.terms.iter().all(|(coef, _)| is_zero(*coef)) {
            debug_assert!(
                empty_constraint_is_vacuous(constraint.op, rhs),
                "an unsatisfiable constant-only constraint would be silently dropped",
            );
            continue;
        }
        lines.push(format!(
            " {}: {} {} {}",
            sanitize_name(&constraint.name),
            write_terms(&constraint.expr.terms),
            op_str(constraint.op),
            format_num(rhs),
        ));
    }

    let bounds = bound_lines(model);
    if !bounds.is_empty() {
        lines.push("Bounds".to_string());
        lines.extend(bounds);
    }

    let binaries = section_vars(model, VarKind::Binary);
    if !binaries.is_empty() {
        lines.push("Binaries".to_string());
        lines.extend(binaries);
    }

    let generals = section_vars(model, VarKind::Integer);
    if !generals.is_empty() {
        lines.push("General".to_string());
        lines.extend(generals);
    }

    lines.push("End".to_string());
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

/// The objective's terms plus its constant offset (CBC honors the constant).
fn write_objective(objective: &LinExpr) -> String {
    let mut out = write_terms(&objective.terms);
    if objective.constant > 0.0 {
        write!(out, " + {}", format_num(objective.constant)).expect("String write is infallible");
    } else if objective.constant < 0.0 {
        write!(out, " - {}", format_num(-objective.constant)).expect("String write is infallible");
    }
    out
}

/// Render a linear combination `Σ coef·x{i}` with explicit `+`/`-` joining.
/// Zero-coefficient terms are dropped; an all-zero list renders as `0`.
fn write_terms(terms: &[(f64, crate::solver::model::LpVar)]) -> String {
    let mut out = String::new();
    let mut first = true;
    for (coef, var) in terms {
        if is_zero(*coef) {
            continue;
        }
        let negative = *coef < 0.0;
        let magnitude = if negative { -coef } else { *coef };
        if first {
            if negative {
                out.push_str("- ");
            }
            first = false;
        } else {
            out.push_str(if negative { " - " } else { " + " });
        }
        write!(out, "{} x{}", format_num(magnitude), var.0).expect("String write is infallible");
    }
    if out.is_empty() {
        "0".to_string()
    } else {
        out
    }
}

/// An exactly-zero coefficient (terms are built from exact integer values).
#[allow(
    clippy::float_cmp,
    reason = "coefficients are exact; zero terms are dropped"
)]
fn is_zero(coef: f64) -> bool {
    coef == 0.0
}

/// Whether a constant-only constraint `0 op rhs` holds — used to assert that
/// dropping such a constraint is safe.
fn empty_constraint_is_vacuous(op: Comparison, rhs: f64) -> bool {
    match op {
        Comparison::Le => rhs >= 0.0,
        Comparison::Ge => rhs <= 0.0,
        Comparison::Eq => rhs.abs() < f64::EPSILON,
    }
}

/// The bounds lines for variables whose domain is not the LP default
/// `[0, +∞)`. Binary variables carry their bounds implicitly.
fn bound_lines(model: &LpModel) -> Vec<String> {
    let mut lines = Vec::new();
    for (index, var) in model.vars.iter().enumerate() {
        if var.kind == VarKind::Binary {
            continue;
        }
        let needs = var.upper.is_finite() || var.lower.abs() > 0.0;
        if !needs {
            continue;
        }
        if var.upper.is_finite() {
            lines.push(format!(
                " {} <= x{index} <= {}",
                format_num(var.lower),
                format_num(var.upper),
            ));
        } else {
            lines.push(format!(" x{index} >= {}", format_num(var.lower)));
        }
    }
    lines
}

/// The indented `x{index}` names of every variable of `kind`.
fn section_vars(model: &LpModel, kind: VarKind) -> Vec<String> {
    model
        .vars
        .iter()
        .enumerate()
        .filter(|(_, var)| var.kind == kind)
        .map(|(index, _)| format!(" x{index}"))
        .collect()
}

fn op_str(op: Comparison) -> &'static str {
    match op {
        Comparison::Le => "<=",
        Comparison::Eq => "=",
        Comparison::Ge => ">=",
    }
}

/// Format a number for the LP body: whole values print without a trailing
/// `.0` (f64 `Display` already does this), keeping the text terse.
fn format_num(value: f64) -> String {
    format!("{value}")
}

/// Make an identifier LP-safe: only `[A-Za-z0-9_]`, never leading with a digit.
fn sanitize_name(name: &str) -> String {
    let mut out: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if out.chars().next().is_none_or(|c| c.is_ascii_digit()) {
        out.insert(0, 'c');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::solver::model::{Comparison, LinExpr, LpModel, Sense};

    #[test]
    fn golden_lp_text() {
        let mut model = LpModel::new(Sense::Minimize);
        let pick = model.add_binary("pick");
        let count = model.add_integer("count", 0.0, 5.0);
        let flow = model.add_continuous("flow_amt", 0.0, f64::INFINITY);

        model.set_objective(
            LinExpr::sum([(2.0, pick), (-3.0, count), (1.0, flow)]).plus_constant(1.0),
        );
        model.add_constraint(
            "cap",
            LinExpr::sum([(1.0, pick), (2.0, count)]),
            Comparison::Le,
            5.0,
        );
        model.add_constraint(
            "flow",
            LinExpr::sum([(1.0, count), (-1.0, flow)]),
            Comparison::Eq,
            0.0,
        );
        // A constraint whose expression carries a constant, to pin RHS folding.
        model.add_constraint(
            "off",
            LinExpr::sum([(1.0, pick)]).plus_constant(1.0),
            Comparison::Ge,
            2.0,
        );

        let expected = "\
\\ x0: pick
\\ x1: count
\\ x2: flow_amt
Minimize
 obj: 2 x0 - 3 x1 + 1 x2 + 1
Subject To
 cap: 1 x0 + 2 x1 <= 5
 flow: 1 x1 - 1 x2 = 0
 off: 1 x0 >= 1
Bounds
 0 <= x1 <= 5
Binaries
 x0
General
 x1
End
";
        assert_eq!(write_cplex_lp(&model), expected);
    }

    #[test]
    fn constraint_names_are_sanitized() {
        let mut model = LpModel::new(Sense::Minimize);
        let v = model.add_binary("");
        model.add_constraint(
            "cut_x=3_capacity",
            LinExpr::sum([(1.0, v)]),
            Comparison::Le,
            1.0,
        );
        assert!(
            write_cplex_lp(&model).contains("cut_x_3_capacity:"),
            "the `=` must be sanitized to `_`",
        );
    }
}
