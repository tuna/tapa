//! `tapa` binary entry point. Parses globals + chained subcommands via
//! clap, applies remote-config bootstrap, and walks the chained-step
//! linked list.

use std::process::ExitCode;

use clap::Parser;

use tapa_cli::chain::Step;
use tapa_cli::context::CliContext;
use tapa_cli::error::CliError;
use tapa_cli::globals::Cli;
use tapa_cli::logging;
use tapa_cli::remote_config::bootstrap_remote;
use tapa_cli::update;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("tapa: {err}");
            ExitCode::from(err.exit_code())
        }
    }
}

fn run() -> Result<(), CliError> {
    let cli = Cli::parse();

    // A bare `tapa` invocation with no subcommand prints `--help`
    // and exits non-zero.
    if cli.step.is_none() {
        use clap::CommandFactory;
        let _ = Cli::command().print_help();
        eprintln!();
        return Err(CliError::InvalidArg(
            "no subcommand supplied — see `tapa --help`".to_string(),
        ));
    }

    logging::install(cli.globals.verbose, cli.globals.quiet);

    // The update flow never touches vendor tools or compiler state:
    // skip remote bootstrap (which may trigger an SSH sync — the hidden
    // `update-check` worker runs detached and must not) and skip the
    // automatic release check (the worker would recurse; `update` would
    // warn about the very release it just installed).
    let is_update_flow = matches!(
        cli.step,
        Some(Step::Update { .. } | Step::UpdateCheck { .. })
    );

    let mut ctx = CliContext::from_globals(&cli.globals);
    // The update flow gets a cache-dir work dir: it has no use for
    // `./work.out`, and creating one in the user's cwd would be litter.
    let work_dir = if is_update_flow {
        update::update_flow_work_dir()
    } else {
        ctx.work_dir.clone()
    };
    ctx.switch_work_dir(work_dir.clone())
        .map_err(|e| CliError::WorkDir(work_dir.clone(), e.to_string()))?;

    if let Some(temp_dir) = cli.globals.temp_dir.as_deref() {
        std::env::set_var("TMPDIR", temp_dir);
    }

    // The update flow never touches vendor tools or compiler state:
    // skip remote bootstrap (which may trigger an SSH sync — the hidden
    // `update-check` worker runs detached and must not) and skip the
    // automatic release check (the worker would recurse; `update` would
    // warn about the very release it just installed).
    let is_update_flow = matches!(
        cli.step,
        Some(Step::Update { .. } | Step::UpdateCheck { .. })
    );

    // Bootstrap remote config (~/.taparc + CLI overrides) before any
    // compiler step runs. Sync failures inside this call are non-fatal so
    // local-only flows are unaffected.
    if !is_update_flow {
        ctx.remote_config = bootstrap_remote(&cli.globals)?;
    }

    if let Some(step) = cli.step {
        step.execute(&mut ctx)?;
    }

    // Cached, non-blocking release check: prints last, never fails.
    if !is_update_flow {
        update::finish();
    }
    Ok(())
}
