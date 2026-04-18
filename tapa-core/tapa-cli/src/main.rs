//! `tapa` binary entry point. Parses globals + chained subcommands via
//! clap, applies remote-config bootstrap, and walks the chained-step
//! linked list.

use std::process::ExitCode;

use clap::Parser;

use tapa_cli::context::CliContext;
use tapa_cli::error::CliError;
use tapa_cli::globals::Cli;
use tapa_cli::logging;
use tapa_cli::remote::bootstrap_remote;

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
    let cli = Cli::parse();

    // Python click's group default: a bare `tapa` invocation with no
    // subcommand prints `--help` and exits non-zero (`no_args_is_help`).
    // Without this branch the Rust CLI silently exited 0, hiding
    // genuine "user forgot to type a subcommand" mistakes.
    if cli.step.is_none() {
        use clap::CommandFactory;
        let _ = Cli::command().print_help();
        eprintln!();
        return Err(CliError::InvalidArg(
            "no subcommand supplied — see `tapa --help`".to_string(),
        ));
    }

    logging::install(cli.globals.verbose, cli.globals.quiet);

    let mut ctx = CliContext::from_globals(&cli.globals);
    let work_dir = ctx.work_dir.clone();
    ctx.switch_work_dir(work_dir.clone())
        .map_err(|e| CliError::WorkDir(work_dir.clone(), e.to_string()))?;

    if let Some(temp_dir) = cli.globals.temp_dir.as_deref() {
        std::env::set_var("TMPDIR", temp_dir);
    }

    // Bootstrap remote config (~/.taparc + CLI overrides) before any
    // native step runs — mirrors `tapa/__main__.py::entry_point`. Sync
    // failures inside this call are non-fatal so local-only flows are
    // unaffected.
    ctx.remote_config = bootstrap_remote(&cli.globals)?;

    if let Some(step) = cli.step {
        step.execute(&mut ctx)?;
    }
    Ok(())
}
