//! `tapa aiesim`: run the AI Engine half of a design in `x86simulator` (or,
//! with `--cycle-accurate`, `aiesimulator`) against PLIO stimulus recorded
//! by software simulation, and compare what it writes back.
//!
//! Software simulation with `TAPA_STREAM_LOG_DIR=DIR` writes `<name>.words`
//! for every named stream; a boundary stream is found by its FIFO name, so
//! name it after its variable (`tapa::stream<T> a_q("a_q")`). The graph is
//! built under `<work_dir>/aie/sim/`.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;

use clap::Parser;
use tapa_ir::{AiePartition, AieStream, AieStreamKind};
use tapa_xilinx::tools::aie::graph::SimMain;
use tapa_xilinx::tools::aie::{plio, simulate, simulator_output, AieTarget};

use crate::aie::{device, dir, pl_freq_mhz, prepare, GraphBuild};
use crate::context::CliContext;
use crate::error::{CliError, Result};
use crate::state::work as work_io;
use crate::steps::synth::report::{fold_aiesim_timing, AiesimSection, AiesimStreamTiming};

#[derive(Debug, Clone, Parser)]
#[command(
    name = "aiesim",
    about = "Simulate the AI Engine tasks against recorded stream data."
)]
pub struct AiesimArgs {
    /// Directory of `<stream>.words` files written by software simulation
    /// with `TAPA_STREAM_LOG_DIR`.
    #[arg(long = "stimulus", value_name = "DIR", required = true)]
    pub stimulus: PathBuf,

    /// Value of a top-task scalar the AI Engine tasks read, `NAME=VALUE`.
    #[arg(long = "arg", value_name = "NAME=VALUE")]
    pub args: Vec<String>,

    /// Run the cycle-approximate `aiesimulator` instead of `x86simulator`,
    /// and report the time of each output stream's beats.
    #[arg(long = "cycle-accurate")]
    pub cycle_accurate: bool,

    /// Vitis platform; defaults to the one `tapa synth` used.
    #[arg(short = 'p', long = "platform", value_name = "PLATFORM")]
    pub platform: Option<String>,

    /// Part number, when no platform is known.
    #[arg(long = "part-num", value_name = "PART")]
    pub part_num: Option<String>,

    /// AI Engine cycles a never-returning graph runs before it is ended.
    #[arg(long = "cycles", value_name = "N", default_value_t = 100_000)]
    pub cycles: u64,
}

pub fn run(args: &AiesimArgs, ctx: &CliContext) -> Result<()> {
    ctx.with_tool_runner(|runner| run_with(args, ctx, runner))
}

fn read_words(path: &std::path::Path) -> Result<Vec<u32>> {
    let text = fs::read_to_string(path).map_err(|error| {
        CliError::InvalidArg(format!(
            "cannot read the stream log `{}`: {error}; run software simulation with \
             `TAPA_STREAM_LOG_DIR` and name the stream after its variable",
            path.display()
        ))
    })?;
    text.split_whitespace()
        .map(|word| {
            word.parse::<u32>().map_err(|error| {
                CliError::InvalidArg(format!(
                    "`{}` holds `{word}`, not a 32-bit word: {error}",
                    path.display()
                ))
            })
        })
        .collect()
}

fn transfer_words(stream: &AieStream) -> usize {
    let AieStreamKind::Plio { width, .. } = stream.kind else {
        unreachable!("only a PLIO has a transfer shape")
    };
    let beat_words = (width / 32).max(1) as usize;
    let frame_words = stream.frame.map_or(beat_words, |elements| {
        elements as usize * stream.words as usize
    });
    plio::transfer_words(beat_words, frame_words)
}

