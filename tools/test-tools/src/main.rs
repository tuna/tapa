mod abgraph;
mod analyze;
mod common;
mod floorplan;
mod package_layout;
mod reports;
mod shared_mmap;
mod zip_diff;

use std::env;
use std::ffi::OsString;
use std::process;

use common::{arg_path, arg_str, Result};

fn main() {
    if let Err(error) = run(&env::args_os().skip(1).collect::<Vec<_>>()) {
        eprintln!("{error}");
        process::exit(1);
    }
}

fn run(args: &[OsString]) -> Result<()> {
    let Some(command) = args.first().and_then(|arg| arg.to_str()) else {
        return Err(usage());
    };
    match command {
        "analyze-smoke" => analyze::analyze_smoke(),
        "compare-abgraph" => {
            let name = arg_str(args, 1, "compare-abgraph <name>")?;
            abgraph::compare_abgraph(name)
        }
        "gen-floorplan" => {
            let index = arg_str(args, 1, "gen-floorplan <index> <app> <output>")?
                .parse::<u64>()
                .map_err(|error| format!("invalid floorplan index: {error}"))?;
            let app = arg_str(args, 2, "gen-floorplan <index> <app> <output>")?;
            let output = arg_path(args, 3, "gen-floorplan <index> <app> <output>")?;
            floorplan::gen_floorplan(index, app, &output)
        }
        "check-xo-reports" => {
            let path = arg_str(args, 1, "check-xo-reports <workspace-path>")?;
            reports::check_xo_reports(&common::workspace_path(path))
        }
        "zip-diff" => {
            let actual = arg_str(args, 1, "zip-diff <actual> <expected>")?;
            let expected = arg_str(args, 2, "zip-diff <actual> <expected>")?;
            zip_diff::zip_diff(
                &common::workspace_path(actual),
                &common::workspace_path(expected),
            )
        }
        "check-shared-mmap-pragmas" => {
            let xo = arg_str(args, 1, "check-shared-mmap-pragmas <xo>")?;
            shared_mmap::check_shared_mmap_pragmas(&common::workspace_path(xo))
        }
        "check-package-layout" => {
            let tar = arg_str(args, 1, "check-package-layout <tar>")?;
            package_layout::check_package_layout(&common::workspace_path(tar))
        }
        _ => Err(usage()),
    }
}

fn usage() -> String {
    "usage: tapa-test-tools <analyze-smoke|compare-abgraph|gen-floorplan|check-xo-reports|zip-diff|check-shared-mmap-pragmas|check-package-layout> ..."
        .to_string()
}
