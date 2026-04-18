//! `tapa version` — prints the contents of the `VERSION` file with no
//! trailing newline. Mirrors `tapa/steps/version.py`.

use std::io::Write;

use clap::Parser;

use crate::context::CliContext;
use crate::error::Result;

#[derive(Debug, Parser)]
#[command(name = "version", about = "Print TAPA version to standard output.")]
pub struct Args {}

/// Compile-time version baked from the workspace `VERSION` file (read by
/// `build.rs`-equivalent: `include_str!`). The file is at the repo root.
const VERSION: &str = include_str!("../../../../VERSION");

pub fn version_string() -> &'static str {
    VERSION.trim()
}

pub fn run(_args: &Args, _ctx: &mut CliContext) -> Result<()> {
    let mut stdout = std::io::stdout().lock();
    stdout.write_all(version_string().as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_non_empty_and_trimmed() {
        let v = version_string();
        assert!(!v.is_empty());
        assert!(!v.starts_with(char::is_whitespace));
        assert!(!v.ends_with(char::is_whitespace));
    }
}
