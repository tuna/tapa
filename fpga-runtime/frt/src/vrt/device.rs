//! Hardware execution of one SLASH VBIN through vrtd.

use super::api::{
    api, Api, BarFile, VrtdBuffer, ALLOC_DIR_BIDIRECTIONAL, ALLOC_DIR_DEVICE_TO_HOST,
    ALLOC_DIR_HOST_TO_DEVICE, ALLOC_TYPE_DDR, ALLOC_TYPE_HBM, ALLOC_TYPE_HBM_VNOC,
    CLOCK_REGION_USER,
};
use super::vbin::{register_words, Arg, ArgKind, Kernel, MemoryTarget, Platform, Vbin};
use crate::device::{BufferAccess, Device};
use crate::error::{FrtError, Result};
use frt_shm::env_non_empty;
use std::collections::BTreeMap;
use std::ffi::{c_int, c_ulong, CString};
use std::path::Path;
use std::ptr;
use std::time::{Duration, Instant};

const VRTD_SOCKET: &str = "/run/vrtd.sock";
const PROGRAM_SETTLE: Duration = Duration::from_secs(1);
const FINISH_TIMEOUT: Duration = Duration::from_secs(60);
const FINISH_POLL: Duration = Duration::from_millis(1);
const CTRL_OFFSET: u32 = 0;
const CTRL_START: u32 = 1;
const CTRL_DONE: u32 = 1 << 1;

// Linux dma-buf uapi: _IOW('b', 0, struct dma_buf_sync { __u64 flags; }).
// x86_64 and aarch64 both use the generic _IOC layout; this is the same ioctl
// issued by the pinned inline slash_bar_file_start/end_{read,write} helpers.
const DMA_BUF_IOCTL_SYNC: c_ulong = 0x4008_6200;
const DMA_BUF_SYNC_START_READ: u64 = 1;
const DMA_BUF_SYNC_START_WRITE: u64 = 2;
const DMA_BUF_SYNC_END_READ: u64 = 1 | (1 << 2);
const DMA_BUF_SYNC_END_WRITE: u64 = 2 | (1 << 2);

#[repr(C)]
struct DmaBufSync {
    flags: u64,
}

#[derive(Clone, Copy)]
enum ExecutionState {
    Idle,
    Running(Instant),
    Finished,
}

