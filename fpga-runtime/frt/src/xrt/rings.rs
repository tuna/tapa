//! Rings on XRT (see `crate::ring`).
//!
//! The kernel is opened for exclusive access, so the host reads and writes
//! its registers during a run, and each ring is a buffer object in its
//! argument's bank, mapped into the host, whose slots are synced one frame
//! at a time.
//!
//! XRT writes every argument register when it starts a run, at some point
//! after `xrtRunStart` returns, the host's counts among them: the device
//! sets each to the count its pump last wrote, and the ring follows its
//! host count only forward, so a frame the pump announces meanwhile stands.
//! A pump announces nothing before the kernel holds its ring's address,
//! which XRT writes then too; the host's count stays in its register when
//! the device closes, for the next process to continue.

use std::ffi::c_int;
use std::sync::Arc;
use std::thread::JoinHandle;

use frt_cosim::metadata::{ArgKind, ArgSpec};

use super::api::{Api, Handle, SYNC_FROM_DEVICE, SYNC_TO_DEVICE};
use super::device::{check, non_null};
use crate::error::{FrtError, Result};
use crate::host_frames::{FrameQueue, POLL};
use crate::ring::{RingAccess, RingLayout, RingPump};

/// An XRT handle the pump thread shares with the device.
#[derive(Clone, Copy)]
struct Shared(Handle);

// SAFETY: XRT handles are reference-counted objects inside XRT, callable
// from any thread; the device stops and joins the pumps before closing them.
unsafe impl Send for Shared {}
// SAFETY: as above.
unsafe impl Sync for Shared {}

/// One ring's buffer, as the pump reaches it.
struct Access {
    api: &'static Api,
    kernel: Shared,
    bo: Shared,
    /// The buffer, mapped into the host.
    map: usize,
    bytes: usize,
    /// The register of the ring's argument, and the buffer's address.
    base: u32,
    address: u64,
}

impl Access {
    /// Waits until the kernel's argument holds the ring's address; false
    /// once the device stops the pump.
    fn wait_for_base(&self, frames: &FrameQueue) -> Result<bool> {
        loop {
            let low = self.read_register(self.base)?;
            let high = self.read_register(self.base + 4)?;
            if ((u64::from(high) << 32) | u64::from(low)) == self.address {
                return Ok(true);
            }
            if frames.stopped() {
                return Ok(false);
            }
            std::thread::sleep(POLL);
        }
    }
}

impl RingAccess for Access {
    fn read_register(&self, offset: u32) -> Result<u32> {
        let mut value = 0;
        // SAFETY: live kernel opened for exclusive access; XRT writes one word.
        let rc = unsafe { (self.api.kernel_read_register)(self.kernel.0, offset, &raw mut value) };
        check(rc, &format!("read the kernel register at {offset:#x}"))?;
        Ok(value)
    }

    fn write_register(&self, offset: u32, value: u32) -> Result<()> {
        // SAFETY: live kernel opened for exclusive access.
        let rc = unsafe { (self.api.kernel_write_register)(self.kernel.0, offset, value) };
        check(rc, &format!("write the kernel register at {offset:#x}"))
    }

    fn write_ring(&self, at: usize, data: &[u8]) -> Result<()> {
        // SAFETY: XRT maps the whole ring, for the buffer's life; only the
        // pump writes it from the host.
        let ring = unsafe { std::slice::from_raw_parts_mut(self.map as *mut u8, self.bytes) };
        ring[at..at + data.len()].copy_from_slice(data);
        // SAFETY: live buffer handle; the range is in the buffer.
        let rc = unsafe { (self.api.bo_sync)(self.bo.0, SYNC_TO_DEVICE, data.len(), at) };
        check(rc, "write a frame to its ring")
    }

