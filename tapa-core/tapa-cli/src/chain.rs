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
use crate::state::work as work_io;
use crate::steps::registry::{self, StepSpec};
use crate::steps::{analyze, floorplan, gcc, meta, pack, synth, version};
use crate::tapacc::find_clang_binary;

/// A pipeline step paired with its concrete arguments.
#[derive(Clone, Copy)]
pub(crate) enum PipelineStep<'a> {
    Analyze(&'a analyze::AnalyzeArgs),
    Synth(&'a synth::SynthArgs),
    Floorplan(&'a floorplan::FloorplanArgs),
    Pack(&'a pack::PackArgs),
}

impl<'a> PipelineStep<'a> {
    fn spec(self) -> &'static StepSpec {
        match self {
            Self::Analyze(_) => registry::analyze(),
            Self::Synth(_) => registry::synth(),
            Self::Floorplan(_) => registry::floorplan(),
            Self::Pack(_) => registry::pack(),
        }
    }

    fn precondition_args(self) -> Option<&'a pack::PackArgs> {
        match self {
            Self::Pack(args) => Some(args),
            Self::Analyze(_) | Self::Synth(_) | Self::Floorplan(_) => None,
        }
    }
}

/// Resolve and validate a pipeline step's registry entry before dispatch.
pub(crate) fn validate_pipeline_step(step: PipelineStep<'_>, ctx: &CliContext) -> Result<()> {
    let spec = step.spec();
    if spec.preconditions.is_empty() {
        return Ok(());
    }
    let state = work_io::load(&ctx.work_dir)?;
    registry::validate(
        spec,
        &state,
        &work_io::path_in(&ctx.work_dir),
        step.precondition_args(),
    )
}

/// Validate and dispatch one pipeline step through the shared machinery.
pub(crate) fn run_pipeline_step(step: PipelineStep<'_>, ctx: &CliContext) -> Result<()> {
    validate_pipeline_step(step, ctx)?;
    match step {
        PipelineStep::Analyze(args) => analyze::run(args, ctx),
        PipelineStep::Synth(args) => synth::run(args, ctx),
        PipelineStep::Floorplan(args) => floorplan::run(args, ctx),
        PipelineStep::Pack(args) => pack::run(args, ctx),
    }
}

/// Tokens captured after a step's own flags, re-parsed as the next step.
#[derive(Debug, clap::Args)]
pub struct ChainTail {
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, hide = true)]
    pub chain_tail: Vec<String>,
}

