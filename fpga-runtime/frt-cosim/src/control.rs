//! The host's side of its control-register requests to a simulated kernel.
//!
//! See `frt_shm::control`: one request at a time, answered in order; the
//! testbench says when the kernel is done and ends the simulation when told.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use frt_shm::config::ControlEntry;
use frt_shm::control::{Reply, Request, REPLY_BYTES, REQUEST_BYTES};
use frt_shm::SharedMemoryQueue;

use crate::error::{CosimError, Result};

/// How long the host waits before it looks at the queues again.
const POLL: Duration = Duration::from_micros(50);
/// Requests the testbench has not taken yet, at most.
const DEPTH: u32 = 16;

pub struct Control {
    /// Requests to the testbench, replies from it.
    queues: Mutex<(SharedMemoryQueue, SharedMemoryQueue)>,
    /// The testbench said the kernel is done.
    done: AtomicBool,
    /// The simulation ended: nothing answers any more.
    closed: AtomicBool,
}

impl Control {
    pub fn create() -> Result<Self> {
        let requests = SharedMemoryQueue::create("ctrl_requests", DEPTH, REQUEST_BYTES as u32)?;
        let replies = SharedMemoryQueue::create("ctrl_replies", DEPTH, REPLY_BYTES as u32)?;
        Ok(Self {
            queues: Mutex::new((requests, replies)),
            done: AtomicBool::new(false),
            closed: AtomicBool::new(false),
        })
    }

    /// The queues, as the testbench opens them.
    pub fn entry(&self) -> ControlEntry {
        let queues = self.queues.lock().expect("control queues");
        let path = |queue: &SharedMemoryQueue| queue.path().to_string_lossy().into_owned();
        ControlEntry {
            requests: path(&queues.0),
            replies: path(&queues.1),
        }
    }

    /// Reads the register at `offset`.
    pub fn read(&self, offset: u32) -> Result<u32> {
        self.call(Request::Read { offset })
    }

    /// Writes `value` to the register at `offset`.
    pub fn write(&self, offset: u32, value: u32) -> Result<()> {
        self.call(Request::Write { offset, value }).map(drop)
    }

    /// Asks the testbench to end the simulation.
    pub fn finish(&self) -> Result<()> {
        let mut queues = self.queues.lock().expect("control queues");
        self.push(&mut queues.0, Request::Finish)
    }

    /// Whether the testbench said the kernel is done.
    pub fn done(&self) -> bool {
        if let Ok(mut queues) = self.queues.try_lock() {
            self.take_replies(&mut queues.1);
        }
        self.done.load(Ordering::Acquire)
    }

    /// The simulation ended: requests fail from now on.
    pub fn close(&self) {
        self.closed.store(true, Ordering::Release);
    }

    /// The simulation has started again.
    pub fn reopen(&self) {
        self.closed.store(false, Ordering::Release);
        self.done.store(false, Ordering::Release);
    }

    fn call(&self, request: Request) -> Result<u32> {
        let mut queues = self.queues.lock().expect("control queues");
        self.push(&mut queues.0, request)?;
        loop {
            if let Some(value) = self.take_replies(&mut queues.1) {
                return Ok(value);
            }
            self.check_open()?;
            std::thread::sleep(POLL);
        }
    }

    fn push(&self, requests: &mut SharedMemoryQueue, request: Request) -> Result<()> {
        while requests.try_push(&request.encode()).is_err() {
            self.check_open()?;
            std::thread::sleep(POLL);
        }
        Ok(())
    }

    /// Takes the replies waiting, noting the end of the run; the value of
    /// the first answer, if one came.
    fn take_replies(&self, replies: &mut SharedMemoryQueue) -> Option<u32> {
        let mut reply = [0u8; REPLY_BYTES];
        while replies.pop_into(&mut reply) {
            match Reply::decode(&reply) {
                Some(Reply::Value(value)) => return Some(value),
                Some(Reply::Done) => self.done.store(true, Ordering::Release),
                None => {}
            }
        }
        None
    }

    fn check_open(&self) -> Result<()> {
        if self.closed.load(Ordering::Acquire) {
            return Err(CosimError::Unanswered);
        }
        Ok(())
    }
}
