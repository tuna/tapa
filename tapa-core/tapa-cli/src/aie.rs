//! Shared AI Engine build inputs for `tapa synth` and `tapa compile
//! --aie-sim`: where each kernel's task is defined in the rewritten tree,
//! the kernel include paths and definitions, the device `aiecompiler`
//! targets, and the streams the host moves to and from the graph.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use camino::{Utf8Path, Utf8PathBuf};
use serde::Serialize;
use tapa_ir::{AiePartition, AieStreamKind, Graph};
use tapa_xilinx::tools::aie::graph::{render_graph, GraphOptions, KernelSource, SimMain};
use tapa_xilinx::tools::aie::{AieCompileJob, AieDevice, AieTarget};

use crate::error::{CliError, Result};
use crate::tapacc::discover::find_resource;

/// The device an AI Engine graph compiles for: a platform's `.xpfm`, or a
/// part number when no platform is known.
pub fn device(platform: Option<&str>, part: Option<&str>) -> Result<AieDevice> {
    if let Some(part) = part {
        return Ok(AieDevice::Part(part.to_string()));
    }
    if let Some(platform) = platform {
        let dir = crate::steps::synth::resolve_platform_dir(platform).ok_or_else(|| {
            CliError::InvalidArg(format!("cannot find the platform `{platform}`"))
        })?;
        let xpfm = fs::read_dir(&dir)?
            .filter_map(std::result::Result::ok)
            .map(|entry| crate::util::utf8(entry.path()))
            .find(|path| path.extension() == Some("xpfm"))
            .ok_or_else(|| {
                CliError::InvalidArg(format!("platform `{platform}` has no `.xpfm` in {dir}"))
            })?;
        return Ok(AieDevice::Platform(xpfm));
    }
    Err(CliError::InvalidArg(
        "the AI Engine compiler needs a device; pass `--platform` or `--part-num`, \
         or run `tapa synth` first"
            .to_string(),
    ))
}

/// Where each AI Engine task is defined: the first translation unit of its
/// manifest whose rewritten text carries its guard, else the first one (a
/// task defined in a shared header is guarded in that header).
fn kernel_sources(
    tree: &Utf8Path,
    graph: &Graph,
    partition: &AiePartition,
) -> Result<BTreeMap<String, KernelSource>> {
    let mut sources = BTreeMap::new();
    for kernel in &partition.kernels {
        let task = &graph.tasks[&kernel.task];
        let mut chosen = task.srcs.first().cloned();
        for src in &task.srcs {
            let text = fs::read_to_string(tree.join(src))?;
            if task
                .defines
                .iter()
                .any(|define| text.contains(define.as_str()))
            {
                chosen = Some(src.clone());
                break;
            }
        }
        let tu = chosen.ok_or_else(|| {
            CliError::Codegen(format!(
                "task `{}` has an empty `srcs` manifest",
                kernel.task
            ))
        })?;
        sources.insert(
            kernel.task.clone(),
            KernelSource {
                tu,
                defines: task.defines.clone(),
            },
        );
    }
    Ok(sources)
}

/// Each port of the kernel's top with the index of its first argument,
/// as `kernel.xml` numbers them: the PL kernel's, where a PLIO's AXIS port
/// comes after the top's own arguments, or a PL-less top's own.
pub fn kernel_args(graph: &Graph, partition: &AiePartition) -> Result<Vec<(u32, tapa_ir::Port)>> {
    let pl;
    let top = if partition.pl_less {
        &graph.tasks[&graph.top]
    } else {
        pl = partition.pl_projection(graph)?;
        &pl.tasks[&pl.top]
    };
    let mut args = Vec::new();
    let mut next = 0u32;
    for port in &top.ports {
        let names = port.kernel_arg_names(tapa_ir::Target::XilinxVitis);
        args.push((next, port.clone()));
        next += u32::try_from(names.len()).unwrap_or(u32::MAX);
    }
    Ok(args)
}

