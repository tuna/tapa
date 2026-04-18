//! Chained-subcommand dispatcher.
//!
//! `tapa analyze … synth … pack …` runs the three steps in order with
//! per-step flags delimited by the next subcommand name. clap has no
//! native chained-group derive, so each step's `Args` struct uses
//! `trailing_var_arg = true` to capture everything after its own flag
//! surface; the captured suffix is re-parsed here as the next [`Step`].
//!
//! This keeps clap responsible for option-arity decisions: a flag value
//! that happens to equal a subcommand name (e.g. `--top synth`) is
//! consumed by clap as the flag value, never as a chunk boundary.

use clap::{Parser, Subcommand};

use crate::context::CliContext;
use crate::error::{CliError, Result};
use crate::steps::{
    analyze, find_clang_binary, floorplan, gcc, meta, pack, synth, version,
};

/// One link in the chained-step list. Each variant carries its step's
/// `Args` (flags) plus a `chain_tail` positional that captures any
/// remaining argv for re-parsing as the next step.
#[derive(Debug, Subcommand)]
pub enum Step {
    /// Analyze TAPA program and store the program description.
    Analyze {
        #[command(flatten)]
        args: analyze::AnalyzeArgs,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, hide = true)]
        chain_tail: Vec<String>,
    },
    /// Synthesize the TAPA program into RTL.
    Synth {
        #[command(flatten)]
        args: synth::SynthArgs,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, hide = true)]
        chain_tail: Vec<String>,
    },
    /// Pack the generated RTL into a Xilinx object file.
    Pack {
        #[command(flatten)]
        args: pack::PackArgs,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, hide = true)]
        chain_tail: Vec<String>,
    },
    /// Floorplan TAPA program and store the program description.
    Floorplan {
        #[command(flatten)]
        args: floorplan::FloorplanArgs,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, hide = true)]
        chain_tail: Vec<String>,
    },
    /// Generate floorplan solution(s) via `AutoBridge`.
    #[command(name = "generate-floorplan")]
    GenerateFloorplan {
        #[command(flatten)]
        args: meta::GenerateFloorplanArgs,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, hide = true)]
        chain_tail: Vec<String>,
    },
    /// Compile a TAPA program (analyze + synth + pack) in one invocation.
    Compile {
        #[command(flatten)]
        args: meta::CompileArgs,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, hide = true)]
        chain_tail: Vec<String>,
    },
    /// Compile a TAPA program with floorplan design space exploration.
    #[command(name = "compile-with-floorplan-dse")]
    CompileWithFloorplanDse {
        #[command(flatten)]
        args: meta::CompileWithFloorplanDseArgs,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, hide = true)]
        chain_tail: Vec<String>,
    },
    /// Invoke g++ with TAPA include and library paths.
    ///
    /// Terminal: `g++`'s own `trailing_var_arg` already consumes any
    /// tokens that follow, so chaining a subsequent subcommand after
    /// `g++` is not supported — matching click's
    /// `nargs=-1, type=UNPROCESSED`.
    #[command(name = "g++")]
    Gpp {
        #[command(flatten)]
        args: gcc::GccArgs,
    },
    /// Print TAPA version to standard output.
    Version {
        #[command(flatten)]
        args: version::VersionArgs,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, hide = true)]
        chain_tail: Vec<String>,
    },
    /// Resolve a clang-family helper and print its absolute path.
    #[command(name = "find-clang-binary", hide = true)]
    FindClangBinary {
        #[command(flatten)]
        args: find_clang_binary::FindClangBinaryArgs,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, hide = true)]
        chain_tail: Vec<String>,
    },
}

/// Standalone parser for re-parsing the trailing chain.
#[derive(Debug, Parser)]
#[command(name = "tapa", disable_help_subcommand = true)]
struct ChainParser {
    #[command(subcommand)]
    step: Step,
}

impl Step {
    /// Walk the chained-step linked list. Mirrors click's chained
    /// group: parse + validate the *entire* chain first, then execute
    /// each step in order. A parse error or `--help` on a later token
    /// (e.g. `tapa analyze … synth --help`) must surface before any
    /// step mutates `work_dir` or shells out to a missing tool.
    pub fn execute(self, ctx: &mut CliContext) -> Result<()> {
        let mut steps: Vec<Self> = Vec::new();
        let mut current: Option<Self> = Some(self);
        while let Some(step) = current {
            let (head, tail) = step.split_chain();
            steps.push(head);
            current = if tail.is_empty() {
                None
            } else {
                Some(parse_chain_tail(&tail)?)
            };
        }
        for step in steps {
            step.run_one(ctx)?;
        }
        Ok(())
    }

