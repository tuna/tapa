//! The `ap_ctrl` handshake both VRT backends drive over their register seam.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use super::vbin::{register_words, Arg, ArgKind, Kernel};
use crate::device::ExecutionState;
use crate::elapsed_ns;
use crate::error::{FrtError, Result};

pub(super) const CTRL_OFFSET: u32 = 0;
pub(super) const CTRL_START: u32 = 1;
pub(super) const CTRL_DONE: u32 = 1 << 1;

/// A kernel has this long to raise DONE; neither backend can cancel a run.
pub(super) const FINISH_TIMEOUT: Duration = Duration::from_secs(60);
const FINISH_POLL: Duration = Duration::from_millis(1);

impl ExecutionState {
    /// Control-channel mutations are rejected while the kernel runs.
    pub(super) fn ensure_not_running(self, action: &str, kernel: &str) -> Result<()> {
        if matches!(self, Self::Running(_)) {
            Err(FrtError::Vrt(format!(
                "cannot {action} while the {kernel} is running"
            )))
        } else {
            Ok(())
        }
    }
}

/// The register read/write seam the two backends implement; everything else
/// about the control channel is shared here.
pub(super) trait ControlChannel {
    /// How diagnostics name the kernel ("VRT kernel", "SLASH simulation
    /// kernel").
    const LABEL: &'static str;

    fn ctrl_write(&mut self, offset: u32, value: u32) -> Result<()>;
    fn ctrl_read(&mut self, offset: u32) -> Result<u32>;
    fn execution_state(&mut self) -> &mut ExecutionState;
    fn compute_ns(&mut self) -> &mut u64;
    fn kernel(&self) -> &Kernel;
    fn scalars(&self) -> &BTreeMap<u32, Vec<u8>>;
    fn scalars_mut(&mut self) -> &mut BTreeMap<u32, Vec<u8>>;
    /// The device-visible address bound to buffer argument `index`, if bound.
    fn buffer_address(&self, index: u32) -> Option<u64>;

    /// A control write for one register image, word by word.
    fn write_value(&mut self, arg: &Arg, value: &[u8]) -> Result<()> {
        for (offset, word) in register_words(arg.offset, arg.range, value)? {
            self.ctrl_write(offset, word)?;
        }
        Ok(())
    }

    /// Stage one scalar argument for the next run.
    fn set_scalar(&mut self, index: u32, value: &[u8]) -> Result<()> {
        self.execution_state()
            .ensure_not_running("change an argument", Self::LABEL)?;
        self.kernel().validate_scalar_arg(index, value)?;
        self.scalars_mut().insert(index, value.to_vec());
        Ok(())
    }

    /// Write every writable argument's register image and pulse START.
    fn start(&mut self) -> Result<()> {
        self.execution_state()
            .ensure_not_running("start the kernel again", Self::LABEL)?;
        for arg in self.kernel().args.clone() {
            if !arg.writable {
                continue;
            }
            let value = match arg.kind {
                ArgKind::Scalar => self.scalars().get(&arg.index).cloned().ok_or_else(|| {
                    FrtError::Vrt(format!(
                        "unbound scalar argument {} (`{}`)",
                        arg.index, arg.name
                    ))
                })?,
                ArgKind::Buffer => self
                    .buffer_address(arg.index)
                    .map(|address| address.to_le_bytes().to_vec())
                    .ok_or_else(|| {
                        FrtError::Vrt(format!(
                            "unbound buffer argument {} (`{}`)",
                            arg.index, arg.name
                        ))
                    })?,
            };
            self.write_value(&arg, &value)?;
        }
        let started = Instant::now();
        self.ctrl_write(CTRL_OFFSET, CTRL_START)?;
        *self.compute_ns() = 0;
        *self.execution_state() = ExecutionState::Running(started);
        Ok(())
    }

    /// The DONE bit, latching the compute time on the running→finished edge.
    fn poll_finished(&mut self) -> Result<bool> {
        let ExecutionState::Running(started) = *self.execution_state() else {
            return Ok(matches!(self.execution_state(), ExecutionState::Finished));
        };
        if self.ctrl_read(CTRL_OFFSET)? & CTRL_DONE == 0 {
            return Ok(false);
        }
        *self.compute_ns() = elapsed_ns(started);
        *self.execution_state() = ExecutionState::Finished;
        Ok(true)
    }

    /// Poll the DONE bit until the kernel finishes; `timeout` builds the
    /// error reported when `FINISH_TIMEOUT` expires.
    fn wait_finished(&mut self, timeout: impl FnOnce(u64) -> FrtError) -> Result<()> {
        if !matches!(*self.execution_state(), ExecutionState::Running(_)) {
            return Ok(());
        }
        let wait_started = Instant::now();
        while !self.poll_finished()? {
            if wait_started.elapsed() >= FINISH_TIMEOUT {
                return Err(timeout(FINISH_TIMEOUT.as_secs()));
            }
            std::thread::sleep(FINISH_POLL);
        }
        Ok(())
    }
}