/// Each top scalar argument's index among the PL kernel's arguments: the
/// runtime finds a runtime parameter's value by that index.
fn rtp_args(graph: &Graph, partition: &AiePartition) -> Result<BTreeMap<String, u32>> {
    Ok(kernel_args(graph, partition)?
        .into_iter()
        .filter(|(_, port)| port.cat.is_scalar())
        .map(|(index, port)| (port.name, index))
        .collect())
}

/// One graph build: where, for which target and device, at which PL clock,
/// and with which simulator entry point.
pub struct GraphBuild {
    pub dir: Utf8PathBuf,
    pub target: AieTarget,
    pub device: AieDevice,
    pub pl_freq_mhz: Option<f64>,
    pub pl_control: bool,
    pub sim: Option<SimMain>,
}

/// Writes the graph sources of `partition` into the build directory (wiped
/// first) and returns the compile job for it.
pub fn prepare(
    work_dir: &Path,
    graph: &Graph,
    partition: &AiePartition,
    build: GraphBuild,
) -> Result<AieCompileJob> {
    let GraphBuild {
        dir,
        target,
        device,
        pl_freq_mhz,
        pl_control,
        sim,
    } = build;
    let dir = dir.as_path();
    let tree = crate::util::utf8(work_dir.join(crate::tapacc::REWRITTEN_DIR));
    let options = GraphOptions {
        sources: kernel_sources(&tree, graph, partition)?,
        rtp_args: rtp_args(graph, partition)?,
        plio_mhz: pl_freq_mhz,
        pl_control,
        sim,
    };
    let files = render_graph(partition, &options)?;
    if dir.exists() {
        fs::remove_dir_all(dir)?;
    }
    for (name, text) in files {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, text)?;
    }
    fs::create_dir_all(dir.join("data"))?;

    // Kernels compile the rewritten tree with the AI Engine target of
    // `tapa.h`; the user's include paths and definitions carry over.
    let mut include_dirs = vec![tree.clone()];
    let mut seen = std::collections::BTreeSet::new();
    for task in graph.tasks.values() {
        for include in &task.include_dirs {
            if !include.is_empty() && seen.insert(include.clone()) {
                include_dirs.push(tree.join(include));
            }
        }
    }
    let mut defines = vec![
        "TAPA_TARGET_DEVICE_".to_string(),
        "TAPA_TARGET_AIE_".to_string(),
    ];
    let mut flags = graph.cflags.iter();
    while let Some(flag) = flags.next() {
        for (prefix, out) in [("-I", true), ("-isystem", true), ("-D", false)] {
            let Some(rest) = flag.strip_prefix(prefix) else {
                continue;
            };
            let value = if rest.is_empty() {
                flags.next().cloned().unwrap_or_default()
            } else {
                rest.to_string()
            };
            if out {
                include_dirs.push(crate::util::utf8(std::path::absolute(&value)?));
            } else {
                defines.push(value);
            }
            break;
        }
    }
    if let Ok(lib) = find_resource("tapa-lib-include") {
        include_dirs.push(crate::util::utf8(lib));
    }
    Ok(AieCompileJob::builder()
        .graph_dir(dir.to_path_buf())
        .target(target)
        .device(device)
        .include_dirs(include_dirs)
        .defines(defines)
        .pl_freq_mhz(pl_freq_mhz)
        .pl_control(pl_control)
        .build())
}

/// The PL clock in MHz a synthesized design runs its PLIOs at.
pub fn pl_freq_mhz(state: &tapa_ir::WorkState) -> Option<f64> {
    state
        .flow
        .clock_period
        .map(|period| 1000.0 / period.nanoseconds())
}

/// `<work_dir>/aie/<name>`.
pub fn dir(work_dir: &Path, name: &str) -> Utf8PathBuf {
    crate::util::utf8(work_dir.join("aie").join(name))
}

/// The design `tapa synth` builds as RTL: the PL projection when some task
/// is on the AI Engine, else the graph itself.
pub fn pl_graph(graph: &Graph) -> Result<Graph> {
    match AiePartition::derive(graph)? {
        Some(partition) => Ok(partition.pl_projection(graph)?),
        None => Ok(graph.clone()),
    }
}

