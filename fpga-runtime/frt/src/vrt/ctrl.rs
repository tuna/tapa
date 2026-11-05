//! The `ap_ctrl` handshake both VRT backends drive over their register seam.

use std::time::Instant;

use super::vbin::{register_words, Arg};
use crate::elapsed_ns;
use crate::error::{FrtError, Result};

pub(super) const CTRL_OFFSET: u32 = 0;
pub(super) const CTRL_START: u32 = 1;
pub(super) const CTRL_DONE: u32 = 1 << 1;

/// Kernel lifecycle: idle, running since the enclosed instant, or finished.
#[derive(Clone, Copy)]
pub(super) enum ExecutionState {
    Idle,
    Running(Instant),
    Finished,
}

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
    fn ctrl_write(&mut self, offset: u32, value: u32) -> Result<()>;
    fn ctrl_read(&mut self, offset: u32) -> Result<u32>;
    fn execution_state(&mut self) -> &mut ExecutionState;
    fn compute_ns(&mut self) -> &mut u64;

    /// A control write for one register image, word by word.
    fn write_value(&mut self, arg: &Arg, value: &[u8]) -> Result<()> {
        for (offset, word) in register_words(arg.offset, arg.range, value)? {
            self.ctrl_write(offset, word)?;
        }
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
}
