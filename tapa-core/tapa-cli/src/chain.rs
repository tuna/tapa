//! Hand-rolled chained-subcommand dispatcher.
//!
//! click's chained-group model lets `tapa analyze … synth … pack …` run
//! the three steps in sequence with per-step flags delimited by the next
//! known subcommand name. clap has no native equivalent. This module:
//!
//! 1. Splits an argv slice into `(name, args)` chunks at every known
//!    subcommand boundary (see [`split`]).
//! 2. Detects orphan flags (a `--something` ahead of any subcommand) and
//!    surfaces them as [`CliError::OrphanFlag`].
//! 3. Iterates the chunks, parsing each via the corresponding step's
//!    clap parser and invoking its `run`.

#![allow(
    clippy::similar_names,
    reason = "clap-derived `args` and the temporary `argv` differ by one letter \
              but reflect distinct concepts; renaming would harm clarity"
)]
#![allow(
    clippy::wildcard_enum_match_arm,
    reason = "the dispatch table enumerates every known subcommand explicitly; \
              the trailing wildcard exists only to convert programmer error \
              into a typed `UnknownSubcommand`"
)]

use clap::Parser;

use crate::context::CliContext;
use crate::error::{CliError, Result};
use crate::steps::{
    self, analyze, find_clang_binary, floorplan, gcc, meta, pack, synth, version,
};

/// One chunk emitted by [`split`]: the subcommand name and its argv tail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk<'a> {
    pub name: &'a str,
    pub args: Vec<&'a str>,
}

/// Tokenize the chained argv into per-subcommand chunks.
///
/// `argv` is the suffix of the user's command line *after* the global
/// flags. Returns one [`Chunk`] per subcommand in left-to-right order.
pub fn split<'a>(argv: &'a [String]) -> Result<Vec<Chunk<'a>>> {
    let mut chunks: Vec<Chunk<'a>> = Vec::new();
    let mut i = 0;

    // Reject orphan flags appearing before any subcommand token.
    while i < argv.len() {
        let tok = argv[i].as_str();
        if tok.starts_with('-') {
            return Err(CliError::OrphanFlag {
                flag: tok.to_string(),
                pos: i,
            });
        }
        if !steps::is_known(tok) {
            return Err(CliError::UnknownSubcommand {
                token: tok.to_string(),
                pos: i,
            });
        }
        // Found the first subcommand; start a chunk and consume tokens
        // until the next known subcommand.
        let name = tok;
        i += 1;
        let mut args: Vec<&'a str> = Vec::new();
        while i < argv.len() && !boundary(name, argv[i].as_str()) {
            args.push(argv[i].as_str());
            i += 1;
        }
        chunks.push(Chunk { name, args });
    }

    Ok(chunks)
}

/// Return `true` when `token` should terminate the current chunk.
///
/// Tokens that look like another subcommand boundary close the chunk —
/// except for `g++`, whose argv intentionally swallows everything.
fn boundary(current: &str, token: &str) -> bool {
    if current == "g++" {
        return false;
    }
    steps::is_known(token)
}

/// Run the parsed chunks against `ctx`. Each chunk's step is parsed by
/// clap and dispatched to its `run` handler.
pub fn dispatch(chunks: Vec<Chunk<'_>>, ctx: &mut CliContext) -> Result<()> {
    for chunk in chunks {
        run_one(&chunk, ctx)?;
    }
    Ok(())
}

fn run_one(chunk: &Chunk<'_>, ctx: &mut CliContext) -> Result<()> {
    // clap expects argv[0] to be the program name.
    let mut argv = vec![chunk.name];
    argv.extend(chunk.args.iter().copied());

    match chunk.name {
        "version" => {
            let args = parse_chunk::<version::Args>(chunk.name, &argv)?;
            version::run(&args, ctx)
        }
        "g++" => {
            let args = parse_chunk::<gcc::Args>(chunk.name, &argv)?;
            gcc::run(&args, ctx)
        }
        "find-clang-binary" => {
            let args = parse_chunk::<find_clang_binary::Args>(chunk.name, &argv)?;
            find_clang_binary::run(&args, ctx)
        }
        "analyze" => {
            let args = parse_chunk::<analyze::Args>(chunk.name, &argv)?;
            analyze::run(&args, ctx)
        }
        "synth" => {
            let args = parse_chunk::<synth::Args>(chunk.name, &argv)?;
            synth::run(&args, ctx)
        }
        "pack" => {
            let args = parse_chunk::<pack::Args>(chunk.name, &argv)?;
            pack::run(&args, ctx)
        }
        "floorplan" => {
            let args = parse_chunk::<floorplan::FloorplanArgs>(chunk.name, &argv)?;
            floorplan::run_floorplan(&args, ctx)
        }
        "generate-floorplan" => {
            let args =
                parse_chunk::<floorplan::GenerateFloorplanArgs>(chunk.name, &argv)?;
            floorplan::run_generate_floorplan(&args, ctx)
        }
        "compile" => {
            let args = parse_chunk::<meta::CompileArgs>(chunk.name, &argv)?;
            meta::run_compile(&args, ctx)
        }
        "compile-with-floorplan-dse" => {
            let args =
                parse_chunk::<meta::CompileWithFloorplanDseArgs>(chunk.name, &argv)?;
            meta::run_compile_with_floorplan_dse(&args, ctx)
        }
        other => Err(CliError::UnknownSubcommand {
            token: other.to_string(),
            pos: 0,
        }),
    }
}

