//! An xclbin whose top runs on the AI Engine (`tapa link` of its `.zip`):
//! there is no PL kernel, so XRT's graph API drives the graph, each scalar
//! argument is a runtime parameter, and each stream argument moves frames
//! between the host's queue and the graph over GMIO, one transfer per
//! frame. Only XRT's embedded runtime implements GMIO.
//!
//! A pump per GMIO moves its frames with one transfer in flight, since XRT
//! waits only for all of a GMIO's posted transfers, and lives as long as
//! the device. A run ends once the graph has and each output has delivered
//! the frames its writer reports in `<graph>.frames_<GMIO>`. Each output
//! then still has a transfer posted for a frame that may come next, which
//! XRT cannot cancel: the device runs the graph once more with every
//! kernel's drain flag set, which fills it with a frame the pump discards,
//! so no transfer outlives the invoke, or the process. An input frame no run
//! consumed carries to the next run, as on a stream.

use std::collections::HashMap;
use std::ffi::{c_char, CString};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Instant;

use super::api::{
    api, gmio_api, Api, GmioApi, Handle, SYNC_AIE_TO_GMIO, SYNC_FROM_DEVICE, SYNC_GMIO_TO_AIE,
    SYNC_TO_DEVICE,
};
use super::device::{bootstrap_emulation_env_once, check, load_xclbin, non_null};
use super::metadata::{
    extract_platform_vbnv, parse_aie_graph, xclbin_target, AieGraph, AieTopArguments, GmioArgument,
};
use crate::device::{BufferAccess, Device};
use crate::error::{FrtError, Result};
use crate::host_frames::{FrameQueue, HostQueue, POLL};

/// An XRT handle the pumps share with the device.
#[derive(Clone, Copy)]
struct Shared(Handle);

// SAFETY: XRT handles are reference-counted objects inside XRT, callable
// from any thread; the device outlives every use (see `Drop`).
unsafe impl Send for Shared {}
// SAFETY: as above.
unsafe impl Sync for Shared {}

/// One GMIO and the host queue bound to it for the current run.
struct Pump {
    gmio: GmioArgument,
    name: CString,
    frames: FrameQueue,
    /// Output frames delivered so far, over every run.
    delivered: AtomicU64,
    /// The next output frame is a drain run's, to discard.
    drain: AtomicBool,
    /// An output waits for the next run before it posts a transfer.
    idle: AtomicBool,
}

pub struct XrtAieTopDevice {
    api: &'static Api,
    gmio: &'static GmioApi,
    device: Handle,
    graph: Handle,
    meta: AieGraph,
    scalars: HashMap<u32, Vec<u8>>,
    pumps: Vec<Arc<Pump>>,
    threads: Vec<JoinHandle<Result<()>>>,
    run: Option<JoinHandle<Result<u64>>>,
    finished: bool,
    compute_ns: u64,
}

// SAFETY: the device is used from one thread at a time; its XRT handles are
// reference-counted objects inside XRT.
unsafe impl Send for XrtAieTopDevice {}

