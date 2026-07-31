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
use crate::solver::solve::{evaluate, LpSolution, LpStatus, SolveOpts, Solver, SolverError};

/// Tuned CBC options for deterministic floorplanning solves.
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
        opts.validate()?;
        let dir = tempfile::tempdir().map_err(|source| self.spawn_err(source))?;
        let lp_path = dir.path().join("model.lp");
        let sol_path = dir.path().join("model.sol");
        let lp_text =
            write_cplex_lp(model).map_err(|error| SolverError::InvalidModel(error.to_string()))?;
        std::fs::write(&lp_path, lp_text).map_err(|source| self.spawn_err(source))?;

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

        let sol_text = std::fs::read_to_string(&sol_path).map_err(|source| {
            SolverError::BadOutput(format!(
                "CBC did not produce a readable solution file: {source}"
            ))
        })?;
        validate_solution(parse_sol(&sol_text)?, model)
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
            cmd.arg("-sec").arg(limit.as_secs_f64().to_string());
        }
        if let Some(gap) = opts.mip_gap {
            cmd.arg("-ratioGap").arg(gap.to_string());
        }
        if let Some(gap) = opts.mip_gap_abs {
            cmd.arg("-allowableGap").arg(gap.to_string());
        }
        cmd.args(TUNED_OPTIONS);
        // PuLP requests `all`: unlike CBC's sparse `normal` format, this emits
        // every column (including zero-valued binaries), allowing completeness
        // checks to distinguish an omitted value from zero.
        cmd.arg("-printingOptions").arg("all");
        cmd.arg("-solve").arg("-solution").arg(sol_path);
        cmd
    }
}

/// Validate CBC's incumbent and normalize its objective to the model value.
///
/// CBC releases disagree on whether the affine constant in an LP objective is
/// included in the solution-file header.  Validate the reported value first,
/// then try the convention that omits the constant.  Both paths use the same
/// strict incumbent validation, so malformed output still fails closed.
fn validate_solution(mut solution: LpSolution, model: &LpModel) -> Result<LpSolution, SolverError> {
    let Err(reported_error) = solution.validate_for(model) else {
        return canonicalize_objective(solution, model);
    };

    if solution.is_found() {
        let reported = solution.objective;
        // CBC releases disagree on whether the affine constant is part of the
        // reported objective; try the constant-omitted convention.
        solution.objective = reported + model.objective.constant;
        if solution.validate_for(model).is_ok() {
            return canonicalize_objective(solution, model);
        }
        solution.objective = reported;
    }

    Err(reported_error)
}

fn canonicalize_objective(
    mut solution: LpSolution,
    model: &LpModel,
) -> Result<LpSolution, SolverError> {
    if solution.is_found() {
        solution.objective = evaluate(&model.objective, &solution.values)?;
    }
    Ok(solution)
}

/// Parse a CBC `.sol` file: a status/objective header followed by row-activity
/// and model-variable `<index> <name> <value> <dual>` lines.
fn parse_sol(text: &str) -> Result<LpSolution, SolverError> {
    let mut lines = text.lines();
    let header = lines
        .next()
        .ok_or_else(|| SolverError::BadOutput("empty solution file".to_string()))?;

    if header.trim().is_empty() {
        return Err(SolverError::BadOutput(
            "empty solution-file header".to_string(),
        ));
    }

    let lower = header.to_ascii_lowercase();
    let status = if lower.starts_with("optimal") {
        LpStatus::Optimal
    } else if lower.starts_with("infeasible") || lower.starts_with("integer infeasible") {
        LpStatus::Infeasible
    } else if lower.starts_with("unbounded") {
        LpStatus::Unbounded
    } else if lower.starts_with("stopped") {
        if lower.contains("no integer solution") || !lower.contains("objective value") {
            LpStatus::NotSolved
        } else {
            LpStatus::Feasible
        }
    } else {
        return Err(SolverError::BadOutput(format!(
            "unrecognized CBC status header `{header}`",
        )));
    };

    let objective = if matches!(status, LpStatus::Optimal | LpStatus::Feasible) {
        parse_objective(header).ok_or_else(|| {
            SolverError::BadOutput(format!(
                "CBC reported an incumbent without an objective: `{header}`",
            ))
        })?
    } else {
        parse_objective(header).unwrap_or(0.0)
    };
    if !objective.is_finite() {
        return Err(SolverError::BadOutput(
            "CBC reported a non-finite objective".to_string(),
        ));
    }

    let mut values = HashMap::new();
    for (line_index, line) in lines.enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let (var, value) = parse_value_line(line).map_err(|message| {
            SolverError::BadOutput(format!(
                "invalid CBC value on line {}: {message}",
                line_index + 2,
            ))
        })?;
        let Some(var) = var else {
            continue;
        };
        if values.insert(var, value).is_some() {
            return Err(SolverError::BadOutput(format!(
                "CBC listed variable x{} more than once",
                var.0,
            )));
        }
    }

    Ok(LpSolution {
        status,
        objective,
        values,
    })
}

