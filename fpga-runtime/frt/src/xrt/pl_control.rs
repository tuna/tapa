use std::collections::{BTreeSet, HashMap};
use std::ffi::CString;

use super::api::{Api, Handle, RUN_COMPLETED, SYNC_TO_DEVICE};
use super::device::{check, non_null};
use super::metadata::{AieGraph, AieTile};
use crate::error::{FrtError, Result};

const KERNEL_NAME: &str = "tapa_aie_pl_control:{controller_1}";
const CORE_CONTROL: u32 = 0x32000;
const CORE_ENABLE: u32 = 1;
const CORE_DISABLE: u32 = 0;
const MAX_PROGRAM_WORDS: usize = 1024;

pub(super) struct PlControl {
    api: &'static Api,
    device: Handle,
    kernel: Handle,
    run: Handle,
}

// SAFETY: FRT accesses each PL controller from one thread at a time; its XRT
// handles are reference-counted objects inside XRT.
unsafe impl Send for PlControl {}

impl PlControl {
    pub(super) fn open(api: &'static Api, device: Handle, uuid: &[u8; 16]) -> Result<Self> {
        let name = CString::new(KERNEL_NAME).expect("PL-control kernel name has no NUL");
        // SAFETY: live device and xclbin UUID, and a valid C string.
        let kernel = unsafe { (api.kernel_open)(device, uuid.as_ptr(), name.as_ptr()) };
        let mut this = Self {
            api,
            device,
            kernel: non_null(kernel, "open the AIE PL control kernel")?,
            run: std::ptr::null_mut(),
        };
        // SAFETY: live PL-kernel handle.
        let run = unsafe { (api.run_open)(this.kernel) };
        this.run = non_null(run, "create an AIE PL control run")?;
        Ok(this)
    }

    pub(super) fn execute(&self, commands: &[u8]) -> Result<()> {
        if commands.is_empty() {
            return Err(FrtError::Xrt("empty AIE PL control command".into()));
        }
        // SAFETY: live PL-kernel handle and argument one is its command BO.
        let group = unsafe { (self.api.kernel_arg_group_id)(self.kernel, 1) };
        let group = u32::try_from(group)
            .map_err(|e| FrtError::Xrt(format!("PL command buffer has no memory bank: {e}")))?;
        // SAFETY: live device handle.
        let bo = unsafe { (self.api.bo_alloc)(self.device, commands.len(), 0, group) };
        let bo = non_null(bo, "allocate the AIE PL command buffer")?;
        let result = (|| {
            // SAFETY: `commands` holds `commands.len()` readable bytes.
            let rc =
                unsafe { (self.api.bo_write)(bo, commands.as_ptr().cast(), commands.len(), 0) };
            check(rc, "write the AIE PL command buffer")?;
            // SAFETY: live BO of `commands.len()` bytes.
            let rc = unsafe { (self.api.bo_sync)(bo, SYNC_TO_DEVICE, commands.len(), 0) };
            check(rc, "sync the AIE PL command buffer")?;
            // SAFETY: live run and BO handles; argument one is the command BO.
            let rc = unsafe { (self.api.run_set_arg)(self.run, 1, bo) };
            check(rc, "bind the AIE PL command buffer")?;
            // SAFETY: live run handle with its argument set.
            let rc = unsafe { (self.api.run_start)(self.run) };
            check(rc, "start the AIE PL control kernel")?;
            // SAFETY: live run handle.
            let state = unsafe { (self.api.run_wait)(self.run) };
            if state != RUN_COMPLETED {
                return Err(FrtError::Xrt(format!(
                    "the AIE PL control run ended in state {state}, not completed"
                )));
            }
            Ok(())
        })();
        // SAFETY: the BO is no longer used by the completed run.
        unsafe { (self.api.bo_free)(bo) };
        result
    }
}

impl Drop for PlControl {
    fn drop(&mut self) {
        if !self.run.is_null() {
            // SAFETY: live run handle, closed once here.
            unsafe { (self.api.run_close)(self.run) };
        }
        if !self.kernel.is_null() {
            // SAFETY: live PL-kernel handle with no open run.
            unsafe { (self.api.kernel_close)(self.kernel) };
        }
    }
}

/// One register write the controller sends to one tile.
type Write = (AieTile, u32, u32);

