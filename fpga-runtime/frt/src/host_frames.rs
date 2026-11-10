//! Frames between a host stream and a device that moves whole frames.
//!
//! The device is a GMIO of a top on the AI Engine, or a ring in device
//! memory. A device thread fills a frame from the host's queue, or empties
//! one into it, element by element; the queue the host binds may change
//! from run to run.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use frt_shm::SharedMemoryQueue;

use crate::error::{FrtError, Result};

/// How long a device thread waits before it looks at an empty or full queue,
/// or a device counter, again.
pub const POLL: Duration = Duration::from_micros(100);

/// One stream argument the device moves frames of.
pub struct FrameQueue {
    /// The argument, as messages name it.
    pub name: String,
    pub element_bytes: usize,
    /// The host queue bound for the current run.
    bound: Mutex<Option<String>>,
    stop: AtomicBool,
}

impl FrameQueue {
    pub fn new(name: String, element_bytes: usize) -> Self {
        Self {
            name,
            element_bytes,
            bound: Mutex::new(None),
            stop: AtomicBool::new(false),
        }
    }

    /// Binds the host queue at `path` for the next run.
    pub fn bind(&self, path: &str) {
        *self.bound.lock().expect("bound queue") = Some(path.to_owned());
    }

    pub fn is_bound(&self) -> bool {
        self.bound.lock().expect("bound queue").is_some()
    }

    /// Stops the thread moving this argument's frames.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Release);
    }

    pub fn stopped(&self) -> bool {
        self.stop.load(Ordering::Acquire)
    }

    /// Waits until `done`, or fails once the device stops the thread.
    pub fn wait_for(&self, done: impl Fn() -> Result<bool>) -> Result<()> {
        while !done()? {
            if self.stopped() {
                return Err(FrtError::Xrt(format!(
                    "the stream argument `{}` stopped short of its frames",
                    self.name
                )));
            }
            std::thread::sleep(POLL);
        }
        Ok(())
    }
}

/// The host queue a device thread has open: the one bound for the current
/// run.
#[derive(Default)]
pub struct HostQueue {
    open: Option<(String, SharedMemoryQueue)>,
    /// The host closed the stream: nothing more until it is bound anew.
    closed: bool,
}

impl HostQueue {
    /// The queue bound now, reopened when a run bound another.
    fn current(&mut self, queue: &FrameQueue) -> Result<&mut SharedMemoryQueue> {
        let bound = queue
            .bound
            .lock()
            .expect("bound queue")
            .clone()
            .unwrap_or_default();
        if self.open.as_ref().is_none_or(|(path, _)| *path != bound) {
            let host = SharedMemoryQueue::open(&bound)?;
            if host.width() <= queue.element_bytes {
                return Err(FrtError::Xrt(format!(
                    "the host stream of `{}` has {}-byte slots, too narrow for {}-byte elements",
                    queue.name,
                    host.width(),
                    queue.element_bytes
                )));
            }
            self.open = Some((bound, host));
            self.closed = false;
        }
        Ok(&mut self.open.as_mut().expect("opened just above").1)
    }

    /// Fills `frame` from the host, zero-padding it if the host closes the
    /// stream part way; false once the device stops the thread.
    pub fn fill(&mut self, queue: &FrameQueue, frame: &mut [u8]) -> Result<bool> {
        let element = queue.element_bytes;
        let mut filled = 0;
        while filled < frame.len() {
            if queue.stopped() {
                return Ok(false);
            }
            // Reopens on a new binding, which also clears `closed`.
            let width = self.current(queue)?.width();
            if self.closed && filled > 0 {
                break;
            }
            let mut slot = vec![0u8; width];
            if self.closed || !self.current(queue)?.pop_into(&mut slot) {
                std::thread::sleep(POLL);
                continue;
            }
            if slot[element] != 0 {
                // The end-of-transmission token: pad what is there.
                self.closed = true;
                continue;
            }
            frame[filled..filled + element].copy_from_slice(&slot[..element]);
            filled += element;
        }
        frame[filled..].fill(0);
        Ok(true)
    }

    /// Pushes `frame` to the host element by element; false once the device
    /// stops the thread.
    pub fn drain(&mut self, queue: &FrameQueue, frame: &[u8]) -> Result<bool> {
        for value in frame.chunks_exact(queue.element_bytes) {
            let host = self.current(queue)?;
            // The flag and the padding stay clear.
            let mut slot = value.to_vec();
            slot.resize(host.width(), 0);
            while host.try_push(&slot).is_err() {
                if queue.stopped() {
                    return Ok(false);
                }
                std::thread::sleep(POLL);
            }
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bound(queue: &SharedMemoryQueue) -> FrameQueue {
        let frames = FrameQueue::new("g".into(), 8);
        frames.bind(&queue.path().to_string_lossy());
        frames
    }

    /// An element of two words, as bytes.
    fn element(words: [u32; 2]) -> Vec<u8> {
        words.iter().flat_map(|word| word.to_le_bytes()).collect()
    }

    /// A slot: an element and the end-of-transmission flag after it.
    fn slot(words: [u32; 2], eot: bool) -> Vec<u8> {
        let mut slot = element(words);
        slot.push(u8::from(eot));
        slot
    }

    #[test]
    fn a_frame_the_host_closes_part_way_is_zero_padded() {
        let mut queue = SharedMemoryQueue::create("frt_frames_fill", 8, 9).expect("queue");
        for element in [
            slot([1, 2], false),
            slot([3, 4], false),
            slot([5, 6], false),
        ] {
            queue.try_push(&element).expect("push");
        }
        queue.try_push(&slot([0, 0], true)).expect("push");
        let frames = bound(&queue);
        let mut host = HostQueue::default();
        let mut frame = [9u8; 16];
        assert!(host.fill(&frames, &mut frame).expect("fill"));
        assert_eq!(frame[..8], element([1, 2]));
        assert_eq!(frame[8..], element([3, 4]));
        assert!(host.fill(&frames, &mut frame).expect("fill"));
        assert_eq!(frame[..8], element([5, 6]));
        assert_eq!(frame[8..], [0; 8]);
    }

    #[test]
    fn a_frame_reaches_the_host_element_by_element() {
        let mut queue = SharedMemoryQueue::create("frt_frames_drain", 8, 9).expect("queue");
        let frames = bound(&queue);
        let mut host = HostQueue::default();
        let frame: Vec<u8> = [element([1, 2]), element([3, 4])].concat();
        assert!(host.drain(&frames, &frame).expect("drain"));
        let mut got = vec![0u8; 9];
        for want in [slot([1, 2], false), slot([3, 4], false)] {
            assert!(queue.pop_into(&mut got));
            assert_eq!(got, want);
        }
        assert!(!queue.pop_into(&mut got));
    }
}