/// Warns about each PL↔AI Engine stream whose elements are narrower than
/// one 128-bit PLIO beat. The PL moves one element per cycle, and only a
/// 128-bit PLIO keeps up with the AI Engine stream port at the other end.
pub fn warn_narrow_boundaries(partition: &AiePartition) {
    // A write acknowledgement is one word, which no element type widens.
    let responses: std::collections::BTreeSet<usize> = partition
        .kernels
        .iter()
        .flat_map(|kernel| &kernel.ports)
        .filter_map(|port| {
            let tapa_ir::AiePortKind::Memory(memory) = &port.kind else {
                return None;
            };
            memory.writes.as_ref().and_then(|writes| writes.responses)
        })
        .collect();
    for (index, stream) in partition.streams.iter().enumerate() {
        if responses.contains(&index) {
            continue;
        }
        let bits = stream.words * 32;
        if matches!(stream.kind, tapa_ir::AieStreamKind::Plio { .. }) && bits < 128 {
            log::warn!(
                "AI Engine boundary stream `{}` moves {bits} bits per PL cycle, {}% of a \
                 128-bit PLIO; carry tapa::vec_t<T, 4> elements of 32-bit T across the \
                 boundary for the full rate",
                stream.fifo,
                bits * 100 / 128
            );
        }
    }
}

/// Logs every stream that crosses between the PL and the AI Engine: each
/// one costs a PLIO channel of the array's interface.
pub fn report_crossings(partition: &AiePartition) {
    for (index, stream) in partition.streams.iter().enumerate() {
        let tapa_ir::AieStreamKind::Plio { input, width } = stream.kind else {
            continue;
        };
        let kernels: Vec<&str> = partition
            .kernels
            .iter()
            .filter(|kernel| {
                kernel
                    .ports
                    .iter()
                    .any(|port| port.kind.streams().contains(&index))
            })
            .map(|kernel| kernel.name.as_str())
            .collect();
        let (direction, verb) = if input {
            ("PL -> AI Engine", "feeds")
        } else {
            ("AI Engine -> PL", "drains")
        };
        log::info!(
            "{direction}: stream `{}` {verb} kernel {} through a {width}-bit PLIO",
            stream.fifo,
            kernels.join(", ")
        );
    }
}

/// The `v++ --link` connectivity of the PLIOs: each PL kernel AXIS port is
/// wired to the PLIO of the same name. The compute unit is named after the
/// top task, matching the `--connectivity.nk` naming `tapa link` uses.
pub fn link_config(top: &str, partition: &AiePartition, pl_control: bool) -> String {
    let mut out = String::from("[connectivity]\n");
    if pl_control {
        out.push_str(
            "nk=tapa_aie_pl_control:1:controller_1\n\
             stream_connect=controller_1.output:ai_engine_0.tapa_ctrl\n",
        );
    }
    for stream in &partition.streams {
        if let tapa_ir::AieStreamKind::Plio { input, .. } = stream.kind {
            let pl = format!("{top}.{}", stream.name);
            let aie = format!("ai_engine_0.{}", stream.name);
            let (src, dst) = if input { (pl, aie) } else { (aie, pl) };
            let _ = writeln!(out, "stream_connect={src}:{dst}");
        }
    }
    out
}

/// A stream the host moves to or from the graph: a PLIO, which the host
/// binds as a stream argument after the top's own, or a GMIO of a top on
/// the AI Engine, one of the top's own stream arguments. The runtime reads
/// these fields from a simulation package's manifest and, for a GMIO, from
/// the xclbin.
#[derive(Debug, Clone, Serialize)]
pub struct HostStream {
    /// The kernel argument index the host binds it at.
    pub index: u32,
    /// The PLIO or GMIO logical name, which is also its file name under
    /// `data/` in a simulation.
    pub name: String,
    /// The host side (the PL, or the host itself for a GMIO) writes it and
    /// the AI Engine reads it.
    pub input: bool,
    /// 32-bit words per stream element.
    pub element_words: u32,
    /// 32-bit words per beat, one line of its simulation file: a PLIO
    /// beat, or a GMIO frame.
    pub beat_words: u32,
    /// 32-bit words in a whole transfer, a whole beat and a whole frame:
    /// a final partial one is zero-padded on input.
    pub transfer_words: u32,
}

