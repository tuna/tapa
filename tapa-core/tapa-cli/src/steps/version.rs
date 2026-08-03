//! `tapa version` — prints the contents of the `VERSION` file with no
//! trailing newline.

use std::io::Write;

use clap::Parser;

use crate::context::CliContext;
use crate::error::Result;

#[derive(Debug, Parser)]
#[command(name = "version", about = "Print TAPA version to standard output.")]
pub struct VersionArgs {}

/// `VERSION` file content baked at compile time with the trailing
/// newline stripped (clap's `version = ...` attribute needs a
/// `&'static str` at parse time).
pub const VERSION: &str = {
    const RAW: &str = include_str!("../../../../VERSION");
    match const_str::strip_suffix!(RAW, "\n") {
        Some(s) => match const_str::strip_suffix!(s, "\r") {
            Some(s2) => s2,
            None => s,
        },
        None => RAW,
    }
};

pub fn run(_args: &VersionArgs, _ctx: &CliContext) -> Result<()> {
    let mut stdout = std::io::stdout().lock();
    stdout.write_all(VERSION.as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_trimmed() {
        assert!(!VERSION.starts_with(char::is_whitespace));
        assert!(!VERSION.ends_with(char::is_whitespace));
    }

    #[test]
    fn version_uses_expected_format() {
        // The version is `major.minor.date[.patch]` — at least 3 segments.
        let segment_count = VERSION.split('.').count();
        assert!(segment_count >= 3, "version must have at least 3 segments");
    }
}
