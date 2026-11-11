use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use camino::{Utf8Path, Utf8PathBuf};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use typed_builder::TypedBuilder;

use crate::error::{Result, XilinxError};
use crate::runtime::process::{ToolInvocation, ToolRunner};
use crate::tools::aie::report::{read_report_with_control, AieTile};
use crate::tools::tool_failure;
use crate::ToolValue;

const SOURCE: &str = include_str!("pl_control.cpp");
/// The controller's table of tiles, generated beside `SOURCE`.
const TILES_HEADER: &str = "pl_control_tiles.h";
pub const KERNEL_NAME: &str = "tapa_aie_pl_control";

#[must_use]
pub fn required(platform: &str) -> bool {
    platform.contains("vck5000")
}

/// A tile the PL controller writes, the packet id aiecompiler routes to
/// that tile's packet controller, and the one its answer to a read returns
/// on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ControlTile {
    pub tile: AieTile,
    pub packet: u32,
    pub reply: u32,
}

/// Reads `name` among the reports of the graph compiled in `graph_dir`.
/// aiecompiler writes no DMA or lock report for a graph without buffers: one
/// that is absent reads as empty.
fn read_optional_report<T: DeserializeOwned + Default>(
    graph_dir: &Utf8Path,
    name: &str,
) -> Result<(Utf8PathBuf, T)> {
    let path = graph_dir.join("Work").join("reports").join(name);
    if path.exists() {
        read_report(graph_dir, name)
    } else {
        Ok((path, T::default()))
    }
}

fn read_report<T: DeserializeOwned>(graph_dir: &Utf8Path, name: &str) -> Result<(Utf8PathBuf, T)> {
    let path = graph_dir.join("Work").join("reports").join(name);
    let text = fs_err::read_to_string(&path).map_err(|source| XilinxError::AieReportIo {
        path: path.clone(),
        source,
    })?;
    let report = serde_json::from_str(&text).map_err(|source| XilinxError::AieReportJson {
        path: path.clone(),
        source,
    })?;
    Ok((path, report))
}

/// The tiles of the graph compiled in `graph_dir` that the PL controller
/// writes: each kernel's, reached through its packet controller
/// `control_<kernel>` (see `graph`), ordered by tile.
pub fn control_tiles(graph_dir: &Utf8Path) -> Result<Vec<ControlTile>> {
    let report = read_report_with_control(graph_dir, true)?;
    let (path, switching) =
        read_report::<PacketSwitching>(graph_dir, "packet_switching_report.json")?;
    let parse_error = |detail: String| XilinxError::AieReportParse {
        path: path.clone(),
        detail,
    };
    // Each packet controller's packet ids: [in, out].
    let mut ids = BTreeMap::<String, [Option<u32>; 2]>::new();
    for entry in switching.internal {
        let (end, side) = match (entry.input, entry.output) {
            (Some(end), _) => (end, 0),
            (None, Some(end)) => (end, 1),
            (None, None) => continue,
        };
        let Some(control) = end.control else {
            continue;
        };
        let [packet] = entry.packet_id[..] else {
            return Err(parse_error(format!(
                "`{control}` has packet ids {:?}, not one",
                entry.packet_id
            )));
        };
        ids.entry(control).or_default()[side] = Some(packet);
    }
    let mut tiles = Vec::new();
    for (control, ends) in ids {
        let Some((_, kernel)) = control.rsplit_once(".control_") else {
            continue;
        };
        let placed = report
            .per_kernel
            .get(&format!("tapa_aie_{kernel}"))
            .ok_or_else(|| parse_error(format!("`{control}` controls no placed kernel")))?;
        let [Some(packet), Some(reply)] = ends else {
            return Err(parse_error(format!(
                "`{control}` lacks its input or its return packet id"
            )));
        };
        tiles.push(ControlTile {
            tile: placed.tile,
            packet,
            reply,
        });
    }
    if tiles.len() != report.per_kernel.len() {
        return Err(parse_error(format!(
            "packet controllers reach {} of {} kernels",
            tiles.len(),
            report.per_kernel.len()
        )));
    }
    tiles.sort_by_key(|tile| tile.tile);
    Ok(tiles)
}

