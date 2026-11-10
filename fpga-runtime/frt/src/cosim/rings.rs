//! Rings in cosimulation (see `crate::ring`).
//!
//! Each ring is the shared segment of its memory argument, which the
//! simulator reads and writes as the run goes, and the host reaches the
//! kernel's registers through the testbench (`frt_cosim::control`). Every
//! run is a new simulation, so its pumps start from zero counts.

use std::sync::Arc;
use std::thread::JoinHandle;

use frt_cosim::context::CosimContext;
use frt_cosim::control::Control;
use frt_cosim::metadata::{ArgKind, KernelSpec};

use crate::error::{FrtError, Result};
use crate::ring::{RingAccess, RingLayout, RingPump};

/// A ring's segment and the testbench, as a pump reaches them.
struct Access {
    control: Arc<Control>,
    /// The ring's segment, mapped into the host.
    memory: usize,
    bytes: usize,
}

impl RingAccess for Access {
    fn read_register(&self, offset: u32) -> Result<u32> {
        Ok(self.control.read(offset)?)
    }

    fn write_register(&self, offset: u32, value: u32) -> Result<()> {
        Ok(self.control.write(offset, value)?)
    }

    fn write_ring(&self, at: usize, data: &[u8]) -> Result<()> {
        // SAFETY: the segment holds `bytes` bytes, outlives the pump, and
        // only the pump writes it from the host.
        let ring = unsafe { std::slice::from_raw_parts_mut(self.memory as *mut u8, self.bytes) };
        ring[at..at + data.len()].copy_from_slice(data);
        Ok(())
    }

    fn read_ring(&self, at: usize, data: &mut [u8]) -> Result<()> {
        // SAFETY: the segment holds `bytes` bytes and outlives the pump.
        let ring = unsafe { std::slice::from_raw_parts(self.memory as *const u8, self.bytes) };
        data.copy_from_slice(&ring[at..at + data.len()]);
        Ok(())
    }
}

/// A ring's pump while a simulation runs.
struct Running {
    pump: Arc<RingPump>,
    access: Arc<Access>,
    thread: JoinHandle<Result<()>>,
}

/// One ring of the kernel, and its pump while a simulation runs.
pub(super) struct CosimRing {
    name: String,
    layout: RingLayout,
    /// The host queue bound to it.
    path: Option<String>,
    running: Option<Running>,
}

impl CosimRing {
    /// The rings of `spec`, each at its memory argument, with its registers
    /// at the offsets the control slave gives them.
    pub(super) fn of(spec: &KernelSpec) -> Result<Vec<Self>> {
        spec.rings
            .iter()
            .map(|ring| {
                let arg = spec
                    .args
                    .iter()
                    .find(|arg| arg.name == ring.port)
                    .ok_or_else(|| missing(&ring.port, "argument"))?;
                let ArgKind::Mmap { data_width, .. } = arg.kind else {
                    return Err(missing(&ring.port, "memory argument"));
                };
                let [host, kernel, _shape] = tapa_ir::ring_registers(&ring.port);
                let offset = |name: &str| {
                    spec.scalar_register_map
                        .get(&name.to_lowercase())
                        .copied()
                        .ok_or_else(|| missing(name, "control register"))
                };
                Ok(Self {
                    name: ring.port.clone(),
                    layout: RingLayout {
                        index: arg.id,
                        input: ring.input,
                        element_bytes: (data_width / 8) as usize,
                        frame: ring.frame,
                        slots: ring.slots,
                        host_frames: offset(&host.name)?,
                        kernel_frames: offset(&kernel.name)?,
                    },
                    path: None,
                    running: None,
                })
            })
            .collect()
    }

    pub(super) fn index(&self) -> u32 {
        self.layout.index
    }

    pub(super) fn bind(&mut self, path: &str) {
        self.path = Some(path.to_owned());
    }

    /// Sizes the ring's segment, before the simulation opens it.
    pub(super) fn size(&self, ctx: &mut CosimContext) -> Result<()> {
        Ok(ctx.resize_buffer(&self.name, self.layout.ring_bytes())?)
    }

    /// Starts the pump of a simulation just spawned.
    pub(super) fn start(&mut self, ctx: &CosimContext) -> Result<()> {
        let path = self.path.as_deref().ok_or_else(|| {
            FrtError::MetadataParse(format!(
                "the stream argument {} (`{}`) was never bound",
                self.layout.index, self.name
            ))
        })?;
        let control = ctx
            .control
            .clone()
            .ok_or_else(|| missing(&self.name, "control queue"))?;
        let segment = ctx
            .buffers
            .get(&self.name)
            .ok_or_else(|| missing(&self.name, "segment"))?;
        let access = Arc::new(Access {
            control,
            memory: segment.shared_ptr() as usize,
            bytes: segment.len(),
        });
        let pump = Arc::new(RingPump::new(self.layout.clone(), self.name.clone(), 0));
        pump.frames.bind(path);
        let thread = {
            let (pump, access) = (Arc::clone(&pump), Arc::clone(&access));
            std::thread::Builder::new()
                .name(format!("ring-{}", self.name))
                .spawn(move || pump.run(access.as_ref()))?
        };
        self.running = Some(Running {
            pump,
            access,
            thread,
        });
        Ok(())
    }

    /// Whether the host has every output frame the kernel wrote.
    pub(super) fn delivered(&self) -> Result<bool> {
        match &self.running {
            Some(running) => running.pump.delivered(running.access.as_ref()),
            None => Ok(true),
        }
    }

    /// Why the pump stopped, if it did.
    pub(super) fn failure(&mut self) -> Option<FrtError> {
        if !self
            .running
            .as_ref()
            .is_some_and(|running| running.thread.is_finished())
        {
            return None;
        }
        Some(match self.running.take()?.thread.join() {
            Ok(Err(error)) => error,
            Ok(Ok(())) => FrtError::MetadataParse(format!("the ring of `{}` stopped", self.name)),
            Err(_panic) => FrtError::MetadataParse(format!("the ring of `{}` panicked", self.name)),
        })
    }

    /// Stops the pump once its simulation ended.
    pub(super) fn stop(&mut self) {
        if let Some(running) = self.running.take() {
            running.pump.frames.stop();
            let _ = running.thread.join();
        }
    }
}

fn missing(name: &str, what: &str) -> FrtError {
    FrtError::MetadataParse(format!("the ring `{name}` has no {what}"))
}