struct BufferBinding {
    ptr: *mut u8,
    bytes: usize,
    access: BufferAccess,
    handle: *mut VrtdBuffer,
    phys_addr: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Allocation {
    kind: u32,
    direction: u32,
    region: u64,
}

fn allocation(target: MemoryTarget, access: BufferAccess) -> Allocation {
    let (kind, region) = match target {
        MemoryTarget::Ddr => (ALLOC_TYPE_DDR, 0),
        MemoryTarget::Hbm(region) => (ALLOC_TYPE_HBM, u64::from(region)),
        MemoryTarget::HbmVnoc => (ALLOC_TYPE_HBM_VNOC, 0),
    };
    let direction = match access {
        BufferAccess::ReadOnly => ALLOC_DIR_HOST_TO_DEVICE,
        BufferAccess::WriteOnly => ALLOC_DIR_DEVICE_TO_HOST,
        BufferAccess::PlaceHolder | BufferAccess::ReadWrite => ALLOC_DIR_BIDIRECTIONAL,
    };
    Allocation {
        kind,
        direction,
        region,
    }
}

pub struct VrtDevice {
    api: &'static Api,
    fd: c_int,
    device_index: u32,
    kernel: Kernel,
    bar: Option<BarFile>,
    scalars: BTreeMap<u32, Vec<u8>>,
    buffers: BTreeMap<u32, BufferBinding>,
    execution_state: ExecutionState,
    load_ns: u64,
    compute_ns: u64,
    store_ns: u64,
}

// SAFETY: a VrtDevice is moved, never shared: all operations require &mut self
// and all native handles are exclusively owned. The host pointers obey the
// same caller-lifetime contract as XRT's raw buffer bindings.
unsafe impl Send for VrtDevice {}

fn check(rc: c_int, action: &str) -> Result<()> {
    if rc == 0 {
        return Ok(());
    }
    let name = match rc {
        1 => "BAD_LIB_CALL",
        2 => "BAD_CONN",
        3 => "BAD_REQUEST",
        4 => "INVALID_ARGUMENT",
        5 => "NOEXIST",
        6 => "INTERNAL_ERROR",
        7 => "AUTH_ERROR",
        8 => "BUSY",
        _ => "UNKNOWN",
    };
    Err(FrtError::Vrt(format!(
        "cannot {action} (vrtd returned {name}, code {rc})"
    )))
}

fn cstring(value: &str, what: &str) -> Result<CString> {
    CString::new(value).map_err(|error| FrtError::Vrt(format!("invalid {what}: {error}")))
}

fn path_cstring(path: &Path) -> Result<CString> {
    let text = path
        .to_str()
        .ok_or_else(|| FrtError::Vrt(format!("non-UTF-8 path `{}`", path.display())))?;
    cstring(text, "VBIN member path")
}

fn elapsed_ns(since: Instant) -> u64 {
    u64::try_from(since.elapsed().as_nanos()).unwrap_or(u64::MAX)
}

/// Match libvrtd++'s board-level lookup: strip an optional PF function and
/// prepend the default domain to a short `BB:DD` address.
fn normalize_bdf(bdf: &str) -> String {
    let board = bdf.rsplit_once('.').map_or(bdf, |(board, _function)| board);
    if board.matches(':').count() == 1 {
        format!("0000:{board}")
    } else {
        board.to_owned()
    }
}

impl VrtDevice {
    pub fn open(vbin: &Vbin) -> Result<Self> {
        if vbin.platform != Platform::Hardware {
            return Err(FrtError::Vrt(
                "VrtDevice requires a Platform=Hardware VBIN".into(),
            ));
        }
        if vbin.pdi_paths.is_empty() {
            return Err(FrtError::Vrt(
                "hardware VBIN contains no PDI to program".into(),
            ));
        }
        let requested_bdf = env_non_empty(frt_shm::env::FRT_VRT_BDF).ok_or_else(|| {
            FrtError::Vrt(format!(
                "set --vrt_bdf or {} before opening a hardware VBIN",
                frt_shm::env::FRT_VRT_BDF
            ))
        })?;
        let bdf = normalize_bdf(&requested_bdf);
        if bdf != requested_bdf {
            tracing::warn!(requested = %requested_bdf, normalized = %bdf, "normalized VRT board BDF");
        }
        let bdf = cstring(&bdf, "VRT BDF")?;
        let api = api()?;
        let socket = cstring(VRTD_SOCKET, "vrtd socket path")?;
        // SAFETY: socket is a live C string; the returned descriptor is owned
        // by the VrtDevice constructed immediately below.
        let fd = unsafe { (api.connect)(socket.as_ptr()) };
        if fd < 0 {
            return Err(FrtError::Vrt(format!(
                "cannot connect to {VRTD_SOCKET}: {}",
                std::io::Error::last_os_error()
            )));
        }
        let mut this = Self {
            api,
            fd,
            device_index: 0,
            kernel: vbin.kernel.clone(),
            bar: None,
            scalars: BTreeMap::new(),
            buffers: BTreeMap::new(),
            execution_state: ExecutionState::Idle,
            load_ns: 0,
            compute_ns: 0,
            store_ns: 0,
        };
        let mut device_index = 0;
        // SAFETY: live socket and C string; device_index points to writable
        // storage retained until the call returns.
        let rc = unsafe { (api.get_device_by_bdf)(fd, bdf.as_ptr(), &raw mut device_index) };
        check(rc, "select the requested VRT board")?;
        this.device_index = device_index;

        for pdi in &vbin.pdi_paths {
            let path = path_cstring(pdi)?;
            tracing::info!(path = %pdi.display(), device = device_index, "programming VRT PDI");
            // SAFETY: live socket and path; vrtd closes the file it opens.
            let rc = unsafe { (api.design_write_file)(fd, device_index, path.as_ptr()) };
            check(rc, &format!("program PDI `{}`", pdi.display()))?;
        }
        std::thread::sleep(PROGRAM_SETTLE);
        this.program_clock(vbin.clock_frequency_hz)?;
        this.bar = Some(this.open_bar()?);
        Ok(this)
    }