/// Compare one complete output stream, allowing only zero padding in its last
/// PLIO beat. Returns the status line on either success or mismatch.
fn compare_output(
    fifo: &str,
    transfer_words: usize,
    trace: &plio::PlioTrace,
    expected: &[u32],
) -> std::result::Result<String, String> {
    let produced = &trace.words;
    let padded_len = expected.len().next_multiple_of(transfer_words.max(1));
    let first_diff = produced
        .iter()
        .zip(expected)
        .position(|(got, want)| got != want);
    let mut mismatch = first_diff;
    if mismatch.is_none() {
        if produced.len() < expected.len() {
            mismatch = Some(produced.len());
        } else if produced.len() > padded_len {
            mismatch = Some(padded_len);
        } else if let Some(index) = produced[expected.len()..]
            .iter()
            .position(|word| *word != 0)
        {
            mismatch = Some(expected.len() + index);
        }
    }

    let mut line = format!("{fifo}: {} of {} words", produced.len(), expected.len());
    if let (Some(first), Some(last)) = (trace.beat_times_ns.first(), trace.beat_times_ns.last()) {
        let gaps = u32::try_from(trace.beat_times_ns.len() - 1).unwrap_or(u32::MAX);
        let spacing = if gaps > 0 {
            (last - first) / f64::from(gaps)
        } else {
            0.0
        };
        let _ = write!(
            line,
            ", beats at {first:.1}..{last:.1} ns ({spacing:.2} ns apart)"
        );
    }
    if let Some(index) = mismatch {
        if index < expected.len() && index < produced.len() {
            let _ = write!(
                line,
                ", first mismatch at word {index}: got {}, expected {}",
                produced[index], expected[index]
            );
        } else {
            let _ = write!(line, ", unexpected data at word {index}");
        }
        Err(line)
    } else {
        Ok(line)
    }
}

fn run_with(
    args: &AiesimArgs,
    ctx: &CliContext,
    runner: &dyn tapa_xilinx::ToolRunner,
) -> Result<()> {
    let state = work_io::load(&ctx.work_dir)?;
    let partition = AiePartition::derive(&state.graph)?.ok_or_else(|| {
        CliError::InvalidArg("the design has no AI Engine task to simulate".to_string())
    })?;
    let mut sim = SimMain {
        args: BTreeMap::new(),
        cycles: args.cycles,
    };
    for arg in &args.args {
        let (name, value) = arg
            .split_once('=')
            .ok_or_else(|| CliError::InvalidArg(format!("`--arg {arg}` is not NAME=VALUE")))?;
        sim.args.insert(name.to_string(), value.to_string());
    }
    let target = if args.cycle_accurate {
        AieTarget::Hw
    } else {
        AieTarget::X86sim
    };
    let platform = args.platform.as_deref().or(state.flow.platform.as_deref());
    // An explicit part is an override for both HLS and AIE. An explicit
    // platform instead supersedes the part persisted by an earlier run.
    let part = if args.part_num.is_some() {
        args.part_num.as_deref()
    } else if args.platform.is_some() {
        None
    } else {
        state.flow.part_num.as_deref()
    };
    let graph_dir = dir(&ctx.work_dir, "sim");
    let job = prepare(
        &ctx.work_dir,
        &state.graph,
        &partition,
        GraphBuild {
            dir: graph_dir.clone(),
            target,
            device: device(platform, part)?,
            pl_freq_mhz: pl_freq_mhz(&state),
            pl_control: false,
            sim: Some(sim),
        },
    )?;
    for stream in &partition.streams {
        if let AieStreamKind::Plio { input: true, width } = stream.kind {
            let words = read_words(&args.stimulus.join(format!("{}.words", stream.fifo)))?;
            fs::write(
                graph_dir.join("data").join(format!("{}.txt", stream.name)),
                plio::render_input(&words, (width / 32) as usize, transfer_words(stream)),
            )?;
        }
    }
    tapa_xilinx::tools::aie::compile(runner, &job)?;
    simulate(runner, &graph_dir, target)?;

    let mut mismatches = Vec::new();
    let mut measured = BTreeMap::new();
    for stream in &partition.streams {
        let AieStreamKind::Plio { input: false, .. } = stream.kind else {
            continue;
        };
        let output = simulator_output(&graph_dir, target, &stream.name);
        let trace = plio::parse_output(&fs::read_to_string(&output)?);
        let expected = read_words(&args.stimulus.join(format!("{}.words", stream.fifo)))?;
        if args.cycle_accurate {
            if let Some(timing) = AiesimStreamTiming::from_beat_times(&trace.beat_times_ns) {
                measured.insert(stream.fifo.clone(), timing);
            }
        }
        let result = compare_output(&stream.fifo, transfer_words(stream), &trace, &expected);
        if result.is_err() {
            mismatches.push(stream.fifo.clone());
        }
        match result {
            Ok(line) | Err(line) => println!("{line}"),
        }
    }
    if mismatches.is_empty() {
        println!("AI Engine simulation matches software simulation");
        // Only `aiesimulator` times beats, and only a fully matching run may
        // touch the reports a previous `tapa synth` wrote; with no root
        // report (analyze-only work dir) this stays a successful no-op.
        if let Some(timing) = timing_to_fold(
            args.cycle_accurate,
            &mismatches,
            state.flow.synthed,
            measured,
        ) {
            let updated = fold_aiesim_timing(&ctx.work_dir, &timing)?;
            if !updated.is_empty() {
                println!("aiesim timing folded into {}", updated.join(", "));
            }
        }
        Ok(())
    } else {
        Err(CliError::Report(format!(
            "AI Engine simulation differs from software simulation on {}",
            mismatches.join(", ")
        )))
    }
}