/// The parts of aiecompiler's `packet_switching_report.json` read here.
#[derive(Deserialize)]
struct PacketSwitching {
    #[serde(rename = "Internal", default)]
    internal: Vec<PacketPort>,
}

#[derive(Deserialize)]
struct PacketPort {
    #[serde(rename = "Input")]
    input: Option<PacketEnd>,
    #[serde(rename = "Output")]
    output: Option<PacketEnd>,
    #[serde(rename = "PacketID", default)]
    packet_id: Vec<u32>,
}

#[derive(Deserialize)]
struct PacketEnd {
    #[serde(rename = "Control")]
    control: Option<String>,
}

/// One step of a run-forever graph's restart: a register write, or a read,
/// by which an AIE1 tile performs a lock operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResetStep {
    pub tile: AieTile,
    pub address: u32,
    /// The value written, or `None` to read.
    pub value: Option<u32>,
}

/// An AIE1 memory module's DMA channel `c` is controlled at `base + 8 * c`
/// (bit 1 resets it, bit 0 enables it), and starts at the buffer
/// descriptor written 4 bytes above. Each direction has two channels.
const DMA_S2MM_CONTROL: u32 = 0x1_DE00;
const DMA_MM2S_CONTROL: u32 = 0x1_DE10;
const DMA_CHANNELS: u32 = 2;
const DMA_START_QUEUE: u32 = 4;
const DMA_RESET: u32 = 2;
const DMA_ENABLE: u32 = 1;
/// Reading `LOCK_RELEASE_V0 + LOCK_STRIDE * n` releases lock `n` of an AIE1
/// memory module with value 0, as the graph starts; it has 16 locks.
const LOCK_RELEASE_V0: u32 = 0x1_E020;
const LOCK_STRIDE: u32 = 0x80;
const LOCKS: u32 = 16;