    fn program_clock(&self, recorded_hz: u64) -> Result<()> {
        let recorded_hz = u32::try_from(recorded_hz).map_err(|error| {
            FrtError::Vrt(format!("recorded VBIN clock cannot be programmed: {error}"))
        })?;
        let mut set_achieved_hz = 0;
        // SAFETY: live socket and writable output pointer.
        let rc = unsafe {
            (self.api.clock_set_rate)(
                self.fd,
                self.device_index,
                CLOCK_REGION_USER,
                recorded_hz,
                &raw mut set_achieved_hz,
            )
        };
        check(rc, "program the recorded USER clock")?;
        let mut achieved_hz = 0;
        // SAFETY: live socket and writable output pointer.
        let rc = unsafe {
            (self.api.clock_get_rate)(
                self.fd,
                self.device_index,
                CLOCK_REGION_USER,
                &raw mut achieved_hz,
            )
        };
        check(rc, "read back the USER clock")?;
        tracing::info!(recorded_hz, achieved_hz, "VRT USER clock programmed");
        if set_achieved_hz != achieved_hz {
            tracing::debug!(
                set_achieved_hz,
                achieved_hz,
                "vrtd clock set/get rates differ"
            );
        }
        Ok(())
    }

    fn open_bar(&self) -> Result<BarFile> {
        let mut bar = BarFile {
            map: ptr::null_mut(),
            len: 0,
            fd: -1,
            mock: 0,
            mock_path: ptr::null_mut(),
        };
        // SAFETY: live socket and a correctly laid-out output BarFile.
        let rc = unsafe { (self.api.open_bar_file)(self.fd, self.device_index, 0, &raw mut bar) };
        check(rc, "open BAR0")?;
        if bar.map.is_null() || bar.len < 4 {
            // SAFETY: open returned success, so this is its one owned mapping.
            unsafe { (self.api.close_bar_file)(&raw mut bar) };
            return Err(FrtError::Vrt(
                "vrtd returned an invalid BAR0 mapping".into(),
            ));
        }
        Ok(bar)
    }

    fn ensure_not_running(&self, action: &str) -> Result<()> {
        if matches!(self.execution_state, ExecutionState::Running(_)) {
            Err(FrtError::Vrt(format!(
                "cannot {action} while the VRT kernel is running"
            )))
        } else {
            Ok(())
        }
    }

    fn bar_sync(bar: &BarFile, flags: u64) -> Result<()> {
        if bar.mock != 0 {
            return Ok(());
        }
        let sync = DmaBufSync { flags };
        // SAFETY: fd belongs to the open dma-buf BAR and sync has the exact
        // pinned uapi layout. The ioctl borrows sync only for this call.
        let rc = unsafe { libc::ioctl(bar.fd, DMA_BUF_IOCTL_SYNC, &sync) };
        if rc == -1 {
            Err(FrtError::Vrt(format!(
                "BAR dma-buf synchronization failed: {}",
                std::io::Error::last_os_error()
            )))
        } else {
            Ok(())
        }
    }

    fn register_ptr(bar: &BarFile, absolute_address: u64) -> Result<*mut u32> {
        if !absolute_address.is_multiple_of(4) {
            return Err(FrtError::Vrt(format!(
                "BAR address {absolute_address:#x} is not 32-bit aligned"
            )));
        }
        let bar_len = u64::try_from(bar.len)
            .map_err(|error| FrtError::Vrt(format!("invalid BAR length: {error}")))?;
        if bar_len == 0 || bar.map.is_null() {
            return Err(FrtError::Vrt("BAR0 is not mapped".into()));
        }
        let offset = absolute_address % bar_len;
        if offset.checked_add(4).is_none_or(|end| end > bar_len) {
            return Err(FrtError::Vrt(format!(
                "32-bit BAR access at {absolute_address:#x} exceeds the mapping"
            )));
        }
        let offset = usize::try_from(offset)
            .map_err(|error| FrtError::Vrt(format!("invalid BAR offset: {error}")))?;
        // SAFETY: the modulo and end check put all four bytes inside the live
        // mapping; both the mapping and address are 32-bit aligned.
        Ok(unsafe { bar.map.cast::<u8>().add(offset).cast::<u32>() })
    }