/// The timing this run may fold into the root reports, if any: only a
/// `--cycle-accurate` run measures PLIO timing, and any output mismatch
/// must leave prior reports untouched. `report_is_current` is the persisted
/// synth flag: a later analyze invalidates it but may leave old report files
/// in the reused work directory, and those stale files must not be updated.
fn timing_to_fold(
    cycle_accurate: bool,
    mismatches: &[String],
    report_is_current: bool,
    streams: BTreeMap<String, AiesimStreamTiming>,
) -> Option<AiesimSection> {
    (cycle_accurate && mismatches.is_empty() && report_is_current)
        .then_some(AiesimSection { streams })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trace(words: &[u32]) -> plio::PlioTrace {
        plio::PlioTrace {
            words: words.to_vec(),
            beat_times_ns: Vec::new(),
        }
    }

    #[test]
    fn output_comparison_rejects_short_surplus_and_nonzero_padding() {
        compare_output("q", 1, &trace(&[1, 2]), &[1, 2]).unwrap();
        compare_output("q", 1, &trace(&[1]), &[1, 2]).unwrap_err();
        compare_output("q", 1, &trace(&[1, 2, 1, 2]), &[1, 2]).unwrap_err();
        compare_output("q", 1, &trace(&[9]), &[]).unwrap_err();
        compare_output("q", 4, &trace(&[1, 2, 3, 0]), &[1, 2, 3]).unwrap();
        compare_output("q", 4, &trace(&[1, 2, 3, 4]), &[1, 2, 3]).unwrap_err();
        compare_output("q", 4, &trace(&[1, 2, 0, 0, 0]), &[1, 2]).unwrap_err();
        let padded = [1, 2, 0, 0, 0, 0, 0, 0];
        compare_output("q", 12, &trace(&padded), &[1, 2]).unwrap();
        let mut non_multiple = padded;
        non_multiple[3] = 9;
        compare_output("q", 12, &trace(&non_multiple), &[1, 2]).unwrap_err();
    }

    #[test]
    fn only_fully_matching_cycle_accurate_runs_fold_timing() {
        let measured = BTreeMap::from([(
            "c_q".to_string(),
            AiesimStreamTiming::from_beat_times(&[240.0, 256.0]).expect("two beats"),
        )]);
        let folded = timing_to_fold(true, &[], true, measured.clone()).expect("fold");
        assert_eq!(folded.streams, measured);
        assert!(
            timing_to_fold(false, &[], true, measured.clone()).is_none(),
            "an x86sim run has no PLIO timing to fold"
        );
        assert!(
            timing_to_fold(true, &["c_q".to_string()], true, measured.clone()).is_none(),
            "a mismatch must leave prior reports untouched"
        );
        assert!(
            timing_to_fold(true, &[], false, measured).is_none(),
            "reports from before a later analyze are stale"
        );
    }
}
