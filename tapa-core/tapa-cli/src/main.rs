//! `tapa` binary entry point. Parses global flags, splits the trailing
//! argv into chained-subcommand chunks, and invokes each step in order.

use std::process::ExitCode;

use clap::{CommandFactory, FromArgMatches};

use tapa_cli::chain;
use tapa_cli::context::CliContext;
use tapa_cli::error::CliError;
use tapa_cli::globals::GlobalArgs;
use tapa_cli::logging;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("tapa: {err}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), CliError> {
    let argv: Vec<String> = std::env::args().collect();

    // Hand-roll global-flag extraction so the chained tail keeps its
    // per-subcommand `--flag` ownership intact. clap's
    // `try_get_matches_from` would otherwise greedy-consume any
    // long-form option that happens to be named identically.
    let (global_argv, rest) = split_globals(&argv);
    let mut cmd = GlobalArgs::command();
    cmd = cmd.no_binary_name(false);
    let matches = match cmd.try_get_matches_from(&global_argv) {
        Ok(m) => m,
        Err(e) => {
            // clap signals `--help` and `--version` via error variants;
            // both are intentional graceful exits, not errors.
            match e.kind() {
                clap::error::ErrorKind::DisplayHelp
                | clap::error::ErrorKind::DisplayVersion => {
                    let _ = e.print();
                    return Ok(());
                }
                _ => {
                    return Err(CliError::ClapParse {
                        step: "<global>".to_string(),
                        message: e.to_string(),
                    });
                }
            }
        }
    };
    let globals = GlobalArgs::from_arg_matches(&matches).map_err(|e| {
        CliError::ClapParse {
            step: "<global>".to_string(),
            message: e.to_string(),
        }
    })?;

    logging::install(globals.verbose, globals.quiet);

    let mut ctx = CliContext::from_globals(&globals);
    ctx.switch_work_dir(ctx.work_dir.clone())
        .map_err(|e| CliError::WorkDir(globals.work_dir.clone(), e.to_string()))?;

    let chunks = chain::split(&rest)?;
    if chunks.is_empty() {
        // No subcommand → behave like click's group with no command:
        // print help and exit 0.
        let _ = GlobalArgs::command().print_help();
        println!();
        return Ok(());
    }
    chain::dispatch(chunks, &mut ctx)
}

/// Partition `argv` into `(globals_with_argv0, trailing_subcommand_argv)`.
///
/// We walk left-to-right and stop at the first non-flag token that names
/// a known subcommand (delegating recognition to `tapa_cli::steps`).
fn split_globals(argv: &[String]) -> (Vec<String>, Vec<String>) {
    let mut idx = 1; // argv[0] is the binary path.
    while idx < argv.len() {
        let tok = &argv[idx];
        if tapa_cli::steps::is_known(tok) {
            break;
        }
        // Long/short flag with `=` value or value as next token: the
        // GlobalArgs parser handles both shapes, so we just keep
        // walking until we hit a subcommand name.
        idx += 1;
    }
    let globals: Vec<String> = argv[..idx].to_vec();
    let rest: Vec<String> = argv[idx..].to_vec();
    (globals, rest)
}