    fn read_register(&self, offset: u32) -> Result<u32> {
        let bar = self
            .bar
            .as_ref()
            .ok_or_else(|| FrtError::Vrt("BAR0 is closed".into()))?;
        let absolute = self
            .kernel
            .base_address
            .checked_add(u64::from(offset))
            .ok_or_else(|| FrtError::Vrt("kernel register address overflow".into()))?;
        let pointer = Self::register_ptr(bar, absolute)?;
        Self::bar_sync(bar, DMA_BUF_SYNC_START_READ)?;
        // SAFETY: pointer is aligned, in bounds, and bracketed for a single
        // volatile MMIO read; &mut VrtDevice prevents concurrent BAR access.
        let value = unsafe { ptr::read_volatile(pointer) };
        Self::bar_sync(bar, DMA_BUF_SYNC_END_READ)?;
        Ok(value)
    }

    fn write_register(&self, offset: u32, value: u32) -> Result<()> {
        let bar = self
            .bar
            .as_ref()
            .ok_or_else(|| FrtError::Vrt("BAR0 is closed".into()))?;
        let absolute = self
            .kernel
            .base_address
            .checked_add(u64::from(offset))
            .ok_or_else(|| FrtError::Vrt("kernel register address overflow".into()))?;
        let pointer = Self::register_ptr(bar, absolute)?;
        Self::bar_sync(bar, DMA_BUF_SYNC_START_WRITE)?;
        // SAFETY: pointer is aligned, in bounds, and bracketed for a single
        // volatile MMIO write; &mut VrtDevice prevents concurrent BAR access.
        unsafe { ptr::write_volatile(pointer, value) };
        Self::bar_sync(bar, DMA_BUF_SYNC_END_WRITE)
    }

    fn write_value(&self, arg: &Arg, value: &[u8]) -> Result<()> {
        for (offset, word) in register_words(arg.offset, arg.range, value)? {
            self.write_register(offset, word)?;
        }
        Ok(())
    }

    fn close_buffer(api: &Api, index: u32, handle: *mut VrtdBuffer) {
        if handle.is_null() {
            return;
        }
        // SAFETY: handle was returned by buffer_open and is consumed exactly
        // once here; libvrtd destroys it even when the daemon close fails.
        let rc = unsafe { (api.buffer_close)(handle) };
        if rc != 0 {
            tracing::warn!(argument = index, code = rc, "failed to close a VRT buffer");
        }
    }

    fn release_native(&mut self) {
        for (index, binding) in &mut self.buffers {
            Self::close_buffer(
                self.api,
                *index,
                std::mem::replace(&mut binding.handle, ptr::null_mut()),
            );
        }
        if let Some(mut bar) = self.bar.take() {
            // SAFETY: the BAR is open, no register access is active, and this
            // is its one close. Buffers were released first.
            unsafe { (self.api.close_bar_file)(&raw mut bar) };
        }
        if self.fd >= 0 {
            // SAFETY: this is the owned vrtd connection, closed once after all
            // dependent native resources.
            let rc = unsafe { libc::close(self.fd) };
            if rc != 0 {
                tracing::warn!(
                    "failed to close vrtd socket: {}",
                    std::io::Error::last_os_error()
                );
            }
            self.fd = -1;
        }
    }

    fn abandon_native_for_running_kernel(&mut self) {
        // Deliberately leak the socket, BAR and buffers. Closing the socket
        // would make vrtd auto-free allocations still addressed by hardware.
        self.fd = -1;
        self.bar = None;
        for binding in self.buffers.values_mut() {
            binding.handle = ptr::null_mut();
        }
    }
}

impl Drop for VrtDevice {
    fn drop(&mut self) {
        if matches!(self.execution_state, ExecutionState::Running(_)) {
            if let Err(error) = self.finish() {
                tracing::warn!(%error, "retaining VRT resources because a running kernel cannot be cancelled");
                self.abandon_native_for_running_kernel();
                return;
            }
        }
        self.release_native();
    }
}

impl Device for VrtDevice {
    fn set_scalar_arg(&mut self, index: u32, value: &[u8]) -> Result<()> {
        self.ensure_not_running("change an argument")?;
        let arg = self.kernel.arg(index)?.clone();
        if arg.kind != ArgKind::Scalar {
            return Err(FrtError::Vrt(format!(
                "argument {index} (`{}`) is not a scalar",
                arg.name
            )));
        }
        // Validate width and offset before changing the staged binding.
        register_words(arg.offset, arg.range, value)?;
        self.scalars.insert(index, value.to_vec());
        Ok(())
    }