/// What restarts the buffers of the run-forever graph compiled in
/// `graph_dir` once its cores are reset, so each kernel starts again at its
/// first buffer: each DMA channel a kernel buffer uses is reset, every lock
/// of a kernel buffer is released with value 0, and each channel restarts at
/// its first buffer descriptor, as the graph's initialization left them.
/// Runtime parameters keep their locks: the host writes them in place.
/// Every step must lie in one of `tiles`, which the PL controller reaches.
pub fn reset_steps(graph_dir: &Utf8Path, tiles: &[ControlTile]) -> Result<Vec<ResetStep>> {
    let controlled = tiles.iter().map(|tile| tile.tile).collect::<BTreeSet<_>>();
    let reach = |tile: AieTile, port: &str| {
        if controlled.contains(&tile) {
            Ok(())
        } else {
            Err(XilinxError::AieGraph(format!(
                "the buffer of `{port}` lies in tile ({}, {}), where no kernel is: TAPA's PL \
                 controller cannot restart the run-forever graph; invoke its tasks joined, or \
                 connect that port to the PL",
                tile.column, tile.row
            )))
        }
    };

    let (path, dma) = read_optional_report::<DmaLocks>(graph_dir, "dma_lock_report.json")?;
    // (tile, channel control address) → the channel's first descriptor.
    let mut channels = BTreeMap::new();
    for (base, ports) in [(DMA_S2MM_CONTROL, &dma.s2mm), (DMA_MM2S_CONTROL, &dma.mm2s)] {
        for port in ports {
            let name = &port.kernel_port.name;
            let parse_error = |detail: &str| XilinxError::AieReportParse {
                path: path.clone(),
                detail: format!("`{name}` {detail}"),
            };
            let first = port
                .buffers
                .first()
                .ok_or_else(|| parse_error("has no buffer"))?;
            let start = *first
                .bds
                .first()
                .ok_or_else(|| parse_error("has no buffer descriptor"))?;
            if port
                .buffers
                .iter()
                .any(|buffer| (buffer.tile(), buffer.channel) != (first.tile(), first.channel))
            {
                return Err(parse_error("spans more than one DMA channel"));
            }
            if first.channel >= DMA_CHANNELS {
                return Err(parse_error("names a DMA channel AIE1 lacks"));
            }
            reach(first.tile(), name)?;
            let channel = (first.tile(), base + 8 * first.channel);
            if channels.insert(channel, start).is_some() {
                return Err(parse_error("shares its DMA channel with another port"));
            }
        }
    }

    let (path, allocation) =
        read_optional_report::<LockAllocation>(graph_dir, "lock_allocation_report.json")?;
    let mut locks = BTreeSet::new();
    for port in allocation.kernels.iter().flat_map(|kernel| &kernel.ports) {
        for lock in &port.locks {
            if lock.name.starts_with("LOCK_RTP_") {
                continue;
            }
            let tile =
                lock_tile(&lock.name, lock.id).ok_or_else(|| XilinxError::AieReportParse {
                    path: path.clone(),
                    detail: format!(
                        "lock `{}` (id {}) of `{}` is not `LOCK_<column>_<row>_<id>_<n>` \
                         with an AIE1 lock id",
                        lock.name, lock.id, port.name
                    ),
                })?;
            reach(tile, &port.name)?;
            locks.insert((tile, lock.id));
        }
    }

    let write = |(tile, address): (AieTile, u32), value| ResetStep {
        tile,
        address,
        value: Some(value),
    };
    let mut steps = Vec::new();
    for &channel in channels.keys() {
        steps.push(write(channel, DMA_RESET));
        steps.push(write(channel, 0));
    }
    steps.extend(locks.into_iter().map(|(tile, id)| ResetStep {
        tile,
        address: LOCK_RELEASE_V0 + LOCK_STRIDE * id,
        value: None,
    }));
    for (&(tile, control), &start) in &channels {
        steps.push(write((tile, control + DMA_START_QUEUE), start));
        steps.push(write((tile, control), DMA_ENABLE));
    }
    Ok(steps)
}

/// The tile of lock `id` named `LOCK_<column>_<row>_<id>_<n>`.
fn lock_tile(name: &str, id: u32) -> Option<AieTile> {
    let fields = name
        .strip_prefix("LOCK_")?
        .split('_')
        .map(|field| field.parse::<u32>().ok())
        .collect::<Option<Vec<_>>>()?;
    match fields[..] {
        [column, row, named, _] if named == id && id < LOCKS => Some(AieTile { column, row }),
        _ => None,
    }
}

/// The parts of aiecompiler's `dma_lock_report.json` read here: the
/// buffers of each kernel port a memory-module DMA channel moves, ping
/// first.
#[derive(Default, Deserialize)]
struct DmaLocks {
    #[serde(rename = "S2MM", default)]
    s2mm: Vec<DmaPort>,
    #[serde(rename = "MM2S", default)]
    mm2s: Vec<DmaPort>,
}

#[derive(Deserialize)]
struct DmaPort {
    #[serde(rename = "KernelPort")]
    kernel_port: KernelPort,
    #[serde(rename = "BufferInfo")]
    buffers: Vec<DmaBuffer>,
}

#[derive(Deserialize)]
struct KernelPort {
    #[serde(rename = "VariableName")]
    name: String,
}

#[derive(Deserialize)]
struct DmaBuffer {
    #[serde(rename = "Column")]
    column: u32,
    #[serde(rename = "Row")]
    row: u32,
    #[serde(rename = "Channel")]
    channel: u32,
    #[serde(rename = "BD")]
    bds: Vec<u32>,
}

impl DmaBuffer {
    const fn tile(&self) -> AieTile {
        AieTile {
            column: self.column,
            row: self.row,
        }
    }
}