impl XrtAieTopDevice {
    pub fn open(xclbin_path: &Path, bytes: &[u8], arguments: AieTopArguments) -> Result<Self> {
        let meta = parse_aie_graph(bytes)?.ok_or_else(|| {
            FrtError::MetadataParse(format!(
                "the xclbin of `{}` runs on the AI Engine but carries no TAPA graph",
                arguments.top
            ))
        })?;
        if meta.drains.is_empty() && arguments.gmios.iter().any(|gmio| !gmio.input) {
            return Err(FrtError::MetadataParse(format!(
                "the graph of `{}` writes a GMIO but its kernels take no drain flag; rebuild \
                 it with this version of TAPA",
                arguments.top
            )));
        }
        let platform = extract_platform_vbnv(bytes).unwrap_or_default();
        bootstrap_emulation_env_once(xclbin_target(bytes), &platform)?;
        let api = api()?;
        let gmio = gmio_api()?;
        let (device, uuid) = load_xclbin(api, xclbin_path)?;
        let name = CString::new(meta.name.clone())
            .map_err(|e| FrtError::Xrt(format!("graph name: {e}")))?;
        // SAFETY: live device, the UUID it reported, a valid C string.
        let graph = unsafe { (api.graph_open)(device, uuid.as_ptr(), name.as_ptr()) };
        let graph = match non_null(graph, &format!("open the AI Engine graph `{}`", meta.name)) {
            Ok(graph) => graph,
            Err(error) => {
                // SAFETY: live device handle with no open children.
                unsafe { (api.device_close)(device) };
                return Err(error);
            }
        };
        let pumps = arguments
            .gmios
            .into_iter()
            .map(|gmio| {
                let name = CString::new(gmio.name.clone())
                    .map_err(|e| FrtError::Xrt(format!("GMIO name: {e}")))?;
                let frames = FrameQueue::new(gmio.name.clone(), gmio.element_words as usize * 4);
                Ok(Arc::new(Pump {
                    gmio,
                    name,
                    frames,
                    delivered: AtomicU64::new(0),
                    drain: AtomicBool::new(false),
                    idle: AtomicBool::new(true),
                }))
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            api,
            gmio,
            device,
            graph,
            meta,
            scalars: HashMap::new(),
            pumps,
            threads: Vec::new(),
            run: None,
            finished: false,
            compute_ns: 0,
        })
    }

    /// Starts every pump, once each GMIO has a host queue.
    fn start_pumps(&mut self) -> Result<()> {
        if !self.threads.is_empty() {
            return Ok(());
        }
        for pump in &self.pumps {
            if !pump.frames.is_bound() {
                return Err(FrtError::Xrt(format!(
                    "the stream argument {} (`{}`) was never bound",
                    pump.gmio.index, pump.gmio.name
                )));
            }
        }
        for pump in &self.pumps {
            let (api, gmio, device) = (self.api, self.gmio, Shared(self.device));
            let pump = Arc::clone(pump);
            let thread = std::thread::Builder::new()
                .name(format!("gmio-{}", pump.gmio.name))
                .spawn(move || run_pump(api, gmio, device, &pump))?;
            self.threads.push(thread);
        }
        Ok(())
    }

    /// The first pump that stopped, with why.
    fn pump_failure(&mut self) -> Option<FrtError> {
        let index = self.threads.iter().position(JoinHandle::is_finished)?;
        let thread = self.threads.remove(index);
        Some(match thread.join() {
            Ok(Err(error)) => error,
            Ok(Ok(())) => FrtError::Xrt("a GMIO pump stopped".into()),
            Err(_panic) => FrtError::Xrt("a GMIO pump panicked".into()),
        })
    }
}

impl Device for XrtAieTopDevice {
    fn set_scalar_arg(&mut self, index: u32, value: &[u8]) -> Result<()> {
        self.scalars.insert(index, value.to_vec());
        Ok(())
    }

    fn set_buffer_arg(&mut self, index: u32, _: *mut u8, _: usize, _: BufferAccess) -> Result<()> {
        Err(FrtError::Xrt(format!(
            "argument {index} is memory, which a top on the AI Engine cannot take"
        )))
    }

    fn set_stream_arg(&mut self, index: u32, shm_path: &str) -> Result<()> {
        let pump = self
            .pumps
            .iter()
            .find(|pump| pump.gmio.index == index)
            .ok_or_else(|| {
                FrtError::Xrt(format!(
                    "argument {index} of a top on the AI Engine is not a stream"
                ))
            })?;
        pump.frames.bind(shm_path);
        Ok(())
    }

    fn write_to_device(&mut self) -> Result<()> {
        Ok(())
    }

    fn read_from_device(&mut self) -> Result<()> {
        Ok(())
    }

