//! Graph control through a PS kernel running on a VCK5000's APU.

use std::collections::HashMap;
use std::ffi::CString;

use super::api::{Api, Handle, RUN_COMPLETED, SYNC_FROM_DEVICE, SYNC_TO_DEVICE};
use super::device::{check, non_null};
use super::metadata::AieGraph;
use crate::error::{FrtError, Result};

pub(super) const KERNEL_NAME: &str = "tapa_aie_control";

const START: u32 = 0;
const STOP: u32 = 1;
const ABORT: u32 = 2;

pub(super) struct PsControl {
    api: &'static Api,
    device: Handle,
    kernel: Handle,
    run: Handle,
}

// SAFETY: FRT accesses each PS controller from one thread at a time; its XRT
// handles are reference-counted objects inside XRT.
unsafe impl Send for PsControl {}

impl PsControl {
    pub(super) fn open(api: &'static Api, device: Handle, uuid: &[u8; 16]) -> Result<Self> {
        let name = CString::new(KERNEL_NAME).expect("PS-control kernel name has no NUL");
        // SAFETY: live device and xclbin UUID, and a valid C string.
        let kernel = unsafe { (api.kernel_open)(device, uuid.as_ptr(), name.as_ptr()) };
        let mut this = Self {
            api,
            device,
            kernel: non_null(kernel, "open the AIE APU control kernel")?,
            run: std::ptr::null_mut(),
        };
        // SAFETY: live PS-kernel handle.
        let run = unsafe { (api.run_open)(this.kernel) };
        this.run = non_null(run, "create an AIE APU control run")?;
        Ok(this)
    }

    pub(super) fn execute(&self, commands: &[u8]) -> Result<()> {
        if commands.is_empty() {
            return Err(FrtError::Xrt("empty AIE APU control command".into()));
        }
        // SAFETY: live PS-kernel handle and argument zero is its command BO.
        let group = unsafe { (self.api.kernel_arg_group_id)(self.kernel, 0) };
        let group = u32::try_from(group)
            .map_err(|e| FrtError::Xrt(format!("APU command buffer has no memory bank: {e}")))?;
        // SAFETY: live device handle.
        let bo = unsafe { (self.api.bo_alloc)(self.device, commands.len(), 0, group) };
        let bo = non_null(bo, "allocate the AIE APU command buffer")?;
        let result = (|| {
            // SAFETY: `commands` holds `commands.len()` readable bytes.
            let rc =
                unsafe { (self.api.bo_write)(bo, commands.as_ptr().cast(), commands.len(), 0) };
            check(rc, "write the AIE APU command buffer")?;
            // SAFETY: live BO of `commands.len()` bytes.
            let rc = unsafe { (self.api.bo_sync)(bo, SYNC_TO_DEVICE, commands.len(), 0) };
            check(rc, "sync the AIE APU command buffer")?;
            // SAFETY: live run and BO handles; argument zero is the command BO.
            let rc = unsafe { (self.api.run_set_arg)(self.run, 0, bo) };
            check(rc, "bind the AIE APU command buffer")?;
            let bytes = u32::try_from(commands.len())
                .map_err(|e| FrtError::Xrt(format!("APU command is too large: {e}")))?;
            // SAFETY: live run handle; argument one is a 32-bit byte count.
            let rc = unsafe { (self.api.run_set_arg)(self.run, 1, bytes) };
            check(rc, "set the AIE APU command size")?;
            // SAFETY: live run handle with both arguments set.
            let rc = unsafe { (self.api.run_start)(self.run) };
            check(rc, "start the AIE APU control kernel")?;
            // SAFETY: live run handle.
            let state = unsafe { (self.api.run_wait)(self.run) };
            if state != RUN_COMPLETED {
                return Err(FrtError::Xrt(format!(
                    "the AIE APU control run ended in state {state}, not completed"
                )));
            }
            // SAFETY: live BO; the APU wrote a 32-bit status at offset zero.
            let rc = unsafe { (self.api.bo_sync)(bo, SYNC_FROM_DEVICE, 4, 0) };
            check(rc, "read the AIE APU control status")?;
            let mut status = [0u8; 4];
            // SAFETY: `status` holds four writable bytes.
            let rc = unsafe { (self.api.bo_read)(bo, status.as_mut_ptr().cast(), 4, 0) };
            check(rc, "unstage the AIE APU control status")?;
            let status = u32::from_le_bytes(status);
            if status != 0 {
                return Err(FrtError::Xrt(format!(
                    "the AIE APU graph control failed with status {status}"
                )));
            }
            Ok(())
        })();
        // SAFETY: the BO is no longer used by the completed run.
        unsafe { (self.api.bo_free)(bo) };
        result
    }
}

