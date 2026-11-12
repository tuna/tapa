//! AI Engine orchestration: render the ADF graph of an AI Engine partition
//! and compile it with `aiecompiler`. The runtime runs a graph built for a
//! simulator (`frt`'s AI Engine simulation backend).

pub mod graph;
pub mod link;
pub mod pl_control;
pub mod report;

use camino::{Utf8Path, Utf8PathBuf};
use typed_builder::TypedBuilder;

use crate::error::{Result, XilinxError};
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
    run_in(runner, inv, &job.graph_dir, "aiecompiler").map_err(|error| match error {
        XilinxError::ToolFailure {
            program,
            code,
            stderr,
        } if matches!(job.device, AieDevice::Part(_))
            && stderr.contains("Unknown command line argument '--part") =>
        {
            XilinxError::ToolFailure {
                program,
                code,
                stderr: format!(
                    "{stderr}\nthis aiecompiler has no `--part` (Vitis 2022.2 has none); \
                     name the device with `--platform` instead"
                ),
            }
        }
        other => other,
    })
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::process::{MockToolRunner, ToolOutput};

    /// Vitis 2022.2's aiecompiler has no `--part`; its refusal names the
    /// flag that works instead.
    #[test]
    fn a_compiler_without_part_points_at_platform() {
        let dir = tempfile::tempdir().expect("tempdir");
        let job = AieCompileJob::builder()
            .graph_dir(crate::util::utf8(dir.path()))
            .target(AieTarget::X86sim)
            .device(AieDevice::Part("xcvc1902-vsva2197-2MP-e-S".into()))
            .build();
        let runner = MockToolRunner::new();
        runner.push_ok(
            "aiecompiler",
            ToolOutput {
                exit_code: 1,
                stdout: "ERROR: [aiecompiler 77-4469] driver aiecompiler: Unknown command line \
                         argument '--part=xcvc1902-vsva2197-2MP-e-S'."
                    .into(),
                stderr: String::new(),
            },
        );
        let error = compile(&runner, &job).expect_err("2022.2 rejects --part");
        assert!(error.to_string().contains("`--platform`"), "{error}");
    }
}