    fn exec(&mut self) -> Result<()> {
        self.start_pumps()?;
        let drains = self
            .meta
            .drains
            .iter()
            .map(|alias| {
                CString::new(alias.clone())
                    .map_err(|e| FrtError::Xrt(format!("drain flag name: {e}")))
            })
            .collect::<Result<Vec<_>>>()?;
        set_drains(self.api, Shared(self.graph), &drains, false)?;
        for rtp in &self.meta.rtps {
            let value = rtp.scalar_value(&self.scalars)?;
            let alias = CString::new(rtp.alias.clone())
                .map_err(|e| FrtError::Xrt(format!("runtime parameter name: {e}")))?;
            // SAFETY: live graph handle; `value` holds `value.len()` bytes.
            let rc = unsafe {
                (self.api.graph_update_rtp)(
                    self.graph,
                    alias.as_ptr(),
                    value.as_ptr().cast(),
                    value.len(),
                )
            };
            check(rc, &format!("update runtime parameter `{}`", rtp.alias))?;
        }
        // What each output had delivered before this run.
        let outputs: Vec<(Arc<Pump>, u64, CString)> = self
            .pumps
            .iter()
            .filter(|pump| !pump.gmio.input)
            .map(|pump| {
                let alias = format!("{}.frames_{}", self.meta.name, pump.gmio.name);
                let alias = CString::new(alias)
                    .map_err(|e| FrtError::Xrt(format!("frame count name: {e}")))?;
                let before = pump.delivered.load(Ordering::Acquire);
                Ok((Arc::clone(pump), before, alias))
            })
            .collect::<Result<_>>()?;
        for (pump, _, _) in &outputs {
            pump.idle.store(false, Ordering::Release);
        }
        let started = Instant::now();
        // SAFETY: live graph handle.
        let rc = unsafe { (self.api.graph_run)(self.graph, 1) };
        check(rc, "run the AI Engine graph")?;
        self.finished = false;
        let (api, gmio, graph) = (self.api, self.gmio, Shared(self.graph));
        self.run = Some(
            std::thread::Builder::new()
                .name("aie-run".into())
                .spawn(move || {
                    // The whole handle moves in, not only its pointer.
                    let graph = graph;
                    // SAFETY: live graph handle; 0 waits for the graph to finish.
                    let rc = unsafe { (api.graph_wait)(graph.0, 0) };
                    check(rc, "wait for the AI Engine graph")?;
                    for (pump, before, alias) in &outputs {
                        let before = *before;
                        let mut frames = [0u8; 4];
                        // SAFETY: live graph handle; the buffer holds 4 bytes.
                        let rc = unsafe {
                            (gmio.graph_read_rtp)(
                                graph.0,
                                alias.as_ptr(),
                                frames.as_mut_ptr().cast::<c_char>(),
                                frames.len(),
                            )
                        };
                        check(rc, &format!("read the frame count of `{}`", pump.gmio.name))?;
                        let frames = u64::from(u32::from_le_bytes(frames));
                        pump.frames.wait_for(|| {
                            Ok(pump.delivered.load(Ordering::Acquire) - before >= frames)
                        })?;
                    }
                    let elapsed = crate::elapsed_ns(started);
                    // The drain run fills the transfer each output left posted.
                    for (pump, _, _) in &outputs {
                        pump.drain.store(true, Ordering::Release);
                    }
                    set_drains(api, graph, &drains, true)?;
                    // SAFETY: live graph handle.
                    check(unsafe { (api.graph_run)(graph.0, 1) }, "run the drain run")?;
                    // SAFETY: live graph handle; 0 waits for the graph to finish.
                    let rc = unsafe { (api.graph_wait)(graph.0, 0) };
                    check(rc, "wait for the drain run")?;
                    for (pump, _, _) in &outputs {
                        pump.frames
                            .wait_for(|| Ok(pump.idle.load(Ordering::Acquire)))?;
                    }
                    Ok(elapsed)
                })?,
        );
        Ok(())
    }

    fn finish(&mut self) -> Result<()> {
        let Some(run) = self.run.take() else {
            return Ok(());
        };
        while !run.is_finished() {
            if let Some(error) = self.pump_failure() {
                for pump in &self.pumps {
                    pump.frames.stop();
                }
                return Err(error);
            }
            std::thread::sleep(POLL);
        }
        self.compute_ns = run
            .join()
            .map_err(|_panic| FrtError::Xrt("the AI Engine run panicked".into()))??;
        self.finished = true;
        Ok(())
    }