fn parse_value_line(line: &str) -> Result<(Option<LpVar>, f64), String> {
    let mut tokens = line.split_whitespace();
    let first = tokens.next().ok_or_else(|| "empty line".to_string())?;
    let index = if first == "**" {
        tokens
            .next()
            .ok_or_else(|| "missing column index".to_string())?
    } else {
        first
    };
    index
        .parse::<u32>()
        .map_err(|_| format!("invalid column index `{index}`"))?;

    let name = tokens
        .next()
        .ok_or_else(|| "missing variable name".to_string())?;
    let value = tokens
        .next()
        .ok_or_else(|| "missing variable value".to_string())?;
    // `printingOptions all` also emits row activities. Only xN column names
    // belong to the variable assignment; all rows are validated independently
    // against the parsed column values below.
    let var = name
        .strip_prefix('x')
        .and_then(|suffix| suffix.parse::<u32>().ok())
        .map(LpVar);
    let value = value
        .parse::<f64>()
        .map_err(|_| format!("invalid value `{value}` for `{name}`"))?;
    if !value.is_finite() {
        return Err(format!("non-finite value `{value}` for `{name}`"));
    }
    Ok((var, value))
}

/// Pull the objective off a header like `Optimal - objective value 12.0`.
fn parse_objective(header: &str) -> Option<f64> {
    let lower = header.to_ascii_lowercase();
    let offset = lower.find("objective value")? + "objective value".len();
    header
        .get(offset..)?
        .split_whitespace()
        .next()?
        .parse::<f64>()
        .ok()
}