/// The PLIOs and the GMIOs of `partition`, by the kernel argument indices
/// in `args` (see [`kernel_args`]).
pub fn host_streams(
    partition: &AiePartition,
    args: &[(u32, tapa_ir::Port)],
) -> Result<(Vec<HostStream>, Vec<HostStream>)> {
    let index_of = |name: &str| {
        args.iter()
            .find(|(_, port)| port.name == name)
            .map(|(index, _)| *index)
            .ok_or_else(|| CliError::InvalidArg(format!("the kernel has no argument `{name}`")))
    };
    let (mut plios, mut gmios) = (Vec::new(), Vec::new());
    for stream in &partition.streams {
        let frame_words = stream.frame.map_or(1, |frame| frame * stream.words);
        let (list, input, beat_words) = match stream.kind {
            AieStreamKind::Plio { input, width } => (&mut plios, input, width / 32),
            AieStreamKind::Gmio { input } => (&mut gmios, input, frame_words),
            AieStreamKind::Net { .. } => continue,
        };
        list.push(HostStream {
            index: index_of(&stream.fifo)?,
            name: stream.name.clone(),
            input,
            element_words: stream.words,
            beat_words,
            transfer_words: lcm(beat_words, frame_words),
        });
    }
    Ok((plios, gmios))
}

/// The least common multiple, or 0 when either is 0.
fn lcm(first: u32, second: u32) -> u32 {
    let (mut gcd, mut rest) = (first, second);
    while rest != 0 {
        (gcd, rest) = (rest, gcd % rest);
    }
    first.checked_div(gcd).map_or(0, |part| part * second)
}

/// The package of a top on the AI Engine (`tapa pack` writes a `.zip`, as
/// it has no PL kernel to make an `.xo` of) holds the work state, the graph
/// under this name, and the argument list under [`PACKAGE_ARGUMENTS`].
pub const PACKAGE_LIBADF: &str = "libadf.a";
pub const PACKAGE_ARGUMENTS: &str = "arguments.json";

/// The argument list of a top on the AI Engine, which `tapa link` adds to
/// the xclbin as `USER_METADATA` for the runtime: the top's name and its
/// GMIOs. Its scalars are runtime parameters, which `AIE_METADATA` names
/// after their argument indices.
#[derive(Debug, Serialize)]
pub struct Arguments {
    pub top: String,
    pub gmios: Vec<HostStream>,
}

/// [`Arguments`] of a top on the AI Engine, as JSON.
pub fn arguments_json(graph: &Graph, partition: &AiePartition) -> Result<String> {
    let (_, gmios) = host_streams(partition, &kernel_args(graph, partition)?)?;
    let arguments = Arguments {
        top: graph.top.clone(),
        gmios,
    };
    serde_json::to_string_pretty(&arguments)
        .map_err(|e| CliError::Report(format!("cannot serialize the argument list: {e}")))
}

/// The AI Engine sidecars of a packaged `.xo`: `<name>.libadf.a` and
/// `<name>.aie.cfg` next to `<name>.xo`.
pub fn sidecars(xo: &Path) -> (std::path::PathBuf, std::path::PathBuf) {
    (xo.with_extension("libadf.a"), xo.with_extension("aie.cfg"))
}

/// The PL control-kernel sidecar used on VCK5000.
pub fn pl_control_sidecar(xo: &Path) -> std::path::PathBuf {
    xo.with_extension("aie_control.xo")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_explicit_part_overrides_the_platform_for_aie() {
        let device = device(Some("/platform/does/not/exist"), Some("xcvc1902-test"))
            .expect("explicit part wins without reading the platform");
        assert!(matches!(device, AieDevice::Part(part) if part == "xcvc1902-test"));
    }
}
