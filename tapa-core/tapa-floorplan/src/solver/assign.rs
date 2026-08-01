//! One-of-k binary assignment rows, shared by the placement and routing
//! formulations.
//!
//! Both formulations give each chooser (a vertex's candidate regions, a
//! net's candidate paths) one binary per candidate and force exactly one
//! selection. [`add_one_of_k_row`] builds that shape in one place; the
//! caller keeps its own variable labels and row names, so the constructed
//! model stays byte-identical to what each formulation built inline.

use crate::solver::{Comparison, LinExpr, LpModel, LpVar};

/// Allocate `count` binary choice variables and add the one-of-k row
/// `Σ vars = 1` named `row_name`; return the sparse variable row.
///
/// `label(index)` names each variable. Variables are created first, then
/// the row, matching the construction order the formulations used inline.
pub fn add_one_of_k_row(
    lp: &mut LpModel,
    row_name: &str,
    count: usize,
    label: impl Fn(usize) -> String,
) -> Vec<LpVar> {
    let vars: Vec<LpVar> = (0..count)
        .map(|index| lp.add_binary(label(index)))
        .collect();
    lp.add_constraint(
        row_name,
        LinExpr::sum(vars.iter().map(|&var| (1.0, var))),
        Comparison::Eq,
        1.0,
    );
    vars
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::solver::{Sense, VarKind};

    #[test]
    fn one_of_k_row_shape() {
        let mut model = LpModel::new(Sense::Minimize);
        let vars = add_one_of_k_row(&mut model, "chooser", 3, |index| format!("c_{index}"));
        assert_eq!(vars.len(), 3);
        assert_eq!(model.num_vars(), 3);
        assert_eq!(model.num_constraints(), 1);

        let text = crate::solver::write_cplex_lp(&model).expect("render");
        assert!(text.contains("chooser: 1 x0 + 1 x1 + 1 x2 = 1"), "{text}");
        for var in &model.vars {
            assert_eq!(var.kind, VarKind::Binary);
        }
    }
}
