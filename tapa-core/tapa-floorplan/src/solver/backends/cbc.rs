//! The CBC backend: solve an [`LpModel`] by spawning the external `cbc` MILP
//! solver, mirroring PuLP's `PULP_CBC_CMD`.
//!
//! `cbc` is a runtime dependency discovered on `PATH` (or via the `TAPA_CBC`
//! override), not vendored or linked. It exits `0` for both feasible and
//! infeasible models and writes the status as the first line of its solution
//! file, so the backend keys off that line rather than the exit code.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;

use crate::solver::lp_writer::write_cplex_lp;
use crate::solver::model::{LpModel, LpVar};
use crate::solver::solve::{LpSolution, LpStatus, SolveOpts, Solver, SolverError};

/// The tuned CBC options ported from RapidStream (`floorplan.py:199-205`).
const TUNED_OPTIONS: &[&str] = &[
    "-cuts",
    "ifmove",
    "-preprocess",
    "aggregate",
    "-heuristics",
    "off",
    "-strongBranching",
    "5",
    "-trustPseudoCosts",
    "6",
];

/// A [`Solver`] backed by the external `cbc` binary.
#[derive(Debug, Clone)]
pub struct CbcSolver {
    program: String,
}

impl CbcSolver {
    /// A solver invoking `cbc` from `PATH`, or the `TAPA_CBC` override.
    #[must_use]
    pub fn new() -> Self {
        let program = std::env::var("TAPA_CBC").unwrap_or_else(|_| "cbc".to_string());
        Self { program }
    }

    /// A solver invoking a specific `cbc` binary.
    #[must_use]
    pub fn with_program(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
        }
    }
}

impl Default for CbcSolver {
    fn default() -> Self {
        Self::new()
    }
}

impl Solver for CbcSolver {
    fn solve(&self, model: &LpModel, opts: &SolveOpts) -> Result<LpSolution, SolverError> {
        let dir = tempfile::tempdir().map_err(|source| self.spawn_err(source))?;
        let lp_path = dir.path().join("model.lp");
        let sol_path = dir.path().join("model.sol");
        std::fs::write(&lp_path, write_cplex_lp(model)).map_err(|source| self.spawn_err(source))?;

        let output = self
            .command(&lp_path, &sol_path, opts)
            .output()
            .map_err(|source| self.spawn_err(source))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            let detail = if stderr.trim().is_empty() {
                stdout
            } else {
                stderr
            };
            return Err(SolverError::ToolFailure {
                status: output.status.code().unwrap_or(-1),
                stderr: detail.into_owned(),
            });
        }

        let sol_text =
            std::fs::read_to_string(&sol_path).map_err(|source| self.spawn_err(source))?;
        parse_sol(&sol_text)
    }
}

impl CbcSolver {
    fn spawn_err(&self, source: std::io::Error) -> SolverError {
        SolverError::Spawn {
            program: self.program.clone(),
            source,
        }
    }

    /// Build the `cbc <lp> [opts] -solve -solution <sol>` invocation.
    fn command(&self, lp_path: &PathBuf, sol_path: &PathBuf, opts: &SolveOpts) -> Command {
        let mut cmd = Command::new(&self.program);
        cmd.arg(lp_path);
        if let Some(threads) = opts.threads {
            cmd.arg("-threads").arg(threads.to_string());
        }
        if let Some(limit) = opts.time_limit {
            cmd.arg("-sec").arg(limit.as_secs().to_string());
        }
        if let Some(gap) = opts.mip_gap {
            cmd.arg("-ratioGap").arg(gap.to_string());
        }
        cmd.args(TUNED_OPTIONS);
        cmd.arg("-solve").arg("-solution").arg(sol_path);
        cmd
    }
}