    fn read_ring(&self, at: usize, data: &mut [u8]) -> Result<()> {
        // SAFETY: live buffer handle; the range is in the buffer.
        let rc = unsafe { (self.api.bo_sync)(self.bo.0, SYNC_FROM_DEVICE, data.len(), at) };
        check(rc, "read a frame from its ring")?;
        // SAFETY: XRT maps the whole ring, for the buffer's life.
        let ring = unsafe { std::slice::from_raw_parts(self.map as *const u8, self.bytes) };
        data.copy_from_slice(&ring[at..at + data.len()]);
        Ok(())
    }
}

/// One ring of the kernel.
pub(super) struct XrtRing {
    pump: Arc<RingPump>,
    access: Arc<Access>,
    /// The kernel argument of the host's count.
    host_arg: c_int,
    thread: Option<JoinHandle<Result<()>>>,
}

/// The rings among `args`: each memory argument with every register of
/// `tapa_ir::ring_registers` beside it. Their registers are no argument
/// the host program sets.
pub(super) fn ring_args(args: &[ArgSpec]) -> Vec<(&ArgSpec, [&ArgSpec; 3])> {
    args.iter()
        .filter(|arg| matches!(arg.kind, ArgKind::Mmap { .. }))
        .filter_map(|arg| {
            let registers = tapa_ir::ring_registers(&arg.name)
                .map(|register| args.iter().find(|other| other.name == register.name));
            let [Some(host), Some(kernel), Some(shape)] = registers else {
                return None;
            };
            Some((arg, [host, kernel, shape]))
        })
        .collect()
}

/// Whether `arg` is a ring's register.
pub(super) fn is_ring_register(args: &[ArgSpec], arg: &ArgSpec) -> bool {
    ring_args(args)
        .iter()
        .any(|(_, registers)| registers.iter().any(|register| register.id == arg.id))
}

impl XrtRing {
    /// Opens the ring of `arg` on `kernel`: a buffer in the argument's bank,
    /// bound to the run, with the shape the kernel reports.
    pub(super) fn open(
        api: &'static Api,
        device: Handle,
        kernel: Handle,
        run: Handle,
        arg: &ArgSpec,
        registers: [&ArgSpec; 3],
    ) -> Result<Self> {
        let ArgKind::Mmap { data_width, .. } = arg.kind else {
            unreachable!("a ring is a memory argument");
        };
        let arg_index = |spec: &ArgSpec| {
            c_int::try_from(spec.id)
                .map_err(|e| FrtError::Xrt(format!("argument index {}: {e}", spec.id)))
        };
        let offset = |spec: &ArgSpec| {
            arg_index(spec).map(|id| {
                // SAFETY: live kernel handle; a valid argument index.
                unsafe { (api.kernel_arg_offset)(kernel, id) }
            })
        };
        let [host, kernel_frames, shape] = registers;
        let probe = Access {
            api,
            kernel: Shared(kernel),
            bo: Shared(std::ptr::null_mut()),
            map: 0,
            bytes: 0,
            base: 0,
            address: 0,
        };
        let shape_offset = offset(shape)?;
        let shape = u64::from(probe.read_register(shape_offset)?)
            | (u64::from(probe.read_register(shape_offset + 4)?) << 32);
        let layout = RingLayout {
            index: arg.id,
            input: false,
            element_bytes: (data_width / 8) as usize,
            frame: 0,
            slots: 0,
            host_frames: offset(host)?,
            kernel_frames: offset(kernel_frames)?,
        }
        .with_shape(shape)?;
        let bytes = layout.ring_bytes();
        // SAFETY: live kernel handle; a valid argument index.
        let group = unsafe { (api.kernel_arg_group_id)(kernel, arg_index(arg)?) };
        let group = u32::try_from(group)
            .map_err(|e| FrtError::Xrt(format!("the ring of `{}` has no bank: {e}", arg.name)))?;
        // SAFETY: live device handle.
        let bo = non_null(
            unsafe { (api.bo_alloc)(device, bytes, 0, group) },
            &format!("allocate the ring of `{}`", arg.name),
        )?;
        // SAFETY: live run and buffer handles; a buffer argument reads a
        // buffer handle.
        let rc = unsafe { (api.run_set_arg)(run, arg_index(arg)?, bo) };
        // SAFETY: live buffer handle.
        let map = unsafe { (api.bo_map)(bo) };
        if let Err(error) = check(rc, &format!("bind the ring of `{}`", arg.name))
            .and_then(|()| non_null(map, &format!("map the ring of `{}`", arg.name)))
        {
            // SAFETY: `bo` is no buffer anything else holds.
            unsafe { (api.bo_free)(bo) };
            return Err(error);
        }
        // The count left by an earlier process, which ran on the same kernel.
        let start = probe.read_register(layout.host_frames)?;
        Ok(Self {
            host_arg: arg_index(host)?,
            pump: Arc::new(RingPump::new(layout, arg.name.clone(), start)),
            access: Arc::new(Access {
                api,
                kernel: Shared(kernel),
                bo: Shared(bo),
                map: map as usize,
                bytes,
                base: offset(arg)?,
                // SAFETY: live buffer handle.
                address: unsafe { (api.bo_address)(bo) },
            }),
            thread: None,
        })
    }