fn parse_chunk<T: Parser>(step: &str, argv: &[&str]) -> Result<T> {
    match T::try_parse_from(argv) {
        Ok(v) => Ok(v),
        Err(e) => match e.kind() {
            clap::error::ErrorKind::DisplayHelp
            | clap::error::ErrorKind::DisplayVersion => {
                // `--help` / `--version` are graceful exits, not errors.
                let _ = e.print();
                std::process::exit(0);
            }
            _ => Err(CliError::ClapParse {
                step: step.to_string(),
                message: e.to_string(),
            }),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn splits_three_step_chain() {
        let v = argv(&[
            "analyze",
            "--input",
            "vadd.cpp",
            "--top",
            "VecAdd",
            "synth",
            "--platform",
            "xilinx_u250",
            "pack",
            "--output",
            "vadd.xo",
        ]);
        let chunks = split(&v).unwrap();
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].name, "analyze");
        assert_eq!(
            chunks[0].args,
            vec!["--input", "vadd.cpp", "--top", "VecAdd"]
        );
        assert_eq!(chunks[1].name, "synth");
        assert_eq!(chunks[1].args, vec!["--platform", "xilinx_u250"]);
        assert_eq!(chunks[2].name, "pack");
        assert_eq!(chunks[2].args, vec!["--output", "vadd.xo"]);
    }

    #[test]
    fn unknown_first_token_is_typed_error() {
        let v = argv(&["bogus", "--flag"]);
        let err = split(&v).unwrap_err();
        match err {
            CliError::UnknownSubcommand { token, pos } => {
                assert_eq!(token, "bogus");
                assert_eq!(pos, 0);
            }
            other => panic!("expected UnknownSubcommand, got {other:?}"),
        }
    }

    #[test]
    fn stray_token_after_step_surfaces_via_clap() {
        // `bogus` after `analyze --top T` is absorbed into the analyze
        // chunk; clap will reject it as an unrecognized argument with a
        // diagnostic that names the offending token.
        let v = argv(&["analyze", "--top", "T", "bogus"]);
        let chunks = split(&v).unwrap();
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].args.contains(&"bogus"));
    }

    #[test]
    fn orphan_flag_errors_with_position() {
        let v = argv(&["--top", "T", "analyze"]);
        let err = split(&v).unwrap_err();
        match err {
            CliError::OrphanFlag { flag, pos } => {
                assert_eq!(flag, "--top");
                assert_eq!(pos, 0);
            }
            other => panic!("expected OrphanFlag, got {other:?}"),
        }
    }

    #[test]
    fn gcc_swallows_all_trailing_tokens_including_subcommand_lookalikes() {
        // `g++` must not let `version` reset the chunk boundary.
        let v = argv(&[
            "g++", "-O2", "main.cpp", "-o", "main", "version",
        ]);
        let chunks = split(&v).unwrap();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].name, "g++");
        assert_eq!(chunks[0].args.last(), Some(&"version"));
    }

    #[test]
    fn empty_argv_is_ok() {
        let v: Vec<String> = Vec::new();
        let chunks = split(&v).unwrap();
        assert!(chunks.is_empty());
    }

    #[test]
    fn analyze_only_chain() {
        let v = argv(&["analyze", "--input", "a.cpp", "--top", "T"]);
        let chunks = split(&v).unwrap();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].name, "analyze");
    }

    #[test]
    fn analyze_synth_chain() {
        let v = argv(&[
            "analyze", "--input", "a.cpp", "--top", "T", "synth", "--platform", "p",
        ]);
        let chunks = split(&v).unwrap();
        assert_eq!(chunks.len(), 2);
    }

    #[test]
    fn compile_chain_one_subcommand() {
        let v = argv(&["compile", "--input", "a.cpp", "--top", "T"]);
        let chunks = split(&v).unwrap();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].name, "compile");
    }
}