impl Drop for PsControl {
    fn drop(&mut self) {
        if !self.run.is_null() {
            // SAFETY: live run handle, closed once here.
            unsafe { (self.api.run_close)(self.run) };
        }
        if !self.kernel.is_null() {
            // SAFETY: live PS-kernel handle with no open run.
            unsafe { (self.api.kernel_close)(self.kernel) };
        }
    }
}

fn push_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn start_command(graph: &AieGraph, values: &HashMap<u32, Vec<u8>>, reset: bool) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    push_u32(&mut out, START);
    push_u32(&mut out, u32::from(reset));
    push_u32(&mut out, if graph.run_forever { u32::MAX } else { 1 });
    push_u32(
        &mut out,
        u32::try_from(graph.rtps.len())
            .map_err(|e| FrtError::Xrt(format!("too many AIE runtime parameters: {e}")))?,
    );
    for rtp in &graph.rtps {
        if rtp.alias.contains('\0') {
            return Err(FrtError::Xrt(format!(
                "runtime parameter name `{}` contains NUL",
                rtp.alias
            )));
        }
        let mut value = values.get(&rtp.arg).cloned().ok_or_else(|| {
            FrtError::Xrt(format!(
                "runtime parameter `{}` carries argument {}, which is not a scalar",
                rtp.alias, rtp.arg
            ))
        })?;
        value.resize(rtp.bytes, 0);
        push_u32(
            &mut out,
            u32::try_from(rtp.alias.len())
                .map_err(|e| FrtError::Xrt(format!("RTP name is too long: {e}")))?,
        );
        push_u32(
            &mut out,
            u32::try_from(value.len())
                .map_err(|e| FrtError::Xrt(format!("RTP value is too large: {e}")))?,
        );
        out.extend_from_slice(rtp.alias.as_bytes());
        out.extend_from_slice(&value);
    }
    Ok(out)
}

fn stop_command(run_forever: bool) -> Vec<u8> {
    let mut out = Vec::new();
    push_u32(&mut out, STOP);
    out.extend_from_slice(&u64::from(run_forever).to_le_bytes());
    out
}

fn abort_command() -> Vec<u8> {
    ABORT.to_le_bytes().to_vec()
}

pub(super) fn start(
    graph: &AieGraph,
    values: &HashMap<u32, Vec<u8>>,
    reset: bool,
) -> Result<Vec<u8>> {
    start_command(graph, values, reset)
}

pub(super) fn stop(run_forever: bool) -> Vec<u8> {
    stop_command(run_forever)
}

pub(super) fn abort() -> Vec<u8> {
    abort_command()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::xrt::metadata::AieRtp;

    fn graph(run_forever: bool) -> AieGraph {
        AieGraph {
            name: "tapa_aie".into(),
            run_forever,
            rtps: vec![AieRtp {
                alias: "g.n".into(),
                arg: 3,
                bytes: 4,
            }],
        }
    }

    #[test]
    fn start_encodes_reset_iterations_and_rtp_bytes() {
        let command = start_command(&graph(true), &HashMap::from([(3, vec![64, 0, 0, 0])]), true)
            .expect("encode start");
        assert_eq!(
            command,
            [
                0, 0, 0, 0, 1, 0, 0, 0, 255, 255, 255, 255, 1, 0, 0, 0, 3, 0, 0, 0, 4, 0, 0, 0,
                b'g', b'.', b'n', 64, 0, 0, 0,
            ]
        );
    }

    #[test]
    fn bounded_start_and_control_actions_have_exact_wire_formats() {
        let command = start_command(&graph(false), &HashMap::from([(3, vec![7])]), false)
            .expect("encode start");
        assert_eq!(
            &command[..16],
            &[0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0]
        );
        assert_eq!(&command[24..], &[b'g', b'.', b'n', 7, 0, 0, 0]);
        assert_eq!(stop_command(true), [1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(stop_command(false), [1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(abort_command(), [2, 0, 0, 0]);
    }

    #[test]
    fn start_rejects_missing_scalars_and_nul_in_names() {
        let missing = start_command(&graph(false), &HashMap::new(), false).unwrap_err();
        assert!(missing.to_string().contains("not a scalar"), "{missing}");
        let mut graph = graph(false);
        graph.rtps[0].alias = "bad\0name".into();
        let invalid = start_command(&graph, &HashMap::from([(3, vec![0])]), false).unwrap_err();
        assert!(invalid.to_string().contains("contains NUL"), "{invalid}");
    }

    #[test]
    fn wide_rtp_values_are_normalized_before_encoding() {
        let mut graph = graph(false);
        graph.rtps[0].bytes = 8;
        let command = start_command(&graph, &HashMap::from([(3, vec![1, 2])]), false)
            .expect("encode wide RTP");
        assert_eq!(&command[24..], &[b'g', b'.', b'n', 1, 2, 0, 0, 0, 0, 0, 0]);
    }
}