    /// Detach the trailing-vararg payload from this step and return
    /// the head (with an empty tail) plus the captured tail tokens.
    /// `g++` is terminal — its own `trailing_var_arg` already
    /// consumed any chained tokens.
    fn split_chain(self) -> (Self, Vec<String>) {
        match self {
            Self::Analyze { args, chain_tail } => (
                Self::Analyze { args, chain_tail: Vec::new() },
                chain_tail,
            ),
            Self::Synth { args, chain_tail } => (
                Self::Synth { args, chain_tail: Vec::new() },
                chain_tail,
            ),
            Self::Pack { args, chain_tail } => (
                Self::Pack { args, chain_tail: Vec::new() },
                chain_tail,
            ),
            Self::Floorplan { args, chain_tail } => (
                Self::Floorplan { args, chain_tail: Vec::new() },
                chain_tail,
            ),
            Self::GenerateFloorplan { args, chain_tail } => (
                Self::GenerateFloorplan { args, chain_tail: Vec::new() },
                chain_tail,
            ),
            Self::Compile { args, chain_tail } => (
                Self::Compile { args, chain_tail: Vec::new() },
                chain_tail,
            ),
            Self::CompileWithFloorplanDse { args, chain_tail } => (
                Self::CompileWithFloorplanDse { args, chain_tail: Vec::new() },
                chain_tail,
            ),
            Self::Gpp { args } => (Self::Gpp { args }, Vec::new()),
            Self::Version { args, chain_tail } => (
                Self::Version { args, chain_tail: Vec::new() },
                chain_tail,
            ),
            Self::FindClangBinary { args, chain_tail } => (
                Self::FindClangBinary { args, chain_tail: Vec::new() },
                chain_tail,
            ),
        }
    }

    /// Dispatch a single step's side-effecting body. Called only
    /// after the whole chain has been parsed and validated.
    fn run_one(self, ctx: &mut CliContext) -> Result<()> {
        match self {
            Self::Analyze { args, .. } => analyze::run(&args, ctx),
            Self::Synth { args, .. } => synth::run(&args, ctx),
            Self::Pack { args, .. } => pack::run(&args, ctx),
            Self::Floorplan { args, .. } => floorplan::run_floorplan(&args, ctx),
            Self::GenerateFloorplan { args, .. } => {
                meta::run_generate_floorplan_composite(&args, ctx)
            }
            Self::Compile { args, .. } => meta::run_compile_composite(&args, ctx),
            Self::CompileWithFloorplanDse { args, .. } => {
                meta::run_compile_with_floorplan_dse_composite(&args, ctx)
            }
            Self::Gpp { args } => gcc::run(&args, ctx),
            Self::Version { args, .. } => version::run(&args, ctx),
            Self::FindClangBinary { args, .. } => find_clang_binary::run(&args, ctx),
        }
    }
}

