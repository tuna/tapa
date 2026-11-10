//! The host side of a ring, on XRT or in cosimulation.
//!
//! A ring of frame slots in device memory carries a frame argument of a PL
//! top (see `tapa_ir::Ring`). A pump per ring moves frames between the
//! host's queue and the slots, handing them over through two counters in
//! the control registers, each counting modulo 2^32: the host's, which it
//! writes, and the kernel's, which it reads. Frame f sits in slot f % slots.
//!
//! An input pump fills a frame from the host, waits for a free slot (fewer
//! than `slots` frames the kernel has not taken), writes the frame there and
//! raises the host's count. An output pump waits for a frame the kernel has
//! written, reads it, hands it to the host and raises the host's count.
//! After a run the kernel's count is final, since the top is done only
//! once every frame it released is in memory: an output delivers up to it.

use std::sync::atomic::{AtomicU32, Ordering};

use crate::error::{FrtError, Result};
use crate::host_frames::{FrameQueue, HostQueue, POLL};

/// What a ring's backend provides: the top's control registers and the
/// ring's memory.
pub trait RingAccess: Send + Sync {
    fn read_register(&self, offset: u32) -> Result<u32>;
    fn write_register(&self, offset: u32, value: u32) -> Result<()>;
    /// Writes `data` at byte `at` of the ring, visible to the kernel.
    fn write_ring(&self, at: usize, data: &[u8]) -> Result<()>;
    /// Reads what the kernel wrote at byte `at` of the ring.
    fn read_ring(&self, at: usize, data: &mut [u8]) -> Result<()>;
}

/// Where a ring's registers are, and its shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RingLayout {
    /// The host argument the ring carries.
    pub index: u32,
    pub input: bool,
    pub element_bytes: usize,
    pub frame: u32,
    pub slots: u32,
    pub host_frames: u32,
    pub kernel_frames: u32,
}

impl RingLayout {
    pub fn frame_bytes(&self) -> usize {
        self.frame as usize * self.element_bytes
    }

    pub fn ring_bytes(&self) -> usize {
        self.frame_bytes() * self.slots as usize
    }

    /// The layout of a ring whose shape register reads `shape` (see
    /// `tapa_ir::ring_registers`).
    pub fn with_shape(mut self, shape: u64) -> Result<Self> {
        self.frame = (shape & 0xffff_ffff) as u32;
        self.slots = ((shape >> 32) & 0x7fff_ffff) as u32;
        self.input = shape >> 63 != 0;
        if self.frame == 0 || self.slots == 0 {
            return Err(FrtError::MetadataParse(format!(
                "the ring of argument {} reports {} frames of {} elements",
                self.index, self.slots, self.frame
            )));
        }
        Ok(self)
    }
}

/// One ring's pump state, shared by its thread and the device.
pub struct RingPump {
    pub layout: RingLayout,
    pub frames: FrameQueue,
    /// The host's count, as last written.
    host: AtomicU32,
}

impl RingPump {
    /// A pump that starts from the host's count `host`.
    pub fn new(layout: RingLayout, name: String, host: u32) -> Self {
        let frames = FrameQueue::new(name, layout.element_bytes);
        Self {
            layout,
            frames,
            host: AtomicU32::new(host),
        }
    }

    /// The host's count, as last written.
    pub fn host_frames(&self) -> u32 {
        self.host.load(Ordering::Acquire)
    }

    /// Whether the host has every output frame the kernel has written.
    pub fn delivered(&self, access: &dyn RingAccess) -> Result<bool> {
        if self.layout.input {
            return Ok(true);
        }
        let written = access.read_register(self.layout.kernel_frames)?;
        Ok(self.host.load(Ordering::Acquire) == written)
    }

    /// Waits until the host has every output frame the kernel has written.
    pub fn deliver_all(&self, access: &dyn RingAccess) -> Result<()> {
        if self.layout.input {
            return Ok(());
        }
        let written = access.read_register(self.layout.kernel_frames)?;
        self.frames
            .wait_for(|| Ok(self.host.load(Ordering::Acquire) == written))
    }

    /// Moves the ring's frames until the device stops the pump.
    pub fn run(&self, access: &dyn RingAccess) -> Result<()> {
        let layout = &self.layout;
        let mut frame = vec![0u8; layout.frame_bytes()];
        let mut host = HostQueue::default();
        loop {
            let count = self.host.load(Ordering::Acquire);
            let at = (count % layout.slots) as usize * layout.frame_bytes();
            // A free slot (input) or a frame to take (output).
            let ready = || -> Result<bool> {
                let kernel = access.read_register(layout.kernel_frames)?;
                Ok(if layout.input {
                    count.wrapping_sub(kernel) < layout.slots
                } else {
                    kernel != count
                })
            };
            if layout.input {
                if !host.fill(&self.frames, &mut frame)? {
                    return Ok(());
                }
                if !self.until(ready)? {
                    return Ok(());
                }
                access.write_ring(at, &frame)?;
            } else {
                if !self.until(ready)? {
                    return Ok(());
                }
                access.read_ring(at, &mut frame)?;
                if !host.drain(&self.frames, &frame)? {
                    return Ok(());
                }
            }
            access.write_register(layout.host_frames, count.wrapping_add(1))?;
            self.host.store(count.wrapping_add(1), Ordering::Release);
        }
    }