/// Fail a solver test when the CBC binary is not installed.
///
/// CBC is a hard build requirement for tapa-floorplan's test suite; these
/// tests never silently skip.
#[cfg(test)]
pub(crate) fn missing_cbc() -> ! {
    panic!(
        "`cbc` was not found on PATH: the CBC solver is required to test \
         tapa-floorplan (Debian/Ubuntu: `sudo apt install coinor-cbc`)"
    )
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
    fn parse_sol_distinguishes_verified_incumbent_status() {
        let incumbent = parse_sol(
            "Stopped on time - objective value 1.0\n\
                   0 x0 1 0\n",
        )
        .expect("parse incumbent");
        assert_eq!(incumbent.status, LpStatus::Feasible);
        assert!(incumbent.is_found());

        let relaxation = parse_sol(
            "Stopped on time (no integer solution - continuous used) - objective value 0.5\n\
                   0 x0 0.5 0\n",
        )
        .expect("parse relaxation status");
        assert_eq!(relaxation.status, LpStatus::NotSolved);
        assert!(!relaxation.is_found());
    }

    #[test]
    fn parse_sol_rejects_malformed_or_ambiguous_output() {
        parse_sol("surprisingly good - objective value 1\n").expect_err("unknown status");
        parse_sol("Optimal - objective value nope\n").expect_err("invalid objective");
        parse_sol("Optimal - objective value 1\n0 x0 nope 0\n").expect_err("invalid value");
        parse_sol("Optimal - objective value 1\n0 x0 1 0\n1 x0 1 0\n")
            .expect_err("duplicate variable");
    }

    #[test]
    fn command_emits_independent_absolute_and_relative_gaps() {
        let opts = SolveOpts {
            mip_gap: Some(0.02),
            mip_gap_abs: Some(0.001),
            ..SolveOpts::default()
        };
        let command = CbcSolver::with_program("cbc").command(
            &PathBuf::from("model.lp"),
            &PathBuf::from("model.sol"),
            &opts,
        );
        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(args.windows(2).any(|pair| pair == ["-ratioGap", "0.02"]));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["-allowableGap", "0.001"]));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["-printingOptions", "all"]));
    }

    #[test]
    fn invalid_options_are_rejected_before_solver_launch() {
        let model = LpModel::new(Sense::Minimize);
        let opts = SolveOpts {
            time_limit: Some(std::time::Duration::ZERO),
            ..SolveOpts::default()
        };
        let error = CbcSolver::with_program("this-program-must-not-run")
            .solve(&model, &opts)
            .expect_err("zero time limit");
        assert!(matches!(error, SolverError::InvalidOptions(_)));
    }

    #[test]
    fn all_column_output_ignores_rows_but_requires_every_variable() {
        let mut model = LpModel::new(Sense::Minimize);
        let selected = model.add_binary("selected");
        let zero = model.add_binary("zero");
        model.set_objective(LinExpr::sum([(1.0, selected)]));
        model.add_constraint(
            "selected",
            LinExpr::sum([(1.0, selected)]),
            Comparison::Eq,
            1.0,
        );

        let complete = parse_sol(
            "Optimal - objective value 1\n\
             0 selected 1 0\n\
             0 x0 1 1\n\
             1 x1 0 0\n",
        )
        .expect("all-column output");
        complete
            .validate_for(&model)
            .expect("valid complete result");
        assert!(approx(complete.value(zero), 0.0));

        let incomplete = parse_sol(
            "Optimal - objective value 1\n\
             0 selected 1 0\n\
             0 x0 1 1\n",
        )
        .expect("syntactically valid but incomplete output");
        assert!(matches!(
            incomplete.validate_for(&model),
            Err(SolverError::InvalidSolution(_))
        ));
    }

    #[test]
    fn objective_header_accepts_affine_constant_conventions() {
        let mut model = LpModel::new(Sense::Minimize);
        let x = model.add_binary("x");
        model.set_objective(LinExpr::sum([(2.0, x)]).plus_constant(1.0));

        for reported in [2.0, 3.0] {
            let solution = parse_sol(&format!("Optimal - objective value {reported}\n0 x0 1 0\n"))
                .expect("parse");
            let solution = validate_solution(solution, &model).expect("valid objective convention");
            assert!(approx(solution.objective, 3.0));
        }
    }

    #[test]
    fn objective_header_rejects_an_unrelated_value() {
        let mut model = LpModel::new(Sense::Minimize);
        let x = model.add_binary("x");
        model.set_objective(LinExpr::sum([(2.0, x)]).plus_constant(1.0));
        let solution = parse_sol("Optimal - objective value 4\n0 x0 1 0\n").expect("parse");

        assert!(matches!(
            validate_solution(solution, &model),
            Err(SolverError::InvalidSolution(_))
        ));
    }

    #[test]
    fn objective_is_canonicalized_when_constant_is_within_tolerance() {
        let mut model = LpModel::new(Sense::Minimize);
        let x = model.add_binary("x");
        model.set_objective(LinExpr::sum([(2_000_000.0, x)]).plus_constant(1.0));
        let solution = parse_sol("Optimal - objective value 2000000\n0 x0 1 0\n").expect("parse");

        let solution = validate_solution(solution, &model).expect("valid incumbent");
        assert!(approx(solution.objective, 2_000_001.0));
    }

    #[test]
    fn negated_objective_headers_are_rejected() {
        // A maximize model whose optimum 12 is reported as -12 mismatches the
        // incumbent and must be rejected like any unrelated objective value.
        let mut model = LpModel::new(Sense::Maximize);
        let x = model.add_integer("x0", 0.0, 10.0);
        model.set_objective(LinExpr::sum([(3.0, x)]));
        model.add_constraint("cap", LinExpr::sum([(1.0, x)]), Comparison::Le, 4.0);
        let solution =
            parse_sol("Optimal - objective value -12\n      0 x0                     4   0\n")
                .expect("parse");
        assert!(matches!(
            validate_solution(solution, &model),
            Err(SolverError::InvalidSolution(_))
        ));

        // A negated minimize report is likewise rejected: a flipped sign must
        // never let a buggy solver mask a mismatched objective.
        let mut model = LpModel::new(Sense::Minimize);
        let x = model.add_binary("x");
        model.set_objective(LinExpr::sum([(2.0, x)]));
        model.add_constraint("pick", LinExpr::sum([(1.0, x)]), Comparison::Eq, 1.0);
        let solution = parse_sol("Optimal - objective value -2\n      0 x0 1 0\n").expect("parse");
        assert!(matches!(
            validate_solution(solution, &model),
            Err(SolverError::InvalidSolution(_))
        ));
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
            Err(SolverError::Spawn { .. }) => missing_cbc(),
            Err(other) => panic!("cbc failed unexpectedly: {other}"),
        }
    }

    #[test]
    fn cbc_returns_the_full_affine_objective() {
        let mut model = LpModel::new(Sense::Minimize);
        let x = model.add_binary("x");
        model.set_objective(LinExpr::sum([(2.0, x)]).plus_constant(1.0));
        model.add_constraint("pick", LinExpr::sum([(1.0, x)]), Comparison::Eq, 1.0);

        match CbcSolver::new().solve(&model, &SolveOpts::default()) {
            Ok(solution) => {
                assert_eq!(solution.status, LpStatus::Optimal);
                assert!(approx(solution.objective, 3.0));
            }
            Err(SolverError::Spawn { .. }) => missing_cbc(),
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
            Err(SolverError::Spawn { .. }) => missing_cbc(),
            Err(other) => panic!("cbc failed unexpectedly: {other}"),
        }
    }
}