fn parse_chain_tail(tail: &[String]) -> Result<Step> {
    // ChainParser expects argv[0] to be the program name.
    let mut argv: Vec<&str> = Vec::with_capacity(tail.len() + 1);
    argv.push("tapa");
    argv.extend(tail.iter().map(String::as_str));
    let parsed = ChainParser::try_parse_from(&argv).map_err(|e| {
        if matches!(
            e.kind(),
            clap::error::ErrorKind::DisplayHelp
                | clap::error::ErrorKind::DisplayVersion
        ) {
            // `--help` / `--version` in the chain tail are graceful
            // exits handled the same way as the top-level parser.
            let _ = e.print();
            std::process::exit(0);
        }
        CliError::ClapParse {
            step: "<chain>".to_string(),
            message: e.to_string(),
        }
    })?;
    Ok(parsed.step)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::globals::Cli;

    fn parse(args: &[&str]) -> std::result::Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once("tapa").chain(args.iter().copied()))
    }

    #[test]
    fn three_step_chain_parses() {
        let cli = parse(&[
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
        ])
        .expect("3-step chain must parse");
        match cli.step {
            Some(Step::Analyze { args, chain_tail }) => {
                assert_eq!(args.top, "VecAdd");
                assert_eq!(chain_tail.first().map(String::as_str), Some("synth"));
            }
            other => panic!("expected Analyze, got {other:?}"),
        }
    }

    #[test]
    fn flag_value_equal_to_subcommand_name_is_not_a_boundary() {
        // Regression for Codex bug: `--top synth` keeps `synth` as the
        // value of `--top`, not as a chained subcommand boundary.
        let cli = parse(&[
            "analyze",
            "--input",
            "a.cpp",
            "--top",
            "synth",
            "pack",
            "--output",
            "out.xo",
        ])
        .expect("flag value `synth` must not boundary the chunk");
        match cli.step {
            Some(Step::Analyze { args, chain_tail }) => {
                assert_eq!(args.top, "synth");
                assert_eq!(chain_tail.first().map(String::as_str), Some("pack"));
            }
            other => panic!("expected Analyze, got {other:?}"),
        }
    }

    #[test]
    fn global_flag_value_equal_to_subcommand_name_is_not_a_boundary() {
        let cli = parse(&["--work-dir", "synth", "version"])
            .expect("global `--work-dir synth` must not boundary on subcommand name");
        assert_eq!(
            cli.globals.work_dir.display().to_string(),
            "synth",
            "the literal `synth` must be captured as the work-dir value",
        );
        assert!(matches!(cli.step, Some(Step::Version { .. })));
    }

    #[test]
    fn unknown_first_token_errors() {
        let err = parse(&["bogus-subcommand"]).expect_err("unknown subcommand fails");
        assert!(
            err.to_string().contains("unrecognized")
                || err.to_string().contains("invalid")
                || err.to_string().contains("unexpected"),
            "error must point at the bad token; got `{err}`",
        );
    }

    #[test]
    fn analyze_synth_chain() {
        let cli = parse(&[
            "analyze", "--input", "a.cpp", "--top", "T", "synth", "--platform", "p",
        ])
        .unwrap();
        match cli.step {
            Some(Step::Analyze { chain_tail, .. }) => {
                assert_eq!(chain_tail, vec!["synth", "--platform", "p"]);
            }
            other => panic!("expected Analyze, got {other:?}"),
        }
    }

    #[test]
    fn version_subcommand_alone() {
        let cli = parse(&["version"]).unwrap();
        assert!(matches!(cli.step, Some(Step::Version { .. })));
    }

    #[test]
    fn no_subcommand_yields_none() {
        let cli = parse(&[]).unwrap();
        assert!(cli.step.is_none());
    }

    #[test]
    fn gcc_swallows_following_subcommand_tokens() {
        let cli = parse(&["g++", "-O2", "main.cpp", "-o", "main", "version"]).unwrap();
        match cli.step {
            Some(Step::Gpp { args }) => {
                assert!(args.argv.contains(&"version".to_string()));
            }
            other => panic!("expected Gpp, got {other:?}"),
        }
    }

    #[test]
    fn compile_exposes_unioned_flag_surface() {
        let cli = parse(&[
            "compile",
            "--input",
            "a.cpp",
            "--top",
            "T",
            "--platform",
            "p",
            "--output",
            "out.xo",
        ])
        .expect("compile must accept the unioned flag surface");
        match cli.step {
            Some(Step::Compile { args, .. }) => {
                assert_eq!(args.analyze.top, "T");
                assert_eq!(args.synth.platform.as_deref(), Some("p"));
                assert_eq!(
                    args.pack.output.as_ref().map(|p| p.display().to_string()),
                    Some("out.xo".to_string()),
                );
            }
            other => panic!("expected Compile, got {other:?}"),
        }
    }

    #[test]
    fn generate_floorplan_exposes_unioned_flag_surface() {
        let cli = parse(&[
            "generate-floorplan",
            "--input",
            "a.cpp",
            "--top",
            "T",
            "--platform",
            "p",
            "--device-config",
            "dev.json",
            "--floorplan-config",
            "fp.json",
        ])
        .expect("generate-floorplan must accept analyze+synth+autobridge flags");
        assert!(matches!(cli.step, Some(Step::GenerateFloorplan { .. })));
    }
}
