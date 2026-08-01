//! The sparse row-builder shared by the placement and routing
//! formulations.
//!
//! Both formulations build constraints and objectives as sparse
//! `(coefficient, variable)` term lists — skipping zero coefficients at
//! the source, keeping insertion order — and sum them into a [`LinExpr`]
//! exactly once. [`SparseRow`] gives that idiom one name and one
//! conversion point, so the collector idiom no longer repeats as raw
//! `Vec` plumbing.

use crate::solver::{LinExpr, LpVar};

/// A sparse linear form under construction: `(coefficient, variable)`
/// terms in insertion order, finished into a [`LinExpr`] in one place.
#[derive(Debug, Default, Clone)]
pub struct SparseRow {
    terms: Vec<(f64, LpVar)>,
}

impl SparseRow {
    /// An empty row.
    pub fn new() -> Self {
        Self::default()
    }

    /// Append one `coefficient·var` term.
    pub fn push(&mut self, coefficient: f64, var: LpVar) {
        self.terms.push((coefficient, var));
    }

    /// The collected terms as a linear expression (constant `0`).
    pub fn into_expr(self) -> LinExpr {
        LinExpr::sum(self.terms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::solver::{LpModel, Sense};

    #[test]
    fn insertion_order_is_preserved() {
        let mut model = LpModel::new(Sense::Minimize);
        let a = model.add_binary("a");
        let b = model.add_binary("b");
        let c = model.add_binary("c");

        let mut row = SparseRow::new();
        row.push(2.0, c);
        row.push(-1.0, a);
        row.push(3.0, b);
        let expr = row.into_expr();
        assert_eq!(expr.terms, vec![(2.0, c), (-1.0, a), (3.0, b)]);
        assert_eq!(
            expr.constant.to_bits(),
            0.0_f64.to_bits(),
            "a collected row carries no constant offset",
        );
    }

    #[test]
    fn a_cloned_row_extends_independently() {
        let mut model = LpModel::new(Sense::Minimize);
        let a = model.add_binary("a");
        let b = model.add_binary("b");
        let cap = model.add_continuous("cap", 0.0, f64::INFINITY);

        let mut crossings = SparseRow::new();
        crossings.push(4.0, a);
        crossings.push(4.0, b);
        let hard_row = crossings.clone();
        let mut normalization = crossings;
        normalization.push(-8.0, cap);

        assert_eq!(hard_row.into_expr().terms, vec![(4.0, a), (4.0, b)]);
        assert_eq!(
            normalization.into_expr().terms,
            vec![(4.0, a), (4.0, b), (-8.0, cap)],
        );
    }
}