    /// Waits until `ready`; false once the device stops the pump.
    fn until(&self, ready: impl Fn() -> Result<bool>) -> Result<bool> {
        while !ready()? {
            if self.frames.stopped() {
                return Ok(false);
            }
            std::thread::sleep(POLL);
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frt_shm::SharedMemoryQueue;
    use std::sync::{Arc, Mutex};

    /// A kernel that takes (input) or writes (output) a frame whenever the
    /// test says, over a ring in host memory.
    struct FakeRing {
        registers: Mutex<[u32; 2]>,
        memory: Mutex<Vec<u8>>,
    }

    impl RingAccess for FakeRing {
        fn read_register(&self, offset: u32) -> Result<u32> {
            Ok(self.registers.lock().expect("registers")[offset as usize])
        }
        fn write_register(&self, offset: u32, value: u32) -> Result<()> {
            self.registers.lock().expect("registers")[offset as usize] = value;
            Ok(())
        }
        fn write_ring(&self, at: usize, data: &[u8]) -> Result<()> {
            self.memory.lock().expect("memory")[at..at + data.len()].copy_from_slice(data);
            Ok(())
        }
        fn read_ring(&self, at: usize, data: &mut [u8]) -> Result<()> {
            data.copy_from_slice(&self.memory.lock().expect("memory")[at..at + data.len()]);
            Ok(())
        }
    }

    fn layout(input: bool) -> RingLayout {
        RingLayout {
            index: 0,
            input,
            element_bytes: 1,
            frame: 2,
            slots: 2,
            host_frames: 0,
            kernel_frames: 1,
        }
    }

    fn wait(done: impl Fn() -> bool) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !done() {
            assert!(std::time::Instant::now() < deadline, "timed out");
            std::thread::sleep(POLL);
        }
    }

    #[test]
    fn an_input_ring_fills_free_slots_only_and_wraps() {
        let mut queue = SharedMemoryQueue::create("frt_ring_input", 8, 2).expect("queue");
        for value in 1..=6u8 {
            queue.try_push(&[value, 0]).expect("push");
        }
        let ring = Arc::new(FakeRing {
            registers: Mutex::new([0, 0]),
            memory: Mutex::new(vec![0; 4]),
        });
        let pump = Arc::new(RingPump::new(layout(true), "in".into(), 0));
        pump.frames.bind(&queue.path().to_string_lossy());
        let thread = {
            let (ring, pump) = (Arc::clone(&ring), Arc::clone(&pump));
            std::thread::spawn(move || pump.run(ring.as_ref()))
        };
        // Two slots: the third frame waits for the kernel to take one.
        wait(|| ring.registers.lock().expect("registers")[0] == 2);
        std::thread::sleep(POLL * 20);
        assert_eq!(ring.registers.lock().expect("registers")[0], 2);
        assert_eq!(*ring.memory.lock().expect("memory"), [1, 2, 3, 4]);
        ring.registers.lock().expect("registers")[1] = 1;
        wait(|| ring.registers.lock().expect("registers")[0] == 3);
        assert_eq!(*ring.memory.lock().expect("memory"), [5, 6, 3, 4]);
        pump.frames.stop();
        thread.join().expect("join").expect("pump");
    }

    #[test]
    fn an_output_ring_delivers_every_frame_the_kernel_wrote() {
        let mut queue = SharedMemoryQueue::create("frt_ring_output", 8, 2).expect("queue");
        let ring = Arc::new(FakeRing {
            registers: Mutex::new([1, 1]),
            memory: Mutex::new(vec![0, 0, 7, 8]),
        });
        // The host took frame 0 in an earlier run; the kernel writes frame 1.
        let pump = Arc::new(RingPump::new(layout(false), "out".into(), 1));
        pump.frames.bind(&queue.path().to_string_lossy());
        let thread = {
            let (ring, pump) = (Arc::clone(&ring), Arc::clone(&pump));
            std::thread::spawn(move || pump.run(ring.as_ref()))
        };
        ring.registers.lock().expect("registers")[1] = 2;
        pump.deliver_all(ring.as_ref()).expect("deliver");
        let mut slot = [0u8; 2];
        assert!(queue.pop_into(&mut slot));
        assert_eq!(slot, [7, 0]);
        assert!(queue.pop_into(&mut slot));
        assert_eq!(slot, [8, 0]);
        assert_eq!(ring.registers.lock().expect("registers")[0], 2);
        pump.frames.stop();
        thread.join().expect("join").expect("pump");
    }
}