/// Parse a CBC `.sol` file: a status/objective header line followed by one
/// `<index> <name> <value> <dual>` line per nonzero variable.
fn parse_sol(text: &str) -> Result<LpSolution, SolverError> {
    let mut lines = text.lines();
    let header = lines
        .next()
        .ok_or_else(|| SolverError::BadOutput("empty solution file".to_string()))?;

    let lower = header.to_ascii_lowercase();
    let status = if lower.contains("infeasible") {
        LpStatus::Infeasible
    } else if lower.contains("unbounded") {
        LpStatus::Unbounded
    } else if lower.contains("optimal") || lower.contains("stopped") {
        // "Stopped on time" with an incumbent is a usable solution (PuLP
        // treats a time-limited feasible result as Optimal).
        LpStatus::Optimal
    } else {
        LpStatus::NotSolved
    };

    let objective = parse_objective(header);

    let mut values = HashMap::new();
    for line in lines {
        let mut tokens = line.split_whitespace();
        let _index = tokens.next();
        let (Some(name), Some(value)) = (tokens.next(), tokens.next()) else {
            continue;
        };
        let Some(var) = name.strip_prefix('x').and_then(|n| n.parse::<u32>().ok()) else {
            continue;
        };
        let Ok(value) = value.parse::<f64>() else {
            continue;
        };
        values.insert(LpVar(var), value);
    }

    Ok(LpSolution {
        status,
        objective,
        values,
    })
}

/// Pull the objective off a header like `Optimal - objective value 12.0`.
fn parse_objective(header: &str) -> f64 {
    header
        .split("objective value")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|token| token.parse::<f64>().ok())
        .unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::solver::model::{Comparison, LinExpr, LpModel, Sense};

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    #[test]
    fn parse_sol_reads_status_objective_and_values() {
        let text = "\
Optimal - objective value 12.00000000
      0 x0                     4                       3
      1 x1                     0                       2
";
        let sol = parse_sol(text).expect("parse");
        assert_eq!(sol.status, LpStatus::Optimal);
        assert!(approx(sol.objective, 12.0));
        assert!(approx(sol.value(LpVar(0)), 4.0));
        assert!(approx(sol.value(LpVar(1)), 0.0));
    }

    #[test]
    fn parse_sol_recognizes_infeasible() {
        let sol = parse_sol("Infeasible - objective value 2.0\n").expect("parse");
        assert_eq!(sol.status, LpStatus::Infeasible);
        assert!(!sol.is_found());
    }

    #[test]
    fn cbc_solves_a_known_milp() {
        // maximize 3 x0 + 2 x1  s.t.  x0 + x1 <= 4,  x0 + 3 x1 <= 6
        // optimum: x0 = 4, x1 = 0, objective 12.
        let mut model = LpModel::new(Sense::Maximize);
        let x0 = model.add_integer("x0", 0.0, 10.0);
        let x1 = model.add_integer("x1", 0.0, 10.0);
        model.set_objective(LinExpr::sum([(3.0, x0), (2.0, x1)]));
        model.add_constraint(
            "c0",
            LinExpr::sum([(1.0, x0), (1.0, x1)]),
            Comparison::Le,
            4.0,
        );
        model.add_constraint(
            "c1",
            LinExpr::sum([(1.0, x0), (3.0, x1)]),
            Comparison::Le,
            6.0,
        );

        let opts = SolveOpts {
            threads: Some(1),
            ..SolveOpts::default()
        };
        match CbcSolver::new().solve(&model, &opts) {
            Ok(sol) => {
                assert!(
                    sol.is_found(),
                    "a feasible MILP must solve; got {:?}",
                    sol.status
                );
                assert!(approx(sol.value(x0), 4.0), "x0 = {}", sol.value(x0));
                assert!(approx(sol.value(x1), 0.0), "x1 = {}", sol.value(x1));
                assert!(approx(sol.objective, 12.0), "objective = {}", sol.objective);
            }
            Err(SolverError::Spawn { .. }) => {
                eprintln!("skipping cbc_solves_a_known_milp: `cbc` not found on PATH");
            }
            Err(other) => panic!("cbc failed unexpectedly: {other}"),
        }
    }

    #[test]
    fn cbc_reports_infeasible_model() {
        // x binary, x >= 1 and x <= 0 — no solution.
        let mut model = LpModel::new(Sense::Minimize);
        let x = model.add_binary("x");
        model.add_constraint("lo", LinExpr::sum([(1.0, x)]), Comparison::Ge, 1.0);
        model.add_constraint("hi", LinExpr::sum([(1.0, x)]), Comparison::Le, 0.0);

        match CbcSolver::new().solve(&model, &SolveOpts::default()) {
            Ok(sol) => assert_eq!(sol.status, LpStatus::Infeasible, "must be infeasible"),
            Err(SolverError::Spawn { .. }) => {
                eprintln!("skipping cbc_reports_infeasible_model: `cbc` not found on PATH");
            }
            Err(other) => panic!("cbc failed unexpectedly: {other}"),
        }
    }
}