/// One link in the chained-step list. Each variant carries its step's
/// `Args` (flags) plus a `chain_tail` positional that captures any
/// remaining argv for re-parsing as the next step.
#[allow(
    clippy::large_enum_variant,
    reason = "a clap dispatch enum: each variant must `#[command(flatten)]` its \
              step's Args struct inline, so the differently-sized Args (Compile \
              bundles the whole flow) cannot be boxed without breaking the derive"
)]
#[derive(Debug, Subcommand)]
pub enum Step {
    /// Analyze TAPA program and store the program description.
    Analyze {
        #[command(flatten)]
        args: analyze::AnalyzeArgs,
        #[command(flatten)]
        chain_tail: ChainTail,
    },
    /// Synthesize the TAPA program into RTL.
    Synth {
        #[command(flatten)]
        args: synth::SynthArgs,
        #[command(flatten)]
        chain_tail: ChainTail,
    },
    /// Coarse-grained floorplan the synthesized design.
    Floorplan {
        #[command(flatten)]
        args: floorplan::FloorplanArgs,
        #[command(flatten)]
        chain_tail: ChainTail,
    },
    /// Pack the generated RTL into a Xilinx object file.
    Pack {
        #[command(flatten)]
        args: pack::PackArgs,
        #[command(flatten)]
        chain_tail: ChainTail,
    },
    /// Compile a TAPA program (analyze + synth + pack) in one invocation.
    Compile {
        #[command(flatten)]
        args: meta::CompileArgs,
        #[command(flatten)]
        chain_tail: ChainTail,
    },
    /// Invoke g++ with TAPA include and library paths.
    ///
    /// Terminal: `g++`'s own `trailing_var_arg` already consumes any
    /// tokens that follow, so chaining a subsequent subcommand after
    /// `g++` is not supported.
    #[command(name = "g++")]
    Gpp {
        #[command(flatten)]
        args: gcc::GccArgs,
    },
    /// Print TAPA version to standard output.
    Version {
        #[command(flatten)]
        args: version::VersionArgs,
        #[command(flatten)]
        chain_tail: ChainTail,
    },
    /// Resolve a clang-family helper and print its absolute path.
    #[command(name = "find-clang-binary", hide = true)]
    FindClangBinary {
        #[command(flatten)]
        args: find_clang_binary::FindClangBinaryArgs,
        #[command(flatten)]
        chain_tail: ChainTail,
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
    /// Walk the chained-step linked list: parse the *entire* chain
    /// first, then validate and execute each step in order (precondition
    /// validation is deliberately per-step, immediately before each
    /// dispatch). A parse error or `--help` on a later token
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
    fn split_chain(mut self) -> (Self, Vec<String>) {
        let tail = self
            .chain_tail_mut()
            .map(std::mem::take)
            .unwrap_or_default();
        (self, tail)
    }

    /// Mutable accessor for the per-variant `chain_tail` field, or
    /// `None` for terminal variants (`g++`).
    fn chain_tail_mut(&mut self) -> Option<&mut Vec<String>> {
        match self {
            Self::Analyze { chain_tail, .. }
            | Self::Synth { chain_tail, .. }
            | Self::Floorplan { chain_tail, .. }
            | Self::Pack { chain_tail, .. }
            | Self::Compile { chain_tail, .. }
            | Self::Version { chain_tail, .. }
            | Self::FindClangBinary { chain_tail, .. } => Some(&mut chain_tail.chain_tail),
            Self::Gpp { .. } => None,
        }
    }

    /// Dispatch a single step's side-effecting body. Called only
    /// after the whole chain has been parsed and validated.
    fn run_one(self, ctx: &mut CliContext) -> Result<()> {
        match self {
            Self::Analyze { args, .. } => run_pipeline_step(PipelineStep::Analyze(&args), ctx),
            Self::Synth { args, .. } => run_pipeline_step(PipelineStep::Synth(&args), ctx),
            Self::Floorplan { args, .. } => run_pipeline_step(PipelineStep::Floorplan(&args), ctx),
            Self::Pack { args, .. } => run_pipeline_step(PipelineStep::Pack(&args), ctx),
            Self::Compile { args, .. } => meta::run_compile_composite(&args, ctx),
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
            clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
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
            Some(Step::Analyze {
                args,
                chain_tail: tail,
            }) => {
                assert_eq!(args.top, "VecAdd");
                assert_eq!(tail.chain_tail.first().map(String::as_str), Some("synth"));
            }
            other => panic!("expected Analyze, got {other:?}"),
        }
    }

    #[test]
    fn flag_value_equal_to_subcommand_name_is_not_a_boundary() {
        // `synth` is the value of `--top`, not a chained subcommand
        // boundary.
        let cli = parse(&[
            "analyze", "--input", "a.cpp", "--top", "synth", "pack", "--output", "out.xo",
        ])
        .expect("flag value `synth` must not boundary the chunk");
        match cli.step {
            Some(Step::Analyze {
                args,
                chain_tail: tail,
            }) => {
                assert_eq!(args.top, "synth");
                assert_eq!(tail.chain_tail.first().map(String::as_str), Some("pack"));
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
            "analyze",
            "--input",
            "a.cpp",
            "--top",
            "T",
            "synth",
            "--platform",
            "p",
        ])
        .unwrap();
        match cli.step {
            Some(Step::Analyze {
                chain_tail: tail, ..
            }) => {
                assert_eq!(tail.chain_tail, ["synth", "--platform", "p"]);
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
}