/// The parts of aiecompiler's `lock_allocation_report.json` read here.
#[derive(Default, Deserialize)]
struct LockAllocation {
    #[serde(rename = "PortsWithLock", default)]
    kernels: Vec<LockKernel>,
}

#[derive(Deserialize)]
struct LockKernel {
    #[serde(rename = "Ports", default)]
    ports: Vec<LockPort>,
}

#[derive(Deserialize)]
struct LockPort {
    #[serde(rename = "PortName")]
    name: String,
    #[serde(rename = "Locks", default)]
    locks: Vec<Lock>,
}

#[derive(Deserialize)]
struct Lock {
    #[serde(rename = "LockID")]
    id: u32,
    #[serde(rename = "LockName")]
    name: String,
}

/// The key a host program names `tile` by: `(column << 16) | row`.
#[must_use]
pub fn tile_key(tile: AieTile) -> u32 {
    (tile.column << 16) | tile.row
}

/// A reset step's tile word: its key, with bit 31 set for a read.
const READ_FLAG: u32 = 1 << 31;

fn tiles_header(tiles: &[ControlTile], reset: &[ResetStep]) -> String {
    let list = |field: fn(&ControlTile) -> u32| {
        tiles
            .iter()
            .map(|tile| format!("{:#x}", field(tile)))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mut out = String::from("// Generated by TAPA: the tiles the PL controller writes.\n");
    let _ = writeln!(out, "constexpr int kTiles = {};", tiles.len());
    let _ = writeln!(
        out,
        "const ap_uint<32> kTileKeys[kTiles] = {{{}}};",
        list(|tile| tile_key(tile.tile))
    );
    let _ = writeln!(
        out,
        "const ap_uint<32> kTilePackets[kTiles] = {{{}}};",
        list(|tile| tile.packet)
    );
    let _ = writeln!(
        out,
        "const ap_uint<32> kTileReturns[kTiles] = {{{}}};",
        list(|tile| tile.reply)
    );
    let steps = reset
        .iter()
        .map(|step| {
            let read = if step.value.is_none() { READ_FLAG } else { 0 };
            format!(
                "\n    {{{:#x}, {:#x}, {:#x}}}",
                tile_key(step.tile) | read,
                step.address,
                step.value.unwrap_or(0)
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    let _ = writeln!(
        out,
        "constexpr int kResetSteps = {};\n\
         const ap_uint<32> kReset[kResetSteps > 0 ? kResetSteps : 1][3] = {{{steps}}};",
        reset.len()
    );
    out
}

#[derive(Debug, Clone, TypedBuilder)]
pub struct AiePlControlJob {
    pub platform: String,
    pub target_mhz: u32,
    pub work_dir: Utf8PathBuf,
    pub output: Utf8PathBuf,
    /// The tiles the controller writes ([`control_tiles`]).
    pub tiles: Vec<ControlTile>,
    /// What restarts a run-forever graph's buffers ([`reset_steps`]); empty
    /// for a graph that returns.
    pub reset: Vec<ResetStep>,
}

pub fn compile(runner: &dyn ToolRunner, job: &AiePlControlJob) -> Result<()> {
    // v++ runs in the work directory, where a relative path would land.
    let job = &AiePlControlJob {
        work_dir: crate::util::absolutize(&job.work_dir),
        output: crate::util::absolutize(&job.output),
        ..job.clone()
    };
    fs_err::create_dir_all(&job.work_dir)?;
    if let Some(parent) = job.output.parent() {
        fs_err::create_dir_all(parent)?;
    }
    if job.tiles.is_empty() {
        return Err(XilinxError::AieGraph(
            "the AI Engine PL controller has no tile to write".into(),
        ));
    }
    let source = job.work_dir.join("pl_control.cpp");
    fs_err::write(&source, SOURCE)?;
    let header = job.work_dir.join(TILES_HEADER);
    fs_err::write(&header, tiles_header(&job.tiles, &job.reset))?;
    let mut inv = ToolInvocation::new("v++")
        .arg("--compile")
        .arg("--target")
        .arg("hw")
        .arg("--platform")
        .arg(job.platform.clone())
        .arg("--clock.defaultFreqHz")
        .arg((u64::from(job.target_mhz) * 1_000_000).to_string())
        .arg("--kernel")
        .arg(KERNEL_NAME)
        .arg(ToolValue::path(&source))
        .arg("--output")
        .arg(ToolValue::path(&job.output));
    inv.cwd = Some(job.work_dir.clone());
    inv.env
        .insert("HOME".into(), ToolValue::path(&job.work_dir));
    inv.uploads.push(source);
    inv.uploads.push(header);
    inv.downloads.push(job.output.clone());
    let out = runner.run(&inv)?;
    if out.exit_code != 0 {
        return Err(tool_failure("v++", out));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MockToolRunner, ToolOutput};

    #[test]
    fn only_vck5000_requires_pl_control() {
        assert!(required("xilinx_vck5000_gen4x8_qdma_2_202220_1"));
        assert!(!required("xilinx_vck190_base_202520_1"));
        assert!(!required("xcvc1902-vsva2197-2MP-e-S"));
    }

    #[test]
    fn compile_builds_the_control_xo_at_the_design_clock() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = crate::util::utf8(dir.path());
        let job = AiePlControlJob::builder()
            .platform("xilinx_vck5000_gen4x8_qdma_2_202220_1".into())
            .target_mhz(300)
            .work_dir(root.join("work"))
            .output(root.join("vadd.aie_control.xo"))
            .tiles(vec![ControlTile {
                tile: AieTile { column: 25, row: 0 },
                packet: 3,
                reply: 1,
            }])
            .reset(vec![
                ResetStep {
                    tile: AieTile { column: 25, row: 0 },
                    address: 0x1_DE00,
                    value: Some(2),
                },
                ResetStep {
                    tile: AieTile { column: 25, row: 0 },
                    address: 0x1_E0A0,
                    value: None,
                },
            ])
            .build();
        let runner = MockToolRunner::new();
        runner.push_ok("v++", ToolOutput::default());
        runner.attach_download(job.output.clone(), b"controller".to_vec());
        compile(&runner, &job).expect("compile");
        let calls = runner.calls();
        assert_eq!(calls.len(), 1);
        let args = calls[0].local_args();
        for expected in [
            "--compile",
            "--clock.defaultFreqHz",
            "300000000",
            "--kernel",
            KERNEL_NAME,
        ] {
            assert!(args.iter().any(|arg| arg == expected), "missing {expected}");
        }
        assert_eq!(fs_err::read(job.output).expect("output"), b"controller");
        let header = root.join("work").join(TILES_HEADER);
        assert!(calls[0].uploads.contains(&header));
        assert_eq!(
            fs_err::read_to_string(&header).expect("tile table"),
            "// Generated by TAPA: the tiles the PL controller writes.\n\
             constexpr int kTiles = 1;\n\
             const ap_uint<32> kTileKeys[kTiles] = {0x190000};\n\
             const ap_uint<32> kTilePackets[kTiles] = {0x3};\n\
             const ap_uint<32> kTileReturns[kTiles] = {0x1};\n\
             constexpr int kResetSteps = 2;\n\
             const ap_uint<32> kReset[kResetSteps > 0 ? kResetSteps : 1][3] = {\n    \
             {0x190000, 0x1de00, 0x2},\n    \
             {0x80190000, 0x1e0a0, 0x0}};\n"
        );
    }

    #[test]
    fn control_tiles_pair_each_kernel_tile_with_its_packet_ids() {
        use crate::tools::aie::report::tests::{graph_dir, linker_map, write_reports, COMPILER};
        let temp = write_reports(
            COMPILER,
            &[("25_1", &linker_map(1)), ("23_0", &linker_map(1))],
        );
        let switching = temp
            .path()
            .join("Work/reports/packet_switching_report.json");
        let report = |internal: &str| format!(r#"{{"Inputs": [], "Internal": [{internal}]}}"#);
        let split = r#"{"Input": {"Split": "tapa_aie.tapa_ctrl_split"}, "PacketID": [0, 1]},
            {"Output": {"Split": "tapa_aie.tapa_ctrl_split"}, "PacketID": [0]}"#;
        fs_err::write(
            &switching,
            report(&format!(
                r#"{split},
                {{"Input": {{"Control": "tapa_aie.control_Z_0"}}, "PacketID": [0]}},
                {{"Output": {{"Control": "tapa_aie.control_Z_0"}}, "PacketID": [1]}},
                {{"Input": {{"Control": "tapa_aie.control_Add_0"}}, "PacketID": [1]}},
                {{"Output": {{"Control": "tapa_aie.control_Add_0"}}, "PacketID": [0]}}"#
            )),
        )
        .expect("packet switching report");
        assert_eq!(
            control_tiles(graph_dir(&temp)).expect("control tiles"),
            [
                ControlTile {
                    tile: AieTile { column: 23, row: 0 },
                    packet: 1,
                    reply: 0,
                },
                ControlTile {
                    tile: AieTile { column: 25, row: 1 },
                    packet: 0,
                    reply: 1,
                },
            ]
        );
        let z = r#"{"Input": {"Control": "tapa_aie.control_Z_0"}, "PacketID": [0]}"#;
        fs_err::write(&switching, report(z)).expect("packet switching report");
        let error = control_tiles(graph_dir(&temp)).expect_err("a controller without return");
        assert!(error.to_string().contains("return packet id"), "{error}");
        let z =
            format!(r#"{z}, {{"Output": {{"Control": "tapa_aie.control_Z_0"}}, "PacketID": [1]}}"#);
        fs_err::write(&switching, report(&z)).expect("packet switching report");
        let error = control_tiles(graph_dir(&temp)).expect_err("a kernel without control");
        assert!(
            error
                .to_string()
                .contains("packet controllers reach 1 of 2 kernels"),
            "{error}"
        );
    }

    /// Two kernels of a run-forever graph: `Add_0` in tile (23, 0) reads
    /// frames on S2MM channel 0, `Z_0` in tile (25, 1) writes them on MM2S
    /// channel 1, and `Add_0`'s runtime parameter keeps its locks.
    #[test]
    fn reset_steps_restart_each_buffer_channel_and_lock_in_the_kernels_tiles() {
        let temp = tempfile::tempdir().expect("temporary graph directory");
        let reports = temp.path().join("Work/reports");
        fs_err::create_dir_all(&reports).expect("report directory");
        let buffers = |column, row, channel, locks: [u32; 2], bds: [u32; 2]| {
            format!(
                r#"[{{"Column": {column}, "Row": {row}, "Channel": {channel}, "LockID": {},
                     "ControlBuffer": false, "AB_Mode": false, "BD": [{}]}},
                   {{"Column": {column}, "Row": {row}, "Channel": {channel}, "LockID": {},
                     "ControlBuffer": false, "AB_Mode": false, "BD": [{}]}}]"#,
                locks[0], bds[0], locks[1], bds[1]
            )
        };
        let port = |name: &str, buffers: String| {
            format!(
                r#"{{"KernelPort": {{"QualifiedGraphName": "tapa_aie_forever",
                    "VariableName": "tapa_aie_forever.{name}"}}, "BufferInfo": {buffers}}}"#
            )
        };
        fs_err::write(
            reports.join("dma_lock_report.json"),
            format!(
                r#"{{"S2MM": [{}], "MM2S": [{}]}}"#,
                port("k_Add_0.in[0]", buffers(23, 0, 0, [0, 1], [0, 1])),
                port("k_Z_0.out[0]", buffers(25, 1, 1, [2, 3], [2, 3])),
            ),
        )
        .expect("DMA report");
        fs_err::write(
            reports.join("lock_allocation_report.json"),
            r#"{"PortsWithLock": [
              {"VariableName": "tapa_aie_forever.k_Add_0", "Ports": [
                {"Port": "i0_pi0", "PortName": "tapa_aie_forever.k_Add_0.in[0]", "Locks": [
                  {"BufferName": "buf0", "LockID": 0, "LockName": "LOCK_23_0_0_0"},
                  {"BufferName": "buf0d", "LockID": 1, "LockName": "LOCK_23_0_1_8160"}]},
                {"Port": "i0_pi1", "PortName": "tapa_aie_forever.k_Add_0.in[1]", "Locks": [
                  {"BufferName": "rtp2", "LockID": 4, "LockName": "LOCK_RTP_23_0_4_0"}]}]},
              {"VariableName": "tapa_aie_forever.k_Z_0", "Ports": [
                {"Port": "i1_po0", "PortName": "tapa_aie_forever.k_Z_0.out[0]", "Locks": [
                  {"BufferName": "buf1", "LockID": 2, "LockName": "LOCK_25_1_2_0"},
                  {"BufferName": "buf1d", "LockID": 3, "LockName": "LOCK_25_1_3_0"}]}]}]}"#,
        )
        .expect("lock report");
        let (add, z) = (
            AieTile { column: 23, row: 0 },
            AieTile { column: 25, row: 1 },
        );
        let control = |tile| ControlTile {
            tile,
            packet: 0,
            reply: 0,
        };
        let steps = reset_steps(
            crate::util::utf8(temp.path()).as_path(),
            &[control(add), control(z)],
        )
        .expect("reset steps");
        let step = |tile, address, value| ResetStep {
            tile,
            address,
            value,
        };
        assert_eq!(
            steps,
            [
                step(add, 0x1_DE00, Some(2)),
                step(add, 0x1_DE00, Some(0)),
                step(z, 0x1_DE18, Some(2)),
                step(z, 0x1_DE18, Some(0)),
                step(add, 0x1_E020, None),
                step(add, 0x1_E0A0, None),
                step(z, 0x1_E120, None),
                step(z, 0x1_E1A0, None),
                step(add, 0x1_DE04, Some(0)),
                step(add, 0x1_DE00, Some(1)),
                step(z, 0x1_DE1C, Some(2)),
                step(z, 0x1_DE18, Some(1)),
            ]
        );
        let error = reset_steps(crate::util::utf8(temp.path()).as_path(), &[control(z)])
            .expect_err("a buffer outside the controlled tiles");
        assert!(
            error
                .to_string()
                .contains("the buffer of `tapa_aie_forever.k_Add_0.in[0]` lies in tile (23, 0)"),
            "{error}"
        );

        // A graph without buffers: aiecompiler writes neither report.
        let streams = tempfile::tempdir().expect("graph directory without reports");
        let steps = reset_steps(crate::util::utf8(streams.path()).as_path(), &[control(add)])
            .expect("no buffers");
        assert!(steps.is_empty(), "{steps:?}");
    }

    #[test]
    fn compile_hands_v_plus_plus_an_absolute_output() {
        let dir = tempfile::tempdir().expect("tempdir");
        let job = AiePlControlJob::builder()
            .platform("xilinx_vck5000_gen4x8_qdma_2_202220_1".into())
            .target_mhz(300)
            .work_dir(crate::util::utf8(dir.path()))
            .output("vadd.aie_control.xo".into())
            .tiles(vec![ControlTile {
                tile: AieTile { column: 25, row: 0 },
                packet: 0,
                reply: 0,
            }])
            .reset(Vec::new())
            .build();
        let runner = MockToolRunner::new();
        runner.push_ok("v++", ToolOutput::default());
        compile(&runner, &job).expect("compile");
        let args = runner.calls()[0].local_args();
        let output = args.iter().skip_while(|arg| *arg != "--output").nth(1);
        let output = output.expect("--output");
        assert!(camino::Utf8Path::new(output).is_absolute(), "{output}");
    }
}
