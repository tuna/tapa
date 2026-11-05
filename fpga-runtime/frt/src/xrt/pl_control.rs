use std::collections::{BTreeSet, HashMap};
use std::ffi::CString;

use super::api::{Api, Handle, RUN_COMPLETED, SYNC_TO_DEVICE};
use super::device::{check, non_null};
use super::metadata::AieGraph;
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
        // SAFETY: live PL-kernel handle and argument two is its command BO.
        let group = unsafe { (self.api.kernel_arg_group_id)(self.kernel, 2) };
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
            // SAFETY: live run handle; argument one is the control packet ID.
            let rc = unsafe { (self.api.run_set_arg)(self.run, 1, 0u32) };
            check(rc, "set the AIE PL control packet ID")?;
            // SAFETY: live run and BO handles; argument two is the command BO.
            let rc = unsafe { (self.api.run_set_arg)(self.run, 2, bo) };
            check(rc, "bind the AIE PL command buffer")?;
            // SAFETY: live run handle with both arguments set.
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

fn encode(writes: &[(u32, u32)]) -> Result<Vec<u8>> {
    let words = writes
        .len()
        .checked_mul(2)
        .filter(|words| *words <= MAX_PROGRAM_WORDS)
        .ok_or_else(|| FrtError::Xrt("AIE PL control program is too large".into()))?;
    let mut out = Vec::with_capacity((words + 1) * 4);
    out.extend_from_slice(&(words as u32).to_le_bytes());
    for (address, value) in writes {
        out.extend_from_slice(&address.to_le_bytes());
        out.extend_from_slice(&value.to_le_bytes());
    }
    Ok(out)
}

fn validate_memory_map(graph: &AieGraph) -> Result<BTreeSet<u32>> {
    let iterations = graph
        .iteration_addresses
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    if iterations.is_empty() {
        return Err(FrtError::Xrt("AIE graph has no iteration register".into()));
    }
    let mut addresses = iterations.clone();
    for rtp in &graph.rtps {
        if rtp.ping_address != rtp.pong_address {
            return Err(FrtError::Xrt(format!(
                "AIE PL control requires single-buffered RTP `{}`",
                rtp.alias
            )));
        }
        if !addresses.insert(rtp.ping_address) {
            return Err(FrtError::Xrt(format!(
                "AIE PL control cannot broadcast colliding register address {:#x}",
                rtp.ping_address
            )));
        }
    }
    Ok(iterations)
}

pub(super) fn start(
    graph: &AieGraph,
    values: &HashMap<u32, Vec<u8>>,
    reset: bool,
) -> Result<Vec<u8>> {
    let iterations = validate_memory_map(graph)?;
    let mut writes = Vec::new();
    if reset {
        writes.push((CORE_CONTROL, CORE_DISABLE));
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
            writes.push((address, u32::from_le_bytes(word)));
        }
    }
    let count = if graph.run_forever { u32::MAX } else { 1 };
    for address in iterations {
        writes.push((address, count));
    }
    writes.push((CORE_CONTROL, CORE_ENABLE));
    encode(&writes)
}

pub(super) fn stop() -> Result<Vec<u8>> {
    encode(&[(CORE_CONTROL, CORE_DISABLE)])
}

pub(super) fn abort() -> Result<Vec<u8>> {
    stop()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::xrt::metadata::AieRtp;

    fn graph(run_forever: bool) -> AieGraph {
        AieGraph {
            name: "tapa_aie".into(),
            run_forever,
            iteration_addresses: vec![0x2004],
            rtps: vec![AieRtp {
                alias: "g.n".into(),
                arg: 3,
                bytes: 4,
                ping_address: 0x1fe0,
                pong_address: 0x1fe0,
            }],
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
                8,
                CORE_CONTROL,
                CORE_DISABLE,
                0x1fe0,
                64,
                0x2004,
                u32::MAX,
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
            [6, 0x1fe0, 7, 0x2004, 1, CORE_CONTROL, CORE_ENABLE]
        );
        assert_eq!(
            words(&stop().expect("stop")),
            [2, CORE_CONTROL, CORE_DISABLE]
        );
        assert_eq!(
            words(&abort().expect("abort")),
            [2, CORE_CONTROL, CORE_DISABLE]
        );
    }

    #[test]
    fn wide_rtp_values_use_consecutive_words() {
        let mut graph = graph(false);
        graph.rtps[0].bytes = 8;
        let program =
            start(&graph, &HashMap::from([(3, vec![1, 2])]), false).expect("encode wide RTP");
        assert_eq!(
            words(&program),
            [
                8,
                0x1fe0,
                0x0201,
                0x1fe4,
                0,
                0x2004,
                1,
                CORE_CONTROL,
                CORE_ENABLE
            ]
        );
    }

    #[test]
    fn distinct_iteration_addresses_are_each_programmed() {
        let mut graph = graph(false);
        graph.iteration_addresses.push(4);
        let program = start(&graph, &HashMap::from([(3, vec![64])]), false)
            .expect("encode multi-kernel start");
        assert_eq!(
            words(&program),
            [8, 0x1fe0, 64, 4, 1, 0x2004, 1, CORE_CONTROL, CORE_ENABLE]
        );
    }

    #[test]
    fn unsafe_memory_maps_are_rejected() {
        let mut graph = graph(false);
        graph.rtps[0].pong_address = 0x2000;
        assert!(start(&graph, &HashMap::new(), false)
            .unwrap_err()
            .to_string()
            .contains("single-buffered"));
        graph.rtps[0].pong_address = graph.rtps[0].ping_address;
        graph.rtps.push(graph.rtps[0].clone());
        assert!(start(&graph, &HashMap::new(), false)
            .unwrap_err()
            .to_string()
            .contains("colliding register"));
    }

    #[test]
    fn missing_scalars_and_oversized_programs_are_errors() {
        let error = start(&graph(false), &HashMap::new(), false).unwrap_err();
        assert!(error.to_string().contains("not a scalar"), "{error}");
        encode(&vec![(0, 0); MAX_PROGRAM_WORDS / 2 + 1]).unwrap_err();
    }
}