    fn set_buffer_arg(
        &mut self,
        index: u32,
        ptr: *mut u8,
        bytes: usize,
        access: BufferAccess,
    ) -> Result<()> {
        self.ensure_not_running("change an argument")?;
        if bytes == 0 {
            return Err(FrtError::Vrt(format!(
                "zero-length buffer argument {index}"
            )));
        }
        if ptr.is_null() {
            return Err(FrtError::Vrt(format!(
                "null host pointer for buffer argument {index}"
            )));
        }
        let arg = self.kernel.arg(index)?.clone();
        if arg.kind != ArgKind::Buffer || arg.range != 64 {
            return Err(FrtError::Vrt(format!(
                "argument {index} (`{}`) is not a 64-bit buffer pointer",
                arg.name
            )));
        }
        let target = self.kernel.memory_target(&arg)?;
        let allocation = allocation(target, access);
        let size = u64::try_from(bytes)
            .map_err(|error| FrtError::Vrt(format!("buffer argument {index}: {error}")))?;
        let mut handle = ptr::null_mut();
        // SAFETY: live vrtd connection and valid output pointer. No host
        // pointer crosses this call; libvrtd allocates its own DMA staging map.
        let rc = unsafe {
            (self.api.buffer_open)(
                self.fd,
                self.device_index,
                allocation.kind,
                allocation.direction,
                allocation.region,
                size,
                &raw mut handle,
            )
        };
        check(rc, &format!("allocate buffer argument {index}"))?;
        if handle.is_null() {
            return Err(FrtError::Vrt(format!(
                "vrtd returned no buffer for argument {index}"
            )));
        }
        // SAFETY: handle is a live pinned C struct until buffer_close.
        let allocated_size = unsafe { (*handle).size };
        // SAFETY: as above; each field is copied from the live C struct.
        let phys_addr = unsafe { (*handle).phys_addr };
        // SAFETY: as above; the staging pointer remains owned by the handle.
        let staging = unsafe { (*handle).buf };
        if staging.is_null() || allocated_size < size {
            Self::close_buffer(self.api, index, handle);
            return Err(FrtError::Vrt(format!(
                "vrtd returned an invalid staging buffer for argument {index}"
            )));
        }
        let old = self.buffers.insert(
            index,
            BufferBinding {
                ptr,
                bytes,
                access,
                handle,
                phys_addr,
            },
        );
        if let Some(old) = old {
            Self::close_buffer(self.api, index, old.handle);
        }
        Ok(())
    }

    fn set_stream_arg(&mut self, index: u32, _shm_path: &str) -> Result<()> {
        Err(FrtError::Vrt(format!(
            "argument {index}: host streams are unsupported by the VRT backend"
        )))
    }

    fn write_to_device(&mut self) -> Result<()> {
        self.ensure_not_running("upload buffers")?;
        let start = Instant::now();
        for (index, binding) in &self.buffers {
            if !binding.access.loads_from_host() {
                continue;
            }
            // SAFETY: the live C handle owns its staging pointer.
            let staging = unsafe { (*binding.handle).buf.cast::<u8>() };
            // SAFETY: the caller keeps ptr valid for bytes, and open validated
            // the distinct staging mapping to be at least that large.
            unsafe { ptr::copy_nonoverlapping(binding.ptr, staging, binding.bytes) };
            let size = u64::try_from(binding.bytes)
                .map_err(|error| FrtError::Vrt(format!("buffer argument {index}: {error}")))?;
            // SAFETY: live buffer; offset zero and size are within its map.
            let rc = unsafe { (self.api.buffer_sync_to_device)(binding.handle, 0, size) };
            check(rc, &format!("write buffer argument {index} to the device"))?;
        }
        self.load_ns = elapsed_ns(start);
        Ok(())
    }

    fn read_from_device(&mut self) -> Result<()> {
        if matches!(self.execution_state, ExecutionState::Running(_)) {
            self.finish()?;
        }
        let start = Instant::now();
        for (index, binding) in &self.buffers {
            if !binding.access.stores_to_host() {
                continue;
            }
            let size = u64::try_from(binding.bytes)
                .map_err(|error| FrtError::Vrt(format!("buffer argument {index}: {error}")))?;
            // SAFETY: live buffer; offset zero and size are within its map.
            let rc = unsafe { (self.api.buffer_sync_from_device)(binding.handle, 0, size) };
            check(rc, &format!("read buffer argument {index} from the device"))?;
            // SAFETY: the live C handle owns its staging pointer.
            let staging = unsafe { (*binding.handle).buf.cast::<u8>() };
            // SAFETY: the caller keeps ptr writable for bytes, and the device
            // staging map is a distinct region of at least that size.
            unsafe { ptr::copy_nonoverlapping(staging, binding.ptr, binding.bytes) };
        }
        self.store_ns = elapsed_ns(start);
        Ok(())
    }