    pub(super) fn bind(&self, path: &str) {
        self.pump.frames.bind(path);
    }

    pub(super) fn index(&self) -> u32 {
        self.pump.layout.index
    }

    pub(super) fn is_bound(&self) -> bool {
        self.pump.frames.is_bound()
    }

    /// Starts the pump, once.
    pub(super) fn start(&mut self) -> Result<()> {
        if self.thread.is_none() {
            let (pump, access) = (Arc::clone(&self.pump), Arc::clone(&self.access));
            self.thread = Some(
                std::thread::Builder::new()
                    .name(format!("ring-{}", pump.frames.name))
                    .spawn(move || {
                        if !access.wait_for_base(&pump.frames)? {
                            return Ok(());
                        }
                        pump.run(access.as_ref())
                    })?,
            );
        }
        Ok(())
    }

    /// Whether every output frame the kernel has written reached the host.
    pub(super) fn delivered(&self) -> Result<bool> {
        self.pump.delivered(self.access.as_ref())
    }

    /// Waits until [`Self::delivered`].
    pub(super) fn deliver_all(&self) -> Result<()> {
        self.pump.deliver_all(self.access.as_ref())
    }

    /// Why the pump stopped, if it did.
    pub(super) fn failure(&mut self) -> Option<FrtError> {
        if !self.thread.as_ref().is_some_and(JoinHandle::is_finished) {
            return None;
        }
        Some(match self.thread.take()?.join() {
            Ok(Err(error)) => error,
            Ok(Ok(())) => FrtError::Xrt(format!("the ring of `{}` stopped", self.pump.frames.name)),
            Err(_panic) => {
                FrtError::Xrt(format!("the ring of `{}` panicked", self.pump.frames.name))
            }
        })
    }

    /// Stops the pump, leaves its count in the host's register, where XRT
    /// may have left an older one, and frees the ring.
    pub(super) fn close(mut self) {
        self.pump.frames.stop();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let _ = self
            .access
            .write_register(self.pump.layout.host_frames, self.pump.host_frames());
        // SAFETY: live buffer handle no thread uses any more.
        unsafe { (self.access.api.bo_free)(self.access.bo.0) };
    }
}

/// Starts `run` with each ring's host count set to the one its pump last
/// wrote, which the ring takes for no change.
pub(super) fn start_run(api: &Api, run: Handle, rings: &[XrtRing]) -> Result<()> {
    for ring in rings {
        // SAFETY: live run handle; XRT reads a 32-bit word.
        let rc = unsafe { (api.run_set_arg)(run, ring.host_arg, ring.pump.host_frames()) };
        check(rc, "set a ring's host count")?;
    }
    // SAFETY: live run handle with every argument set.
    check(unsafe { (api.run_start)(run) }, "start the kernel")
}