/// The program the controller runs: the word count, then per write the
/// tile's key (`(column << 16) | row`, as `tapa-xilinx`'s
/// `pl_control::tile_key`), the tile-local address and the value.
fn encode(writes: &[Write]) -> Result<Vec<u8>> {
    let words = writes
        .len()
        .checked_mul(3)
        .filter(|words| *words <= MAX_PROGRAM_WORDS)
        .ok_or_else(|| FrtError::Xrt("AIE PL control program is too large".into()))?;
    let mut out = Vec::with_capacity((words + 1) * 4);
    out.extend_from_slice(&(words as u32).to_le_bytes());
    for (tile, address, value) in writes {
        for word in [(tile.column << 16) | tile.row, *address, *value] {
            out.extend_from_slice(&word.to_le_bytes());
        }
    }
    Ok(out)
}

/// The controller reaches the kernels' tiles only, each through its own
/// packet controller; every runtime parameter and iteration count must lie
/// there, as the graph keeps them (each kernel's stack, and the iteration
/// count beside it, is pinned to the kernel's tile).
fn validate(graph: &AieGraph) -> Result<()> {
    let cores = graph.cores.iter().copied().collect::<BTreeSet<_>>();
    if cores.is_empty() {
        return Err(FrtError::Xrt("AIE graph has no kernel tile".into()));
    }
    let unreachable = |what: &str, tile: AieTile| {
        FrtError::Xrt(format!(
            "AIE PL control reaches only the kernels' tiles, not {what} in tile ({}, {})",
            tile.column, tile.row
        ))
    };
    for rtp in &graph.rtps {
        if rtp.ping_address != rtp.pong_address {
            return Err(FrtError::Xrt(format!(
                "AIE PL control requires single-buffered RTP `{}`",
                rtp.alias
            )));
        }
        if !cores.contains(&rtp.tile) {
            return Err(unreachable(&format!("RTP `{}`", rtp.alias), rtp.tile));
        }
    }
    for (tile, _) in &graph.iterations {
        if !cores.contains(tile) {
            return Err(unreachable("an iteration count", *tile));
        }
    }
    Ok(())
}

fn core_control(graph: &AieGraph, value: u32) -> impl Iterator<Item = Write> + '_ {
    graph
        .cores
        .iter()
        .map(move |tile| (*tile, CORE_CONTROL, value))
}

pub(super) fn start(
    graph: &AieGraph,
    values: &HashMap<u32, Vec<u8>>,
    reset: bool,
) -> Result<Vec<u8>> {
    validate(graph)?;
    let mut writes = Vec::new();
    if reset {
        writes.extend(core_control(graph, CORE_DISABLE));
    }
    for rtp in &graph.rtps {
        let value = rtp.scalar_value(values)?;
        for (index, chunk) in value.chunks(4).enumerate() {
            let mut word = [0u8; 4];
            word[..chunk.len()].copy_from_slice(chunk);
            let offset = u32::try_from(index * 4)
                .map_err(|e| FrtError::Xrt(format!("RTP is too large: {e}")))?;
            let address = rtp.ping_address.checked_add(offset).ok_or_else(|| {
                FrtError::Xrt(format!("RTP address overflows for `{}`", rtp.alias))
            })?;
            writes.push((rtp.tile, address, u32::from_le_bytes(word)));
        }
    }
    let count = if graph.run_forever { u32::MAX } else { 1 };
    for (tile, address) in &graph.iterations {
        writes.push((*tile, *address, count));
    }
    writes.extend(core_control(graph, CORE_ENABLE));
    encode(&writes)
}

pub(super) fn stop(graph: &AieGraph) -> Result<Vec<u8>> {
    encode(&core_control(graph, CORE_DISABLE).collect::<Vec<_>>())
}

