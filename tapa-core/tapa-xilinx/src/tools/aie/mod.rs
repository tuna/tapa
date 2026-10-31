//! AI Engine orchestration: render the ADF graph of an AI Engine partition,
//! compile it with `aiecompiler`, and run it in `x86simulator` or
//! `aiesimulator` against PLIO stimulus.

pub mod graph;
pub mod pl_control;
pub mod plio;
pub mod ps_control;
pub mod report;

use camino::{Utf8Path, Utf8PathBuf};
use typed_builder::TypedBuilder;

use crate::error::Result;
use crate::runtime::process::{ToolInvocation, ToolRunner};
use crate::tools::tool_failure;
use crate::ToolValue;

/// What `aiecompiler` builds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AieTarget {
    /// A functional model for `x86simulator`; needs no licence.
    X86sim,
    /// The array configuration (`libadf.a`), also run by `aiesimulator`.
    Hw,
}

impl AieTarget {
    const fn as_str(self) -> &'static str {
        match self {
            Self::X86sim => "x86sim",
            Self::Hw => "hw",
        }
    }
}

/// The device an AI Engine graph is compiled for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AieDevice {
    /// A Vitis platform's `.xpfm`.
    Platform(Utf8PathBuf),
    /// A part number, for releases whose `aiecompiler` takes `--part`.
    Part(String),
}

/// One `aiecompiler` run over a graph directory holding `graph.cpp`.
#[derive(Debug, Clone, TypedBuilder)]
pub struct AieCompileJob {
    /// The graph directory; the compiler runs there and writes `Work/` and
    /// `libadf.a`.
    pub graph_dir: Utf8PathBuf,
    pub target: AieTarget,
    pub device: AieDevice,
    /// Kernel include directories.
    #[builder(default)]
    pub include_dirs: Vec<Utf8PathBuf>,
    /// Preprocessor definitions, `NAME` or `NAME=VALUE`.
    #[builder(default)]
    pub defines: Vec<String>,
    /// The PL clock in MHz, which every PLIO runs at.
    #[builder(default)]
    pub pl_freq_mhz: Option<f64>,
    /// PL control writes RTP storage directly, without AIE lock operations.
    #[builder(default)]
    pub pl_control: bool,
}

/// Compile the graph; on success the graph directory holds `Work/` and, for
/// [`AieTarget::Hw`], `libadf.a`.
pub fn compile(runner: &dyn ToolRunner, job: &AieCompileJob) -> Result<()> {
    let mut inv = ToolInvocation::new("aiecompiler")
        .arg(format!("--target={}", job.target.as_str()))
        .arg("--workdir=Work");
    inv = match &job.device {
        AieDevice::Platform(xpfm) => inv.arg(ToolValue::prefixed_path("--platform=", xpfm)),
        AieDevice::Part(part) => inv.arg(format!("--part={part}")),
    };
    for dir in &job.include_dirs {
        inv = inv.arg(ToolValue::prefixed_path("--include=", dir));
        inv.uploads.push(dir.clone());
    }
    for define in &job.defines {
        inv = inv.arg(format!("--Xpreproc=-D{define}"));
    }
    if let Some(mhz) = job.pl_freq_mhz {
        inv = inv.arg(format!("--pl-freq={mhz}"));
    }
    if job.pl_control {
        inv = inv.arg("-use-async-rtp-locks=false");
    }
    inv = inv.arg("graph.cpp");
    run_in(runner, inv, &job.graph_dir, "aiecompiler")
}

/// Run the compiled graph in `x86simulator` (after an [`AieTarget::X86sim`]
/// compile) or `aiesimulator` (after [`AieTarget::Hw`]). PLIO inputs are
/// read from, and outputs written under, the graph directory's `data/`.
pub fn simulate(runner: &dyn ToolRunner, graph_dir: &Utf8Path, target: AieTarget) -> Result<()> {
    let program = match target {
        AieTarget::X86sim => "x86simulator",
        AieTarget::Hw => "aiesimulator",
    };
    let inv = ToolInvocation::new(program).arg("--pkg-dir=Work");
    run_in(runner, inv, graph_dir, program)
}

/// Where the simulator wrote PLIO `name`'s output.
#[must_use]
pub fn simulator_output(graph_dir: &Utf8Path, target: AieTarget, name: &str) -> Utf8PathBuf {
    let dir = match target {
        AieTarget::X86sim => "x86simulator_output",
        AieTarget::Hw => "aiesimulator_output",
    };
    graph_dir.join(dir).join("data").join(format!("{name}.txt"))
}

fn run_in(
    runner: &dyn ToolRunner,
    mut inv: ToolInvocation,
    dir: &Utf8Path,
    program: &str,
) -> Result<()> {
    inv.cwd = Some(dir.to_path_buf());
    inv.uploads.push(dir.to_path_buf());
    inv.downloads.push(dir.to_path_buf());
    let out = runner.run(&inv)?;
    if out.exit_code != 0 {
        return Err(tool_failure(program, out));
    }
    Ok(())
}
