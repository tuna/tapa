//! `tapa compile --aie-sim`: the AI Engine graph of a design, built for
//! `x86simulator` or `aiesimulator` into a package directory the host
//! program runs with `--bitstream=<package>`.
//!
//! The package holds the compiled graph (`Work/`), whose simulation entry
//! point reads the runtime parameters from `rtp.txt`, and `manifest.json`,
//! which tells the runtime how the host's arguments and streams reach it.
//! The runtime's AI Engine simulation backend (`frt`) reads the manifest;
//! the two structs below and its reader are one contract.

use std::path::Path;

use serde::Serialize;
use tapa_ir::{AiePartition, AieStreamKind};
use tapa_xilinx::tools::aie::graph::{runtime_parameters, SimMain};
use tapa_xilinx::tools::aie::AieTarget;

use crate::aie::{device, kernel_args, pl_freq_mhz, prepare, GraphBuild};
use crate::context::CliContext;
use crate::error::{CliError, Result};
use crate::state::work as work_io;

/// The simulator a package is built for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum AieSim {
    /// The functional `x86simulator`.
    X86sim,
    /// The cycle-approximate `aiesimulator`, which needs the AIE licence.
    Aiesim,
}

/// The file naming a directory as a package, and what it says.
pub const MANIFEST: &str = "manifest.json";

/// Cycles a graph that never returns runs before the host ends it.
const RUN_FOREVER_CYCLES: u64 = 100_000;

#[derive(Serialize)]
struct Manifest {
    /// `x86sim` or `aiesim`.
    simulator: &'static str,
    /// The top scalar arguments, by the kernel argument index the host
    /// sets them at.
    scalars: Vec<ScalarArg>,
    /// The scalar behind each line of `rtp.txt`, in order.
    runtime_parameters: Vec<String>,
    /// The PL <-> AI Engine streams, which the host binds as stream
    /// arguments after the top's own.
    plios: Vec<Plio>,
    /// The upper task instances the graph is built with inlined, whose
    /// bodies the host runs itself to bind the streams inside them.
    inlined: Vec<tapa_ir::InlinedInstance>,
}

#[derive(Serialize)]
struct ScalarArg {
    index: u32,
    name: String,
}

#[derive(Serialize)]
struct Plio {
    index: u32,
    /// The PLIO's logical name, which is also its file name under `data/`.
    name: String,
    /// The PL writes it and the AI Engine reads it.
    input: bool,
    /// 32-bit words per stream element.
    element_words: u32,
    /// 32-bit words per PLIO beat, one line of its file.
    beat_words: u32,
    /// 32-bit words in a whole transfer, a whole beat and a whole frame:
    /// a final partial one is zero-padded on input.
    transfer_words: u32,
}

/// Build the package for `sim` at `output` from the analyzed work dir.
pub fn package(
    sim: AieSim,
    platform: Option<&str>,
    part: Option<&str>,
    output: Option<&Path>,
    ctx: &CliContext,
) -> Result<()> {
    let output = output.ok_or_else(|| {
        CliError::InvalidArg("`--aie-sim` builds a package at `--output`; name it".to_string())
    })?;
    let output = std::path::absolute(output)?;
    // The build wipes its directory, so it may only replace a package.
    if output.exists() && !output.join(MANIFEST).is_file() {
        return Err(CliError::InvalidArg(format!(
            "`{}` exists and is not an AI Engine simulation package; name a new path",
            output.display()
        )));
    }
    let state = work_io::load(&ctx.work_dir)?;
    let partition = AiePartition::derive(&state.graph)?.ok_or_else(|| {
        CliError::InvalidArg("`--aie-sim` needs an AI Engine task; the design has none".into())
    })?;
    let job = prepare(
        &ctx.work_dir,
        &state.graph,
        &partition,
        GraphBuild {
            dir: crate::util::utf8(&output),
            target: match sim {
                AieSim::X86sim => AieTarget::X86sim,
                AieSim::Aiesim => AieTarget::Hw,
            },
            device: device(platform, part)?,
            pl_freq_mhz: pl_freq_mhz(&state),
            pl_control: false,
            sim: Some(SimMain {
                cycles: RUN_FOREVER_CYCLES,
            }),
        },
    )?;
    ctx.with_tool_runner(|runner| -> Result<()> {
        Ok(tapa_xilinx::tools::aie::compile(runner, &job)?)
    })?;
    let manifest = manifest(sim, &state.graph, &partition)?;
    let text = serde_json::to_string_pretty(&manifest)
        .map_err(|e| CliError::Report(format!("cannot serialize {MANIFEST}: {e}")))?;
    std::fs::write(output.join(MANIFEST), text)?;
    log::info!(
        "AI Engine {} package written to {}",
        manifest.simulator,
        output.display()
    );
    Ok(())
}

fn manifest(sim: AieSim, graph: &tapa_ir::Graph, partition: &AiePartition) -> Result<Manifest> {
    let args = kernel_args(graph, partition)?;
    let index_of = |name: &str| {
        args.iter()
            .find(|(_, port)| port.name == name)
            .map(|(index, _)| *index)
            .ok_or_else(|| CliError::InvalidArg(format!("the PL kernel has no argument `{name}`")))
    };
    let mut plios = Vec::new();
    for stream in &partition.streams {
        let AieStreamKind::Plio { input, width } = stream.kind else {
            continue;
        };
        let beat_words = width / 32;
        let frame_words = stream.frame.map_or(1, |frame| frame * stream.words);
        plios.push(Plio {
            index: index_of(&stream.fifo)?,
            name: stream.name.clone(),
            input,
            element_words: stream.words,
            beat_words,
            transfer_words: lcm(beat_words, frame_words),
        });
    }
    Ok(Manifest {
        simulator: match sim {
            AieSim::X86sim => "x86sim",
            AieSim::Aiesim => "aiesim",
        },
        scalars: args
            .iter()
            .filter(|(_, port)| port.cat.is_scalar())
            .map(|(index, port)| ScalarArg {
                index: *index,
                name: port.name.clone(),
            })
            .collect(),
        runtime_parameters: runtime_parameters(partition),
        plios,
        inlined: tapa_ir::inlined_instances(graph)?,
    })
}

/// The least common multiple, or 0 when either is 0.
fn lcm(first: u32, second: u32) -> u32 {
    let (mut gcd, mut rest) = (first, second);
    while rest != 0 {
        (gcd, rest) = (rest, gcd % rest);
    }
    first.checked_div(gcd).map_or(0, |part| part * second)
}
