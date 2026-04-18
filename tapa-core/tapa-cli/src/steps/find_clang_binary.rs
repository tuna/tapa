//! Hidden `find-clang-binary` subcommand.
//!
//! Mirrors `tapa/__main__.py::_find_clang_binary_cmd`: resolves a
//! clang-family helper and prints its absolute path with no trailing
//! newline.

use std::io::Write;

use clap::Parser;

use crate::context::CliContext;
use crate::error::Result;
use crate::tapacc::discover::find_clang_binary;

#[derive(Debug, Parser)]
#[command(
    name = "find-clang-binary",
    hide = true,
    about = "Resolve a clang-family helper and print its absolute path."
)]
pub struct Args {
    /// `POTENTIAL_PATHS` key (e.g. `tapacc-binary`).
    pub name: String,
}

pub fn run(args: &Args, _ctx: &mut CliContext) -> Result<()> {
    let resolved = find_clang_binary(&args.name)?;
    let mut stdout = std::io::stdout().lock();
    write!(stdout, "{}", resolved.display())?;
    Ok(())
}