pub(super) fn abort(graph: &AieGraph) -> Result<Vec<u8>> {
    stop(graph)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::xrt::metadata::AieRtp;

    const T0: AieTile = AieTile { column: 24, row: 0 };
    const T1: AieTile = AieTile { column: 25, row: 0 };
    const K0: u32 = 0x18_0000;
    const K1: u32 = 0x19_0000;

    /// Two kernels on T0 and T1; T0's holds the runtime parameter and the
    /// iteration count.
    fn graph(run_forever: bool) -> AieGraph {
        AieGraph {
            name: "tapa_aie".into(),
            run_forever,
            cores: vec![T0, T1],
            iterations: vec![(T0, 0x2004)],
            rtps: vec![AieRtp {
                alias: "g.n".into(),
                arg: 3,
                bytes: 4,
                tile: T0,
                ping_address: 0x1fe0,
                pong_address: 0x1fe0,
            }],
            drains: Vec::new(),
        }
    }

    fn words(program: &[u8]) -> Vec<u32> {
        program
            .as_chunks::<4>()
            .0
            .iter()
            .map(|word| u32::from_le_bytes(*word))
            .collect()
    }

    #[test]
    fn start_encodes_reset_rtp_iterations_and_core_enable() {
        let program = start(&graph(true), &HashMap::from([(3, vec![64, 0, 0, 0])]), true)
            .expect("encode start");
        assert_eq!(
            words(&program),
            [
                18,
                K0,
                CORE_CONTROL,
                CORE_DISABLE,
                K1,
                CORE_CONTROL,
                CORE_DISABLE,
                K0,
                0x1fe0,
                64,
                K0,
                0x2004,
                u32::MAX,
                K0,
                CORE_CONTROL,
                CORE_ENABLE,
                K1,
                CORE_CONTROL,
                CORE_ENABLE,
            ]
        );
    }

    #[test]
    fn bounded_start_and_stop_have_exact_programs() {
        let program =
            start(&graph(false), &HashMap::from([(3, vec![7])]), false).expect("encode start");
        assert_eq!(
            words(&program),
            [
                12,
                K0,
                0x1fe0,
                7,
                K0,
                0x2004,
                1,
                K0,
                CORE_CONTROL,
                CORE_ENABLE,
                K1,
                CORE_CONTROL,
                CORE_ENABLE
            ]
        );
        let stopped = [
            6,
            K0,
            CORE_CONTROL,
            CORE_DISABLE,
            K1,
            CORE_CONTROL,
            CORE_DISABLE,
        ];
        assert_eq!(words(&stop(&graph(false)).expect("stop")), stopped);
        assert_eq!(words(&abort(&graph(false)).expect("abort")), stopped);
    }

    #[test]
    fn wide_rtp_values_use_consecutive_words() {
        let mut graph = graph(false);
        graph.rtps[0].bytes = 8;
        graph.cores.pop();
        let program =
            start(&graph, &HashMap::from([(3, vec![1, 2])]), false).expect("encode wide RTP");
        assert_eq!(
            words(&program),
            [
                12,
                K0,
                0x1fe0,
                0x0201,
                K0,
                0x1fe4,
                0,
                K0,
                0x2004,
                1,
                K0,
                CORE_CONTROL,
                CORE_ENABLE
            ]
        );
    }

    #[test]
    fn each_iteration_count_goes_to_its_own_tile() {
        let mut graph = graph(false);
        graph.iterations.push((T1, 0x2004));
        let program = start(&graph, &HashMap::from([(3, vec![64])]), false)
            .expect("encode multi-kernel start");
        assert_eq!(
            &words(&program)[4..10],
            [K0, 0x2004, 1, K1, 0x2004, 1],
            "the same address on two tiles is two writes"
        );
    }

    #[test]
    fn parameters_outside_the_kernels_tiles_or_double_buffered_are_rejected() {
        let error = |graph: &AieGraph| {
            start(graph, &HashMap::new(), false)
                .unwrap_err()
                .to_string()
        };
        let mut double = graph(false);
        double.rtps[0].pong_address = 0x2000;
        assert!(error(&double).contains("single-buffered"));
        let mut far = graph(false);
        far.rtps[0].tile = AieTile { column: 25, row: 1 };
        assert!(
            error(&far).contains("not RTP `g.n` in tile (25, 1)"),
            "{}",
            error(&far)
        );
        let mut far = graph(false);
        far.iterations[0].0 = AieTile { column: 25, row: 1 };
        assert!(error(&far).contains("not an iteration count in tile (25, 1)"));
    }

    #[test]
    fn missing_scalars_and_oversized_programs_are_errors() {
        let error = start(&graph(false), &HashMap::new(), false).unwrap_err();
        assert!(error.to_string().contains("not a scalar"), "{error}");
        encode(&vec![(T0, 0, 0); MAX_PROGRAM_WORDS / 3 + 1]).unwrap_err();
    }
}