    fn exec(&mut self) -> Result<()> {
        self.ensure_not_running("start the kernel again")?;
        for arg in self.kernel.args.clone() {
            if !arg.writable {
                continue;
            }
            let value = match arg.kind {
                ArgKind::Scalar => self.scalars.get(&arg.index).cloned().ok_or_else(|| {
                    FrtError::Vrt(format!(
                        "unbound scalar argument {} (`{}`)",
                        arg.index, arg.name
                    ))
                })?,
                ArgKind::Buffer => self
                    .buffers
                    .get(&arg.index)
                    .map(|binding| binding.phys_addr.to_le_bytes().to_vec())
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
        self.write_register(CTRL_OFFSET, CTRL_START)?;
        self.compute_ns = 0;
        self.execution_state = ExecutionState::Running(started);
        Ok(())
    }

    fn finish(&mut self) -> Result<()> {
        if !matches!(self.execution_state, ExecutionState::Running(_)) {
            return Ok(());
        }
        tracing::debug!(
            timeout_seconds = FINISH_TIMEOUT.as_secs(),
            "waiting for VRT kernel"
        );
        let wait_started = Instant::now();
        while !self.is_finished()? {
            if wait_started.elapsed() >= FINISH_TIMEOUT {
                let error = FrtError::Vrt(format!(
                    "VRT kernel did not finish within {}s and cannot be cancelled",
                    FINISH_TIMEOUT.as_secs()
                ));
                tracing::warn!(%error, "bounded VRT wait expired");
                return Err(error);
            }
            std::thread::sleep(FINISH_POLL);
        }
        Ok(())
    }

    fn kill(&mut self) -> Result<()> {
        Err(FrtError::Vrt(
            "VRT has no kernel cancellation; finish is bounded and a hung kernel's buffers are retained on close"
                .into(),
        ))
    }

    fn is_finished(&mut self) -> Result<bool> {
        let ExecutionState::Running(started) = self.execution_state else {
            return Ok(matches!(self.execution_state, ExecutionState::Finished));
        };
        if self.read_register(CTRL_OFFSET)? & CTRL_DONE == 0 {
            return Ok(false);
        }
        self.compute_ns = elapsed_ns(started);
        self.execution_state = ExecutionState::Finished;
        Ok(true)
    }

    fn load_ns(&self) -> u64 {
        self.load_ns
    }

    fn compute_ns(&self) -> u64 {
        self.compute_ns
    }

    fn store_ns(&self) -> u64 {
        self.store_ns
    }
}

#[cfg(test)]
mod tests {
    use super::{allocation, normalize_bdf, Allocation};
    use crate::device::BufferAccess;
    use crate::vrt::api::{
        ALLOC_DIR_DEVICE_TO_HOST, ALLOC_DIR_HOST_TO_DEVICE, ALLOC_TYPE_DDR, ALLOC_TYPE_HBM,
    };
    use crate::vrt::vbin::MemoryTarget;

    #[test]
    fn board_bdf_lookup_matches_vrtd_normalization() {
        assert_eq!(normalize_bdf("a1:00"), "0000:a1:00");
        assert_eq!(normalize_bdf("0000:a1:00.0"), "0000:a1:00");
        assert_eq!(normalize_bdf("0000:a1:00"), "0000:a1:00");
    }

    #[test]
    fn connections_choose_allocation_type_region_and_direction() {
        assert_eq!(
            allocation(MemoryTarget::Hbm(17), BufferAccess::ReadOnly),
            Allocation {
                kind: ALLOC_TYPE_HBM,
                direction: ALLOC_DIR_HOST_TO_DEVICE,
                region: 17,
            }
        );
        assert_eq!(
            allocation(MemoryTarget::Ddr, BufferAccess::WriteOnly),
            Allocation {
                kind: ALLOC_TYPE_DDR,
                direction: ALLOC_DIR_DEVICE_TO_HOST,
                region: 0,
            }
        );
    }
}