    fn kill(&mut self) -> Result<()> {
        for pump in &self.pumps {
            pump.frames.stop();
        }
        // SAFETY: live graph handle.
        let rc = unsafe { (self.api.graph_reset)(self.graph) };
        self.finished = true;
        check(rc, "reset the AI Engine graph")
    }

    fn is_finished(&mut self) -> Result<bool> {
        Ok(self.finished || self.run.as_ref().is_some_and(JoinHandle::is_finished))
    }

    fn compute_ns(&self) -> u64 {
        self.compute_ns
    }
}

impl Drop for XrtAieTopDevice {
    // A pump may still wait inside XRT on a transfer no run will fill, so
    // the device stays open under it for the rest of the process.
    fn drop(&mut self) {
        for pump in &self.pumps {
            pump.frames.stop();
        }
        // SAFETY: live graph handle, closed once.
        unsafe { (self.api.graph_close)(self.graph) };
        if self.threads.is_empty() {
            // SAFETY: live device handle with no open children.
            unsafe { (self.api.device_close)(self.device) };
        }
    }
}

/// Sets every kernel's drain flag.
fn set_drains(api: &Api, graph: Shared, drains: &[CString], on: bool) -> Result<()> {
    let value = i32::from(on).to_le_bytes();
    for alias in drains {
        // SAFETY: live graph handle; `value` holds 4 bytes.
        let rc = unsafe {
            (api.graph_update_rtp)(graph.0, alias.as_ptr(), value.as_ptr().cast(), value.len())
        };
        check(rc, "set a kernel's drain flag")?;
    }
    Ok(())
}

/// Moves one GMIO's frames until the device stops it.
fn run_pump(api: &Api, gmio: &GmioApi, device: Shared, pump: &Pump) -> Result<()> {
    let bytes = pump.gmio.transfer_words as usize * 4;
    // SAFETY: live device; default flags in the default memory group.
    let bo = non_null(
        unsafe { (api.bo_alloc)(device.0, bytes, 0, 0) },
        &format!("allocate the buffer of GMIO `{}`", pump.gmio.name),
    )?;
    // SAFETY: live buffer handle.
    let map = unsafe { (api.bo_map)(bo) }.cast::<u8>();
    if map.is_null() {
        return Err(FrtError::Xrt(format!(
            "cannot map the buffer of GMIO `{}`",
            pump.gmio.name
        )));
    }
    // SAFETY: XRT maps `bytes` bytes for the buffer's life.
    let frame = unsafe { std::slice::from_raw_parts_mut(map, bytes) };
    let mut host = HostQueue::default();
    let transfer = |direction| -> Result<()> {
        // SAFETY: live device and buffer; a valid C string.
        let rc =
            unsafe { (gmio.sync_bo_nb)(device.0, bo, pump.name.as_ptr(), direction, bytes, 0) };
        check(rc, &format!("post a transfer on GMIO `{}`", pump.gmio.name))?;
        // SAFETY: live device; a valid C string.
        let rc = unsafe { (gmio.wait)(device.0, pump.name.as_ptr()) };
        check(rc, &format!("wait for GMIO `{}`", pump.gmio.name))
    };
    let sync = |direction| {
        // SAFETY: live buffer handle; the range is the whole buffer.
        let rc = unsafe { (api.bo_sync)(bo, direction, bytes, 0) };
        check(rc, &format!("sync the buffer of GMIO `{}`", pump.gmio.name))
    };
    loop {
        if pump.gmio.input {
            if !host.fill(&pump.frames, frame)? {
                return Ok(());
            }
            sync(SYNC_TO_DEVICE)?;
            transfer(SYNC_GMIO_TO_AIE)?;
        } else {
            while pump.idle.load(Ordering::Acquire) {
                if pump.frames.stopped() {
                    return Ok(());
                }
                std::thread::sleep(POLL);
            }
            transfer(SYNC_AIE_TO_GMIO)?;
            if pump.drain.swap(false, Ordering::AcqRel) {
                pump.idle.store(true, Ordering::Release);
                continue;
            }
            sync(SYNC_FROM_DEVICE)?;
            if !host.drain(&pump.frames, frame)? {
                return Ok(());
            }
            pump.delivered.fetch_add(1, Ordering::AcqRel);
        }
    }
}
