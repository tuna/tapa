//! `tapa version` — prints the contents of the `VERSION` file with no
//! trailing newline. Mirrors `tapa/steps/version.py`.

use std::io::Write;

use clap::Parser;

use crate::context::CliContext;
use crate::error::Result;

#[derive(Debug, Parser)]
#[command(name = "version", about = "Print TAPA version to standard output.")]
pub struct VersionArgs {}

/// Raw `VERSION` file content baked at compile time. Includes any trailing
/// newline; use [`VERSION`] for the trimmed string.
const VERSION_RAW: &str = include_str!("../../../../VERSION");

/// Compile-time, ASCII-trimmed view of `VERSION_RAW`. Required because
/// clap's `version = ...` attribute needs a `&'static str` at parse time
/// and `str::trim_end` is not yet `const fn` on stable Rust.
pub const VERSION: &str = trim_ascii_end(VERSION_RAW);

const fn trim_ascii_end(input: &str) -> &str {
    let bytes = input.as_bytes();
    let mut end = bytes.len();
    while end > 0 {
        let b = bytes[end - 1];
        if b != b'\n' && b != b'\r' && b != b' ' && b != b'\t' {
            break;
        }
        end -= 1;
    }
    // SAFETY: the slice ends on a UTF-8 char boundary because we only
    // peel off ASCII whitespace bytes (each one byte wide in UTF-8).
    let trimmed = unsafe { std::slice::from_raw_parts(bytes.as_ptr(), end) };
    match std::str::from_utf8(trimmed) {
        Ok(s) => s,
        // SAFETY: input was already valid UTF-8; trimming ASCII bytes
        // cannot break that invariant.
        Err(_) => unsafe { std::hint::unreachable_unchecked() },
    }
}

pub fn version_string() -> &'static str {
    VERSION
}

pub fn run(_args: &VersionArgs, _ctx: &mut CliContext) -> Result<()> {
    let mut stdout = std::io::stdout().lock();
    stdout.write_all(VERSION.as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_non_empty_and_trimmed() {
        assert!(!VERSION.is_empty());
        assert!(!VERSION.starts_with(char::is_whitespace));
        assert!(!VERSION.ends_with(char::is_whitespace));
    }

    #[test]
    fn version_matches_python_format() {
        // Mirrors `0.1.YYYYMMDD` shape from `tapa/_version.py`.
        let segment_count = VERSION.split('.').count();
        assert!(segment_count >= 3, "version must have at least 3 segments");
    }
}
