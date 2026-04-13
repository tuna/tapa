use super::metadata::{
    extract_embedded_xml, extract_platform_vbnv, parse_embedded_xml, XrtArgKind, XrtMetadata,
};
use crate::device::{BufferAccess, Device, RuntimeArgCategory, RuntimeArgInfo};
use crate::error::{FrtError, Result};
use frt_cosim::runner::environ::xilinx_environ;
use opencl3::command_queue::{
    CommandQueue, CL_QUEUE_OUT_OF_ORDER_EXEC_MODE_ENABLE, CL_QUEUE_PROFILING_ENABLE,
};
use opencl3::context::Context;
use opencl3::device::{Device as OclDevice, CL_DEVICE_TYPE_ACCELERATOR, CL_DEVICE_TYPE_ALL};
use opencl3::event::Event;
use opencl3::kernel::{set_kernel_arg, Kernel};
use opencl3::memory::{Buffer, ClMem, CL_MEM_READ_WRITE, CL_MEM_USE_HOST_PTR};
use opencl3::platform::get_platforms;
use opencl3::program::Program;
use opencl3::types::{cl_device_id, cl_event, CL_BLOCKING};
use std::collections::HashMap;
use std::ffi::c_void;
use std::ffi::CStr;
use std::path::{Path, PathBuf};
use std::process::Command;

struct BufferBinding {
    ptr: *mut u8,
    bytes: usize,
    access: BufferAccess,
    buffer: Buffer<u8>,
}

pub struct XrtDevice {
    _meta: XrtMetadata,
    context: Context,
    queue: CommandQueue,
    kernel: Kernel,
    scalars: HashMap<u32, Vec<u8>>,
    buffers: HashMap<u32, BufferBinding>,
    load_events: Vec<Event>,
    compute_events: Vec<Event>,
    store_events: Vec<Event>,
    load_ns: u64,
    compute_ns: u64,
    store_ns: u64,
    finished: bool,
}

// SAFETY: XrtDevice is only accessed from a single thread at a time.
// The OpenCL types (Context, CommandQueue, Kernel) are internally
// reference-counted by the driver; Buffer<u8> wraps a cl_mem handle.
unsafe impl Send for XrtDevice {}

impl XrtDevice {
    pub fn open(xclbin_path: &Path) -> Result<Self> {
        let bytes = std::fs::read(xclbin_path)?;
        let xml = extract_embedded_xml(&bytes)?;
        let mut meta = parse_embedded_xml(&xml)?;
        // The XML <platform name="..."> attribute may be truncated in some
        // xclbin versions. The binary header has a 64-byte m_platformVBNV
        // field at offset 352 that always contains the full identifier.
        // Only override if the header VBNV is longer (more complete) than
        // what we got from XML.
        if let Some(vbnv) = extract_platform_vbnv(&bytes) {
            if vbnv.len() > meta.platform.len() {
                meta.platform = vbnv;
            }
        }
        apply_emulation_mode_env(meta.mode);
        ensure_xrt_emulation_bootstrap(&meta)?;

        let device_id = select_device(&meta)?;
        let ocl_device = OclDevice::new(device_id);
        let context = ocl_result(Context::from_device(&ocl_device), "create OpenCL context")?;
        let queue = ocl_result(
            #[allow(
                deprecated,
                reason = "opencl3 CommandQueue::create_default is the stable API for this crate version"
            )]
            CommandQueue::create_default(
                &context,
                CL_QUEUE_OUT_OF_ORDER_EXEC_MODE_ENABLE | CL_QUEUE_PROFILING_ENABLE,
            ),
            "create OpenCL command queue",
        )?;

        // SAFETY: create_from_binary requires a valid OpenCL context and a
        // device id obtained from the same platform.  Both are guaranteed by
        // the preceding select_device / Context::from_device calls.
        let mut program = unsafe {
            ocl_result(
                Program::create_from_binary(&context, &[device_id], &[&bytes]),
                "create OpenCL program from xclbin",
            )?
        };
        ocl_result(program.build(&[device_id], ""), "build OpenCL program")?;

        let kernel = ocl_result(
            Kernel::create(&program, &meta.top_name),
            "create OpenCL kernel",
        )?;

        Ok(Self {
            _meta: meta,
            context,
            queue,
            kernel,
            scalars: HashMap::new(),
            buffers: HashMap::new(),
            load_events: Vec::new(),
            compute_events: Vec::new(),
            store_events: Vec::new(),
            load_ns: 0,
            compute_ns: 0,
            store_ns: 0,
            finished: true,
        })
    }
}

fn apply_emulation_mode_env(mode: super::metadata::XclbinMode) {
    if std::env::var_os("XCL_EMULATION_MODE").is_some() {
        return;
    }
    match mode {
        super::metadata::XclbinMode::HwEmu => {
            std::env::set_var("XCL_EMULATION_MODE", "hw_emu");
        }
        super::metadata::XclbinMode::SwEmu => {
            std::env::set_var("XCL_EMULATION_MODE", "sw_emu");
        }
        super::metadata::XclbinMode::Flat => {}
    }
}

fn ensure_xrt_emulation_bootstrap(meta: &XrtMetadata) -> Result<()> {
    if std::env::var_os("XCL_EMULATION_MODE").is_none() {
        return Ok(());
    }

    // Preserve parent environment and overlay Xilinx toolchain settings.
    for (k, v) in xilinx_environ() {
        std::env::set_var(k, v);
    }

    // SAFETY: geteuid() is always safe to call; it has no preconditions.
    let uid = unsafe { libc::geteuid() };
    let user = current_username().unwrap_or_else(|| uid.to_string());
    std::env::set_var("USER", user);

    let tmp_root = std::env::var("TMPDIR").unwrap_or_else(|_| "/tmp".to_owned());
    let frt_tmp = PathBuf::from(tmp_root).join(format!(".frt.{uid}"));
    std::fs::create_dir_all(&frt_tmp)?;
    if std::env::var_os("SDACCEL_EM_RUN_DIR").is_none() {
        std::env::set_var("SDACCEL_EM_RUN_DIR", &frt_tmp);
    }

    let emconfig_dir = if let Some(path) = env_non_empty("EMCONFIG_PATH") {
        PathBuf::from(path)
    } else {
        let p = frt_tmp.join(format!("emconfig.{}", meta.platform));
        std::env::set_var("EMCONFIG_PATH", &p);
        p
    };

    if meta.platform.is_empty() || emconfig_ready(&emconfig_dir, &meta.platform) {
        return Ok(());
    }

    std::fs::create_dir_all(&emconfig_dir)?;
    let status = Command::new("emconfigutil")
        .arg("--platform")
        .arg(&meta.platform)
        .arg("--od")
        .arg(&emconfig_dir)
        .status()?;
    if !status.success() {
        return Err(FrtError::MetadataParse(format!(
            "emconfigutil failed for platform '{}' with status {status}",
            meta.platform
        )));
    }
    if !emconfig_ready(&emconfig_dir, &meta.platform) {
        return Err(FrtError::MetadataParse(format!(
            "emconfigutil did not produce a valid emconfig.json for '{}'",
            meta.platform
        )));
    }
    Ok(())
}

fn emconfig_ready(dir: &Path, platform: &str) -> bool {
    let path = dir.join("emconfig.json");
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
        return false;
    };
    let Some(boards) = v
        .get("Platform")
        .and_then(|x| x.get("Boards"))
        .and_then(|x| x.as_array())
    else {
        return false;
    };
    for board in boards {
        let Some(devices) = board.get("Devices").and_then(|x| x.as_array()) else {
            continue;
        };
        for dev in devices {
            if dev
                .get("Name")
                .and_then(|x| x.as_str())
                .is_some_and(|name| name == platform)
            {
                return true;
            }
        }
    }
    false
}

impl Device for XrtDevice {
    fn set_scalar_arg(&mut self, index: u32, value: &[u8]) -> Result<()> {
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
        if bytes == 0 {
            return Err(FrtError::MetadataParse(format!(
                "zero-length buffer arg {index} is unsupported in XRT runtime"
            )));
        }
        if ptr.is_null() && bytes != 0 {
            return Err(FrtError::MetadataParse(format!(
                "null host pointer for non-empty buffer arg {index}"
            )));
        }
        // SAFETY: ptr is non-null (checked above) and points to a host buffer
        // of at least `bytes` bytes.  CL_MEM_USE_HOST_PTR tells OpenCL to use
        // that allocation directly.
        let buffer = unsafe {
            ocl_result(
                Buffer::<u8>::create(
                    &self.context,
                    CL_MEM_READ_WRITE | CL_MEM_USE_HOST_PTR,
                    bytes,
                    ptr.cast::<c_void>(),
                ),
                "create OpenCL buffer",
            )?
        };
        self.buffers.insert(
            index,
            BufferBinding {
                ptr,
                bytes,
                access,
                buffer,
            },
        );
        Ok(())
    }

    fn set_stream_arg(&mut self, index: u32, _shm_path: &str) -> Result<()> {
        if self
            ._meta
            .args
            .iter()
            .any(|a| a.id == index && matches!(a.kind, XrtArgKind::Stream { .. }))
        {
            return Err(FrtError::MetadataParse(
                "XRT/OpenCL stream arguments are not supported in this runtime path".into(),
            ));
        }
        Err(FrtError::MetadataParse(format!(
            "no stream arg at index {index}"
        )))
    }

    fn suspend_buffer(&mut self, index: u32) -> usize {
        usize::from(self.buffers.remove(&index).is_some())
    }

    fn write_to_device(&mut self) -> Result<()> {
        self.load_events.clear();
        for binding in self.buffers.values_mut() {
            if !binding.access.loads_from_host() {
                continue;
            }
            // SAFETY: binding.ptr is non-null (set_buffer_arg checks this) and
            // binding.bytes is the size of the host allocation.
            let host_slice = unsafe { std::slice::from_raw_parts(binding.ptr, binding.bytes) };
            // SAFETY: self.queue and binding.buffer are valid OpenCL handles,
            // and host_slice points to the host allocation.
            let event = unsafe {
                ocl_result(
                    self.queue.enqueue_write_buffer(
                        &mut binding.buffer,
                        CL_BLOCKING,
                        0,
                        host_slice,
                        &[],
                    ),
                    "enqueue OpenCL write buffer",
                )?
            };
            self.load_events.push(event);
        }
        self.load_ns = elapsed_ns(&self.load_events);
        Ok(())
    }

    fn read_from_device(&mut self) -> Result<()> {
        self.store_events.clear();
        let waits: Vec<cl_event> = self.compute_events.iter().map(Event::get).collect();
        for binding in self.buffers.values() {
            if !binding.access.stores_to_host() {
                continue;
            }
            // SAFETY: binding.ptr is non-null (set_buffer_arg checks this) and
            // binding.bytes is the size of the host allocation.
            let host_slice = unsafe { std::slice::from_raw_parts_mut(binding.ptr, binding.bytes) };
            // SAFETY: self.queue and binding.buffer are valid OpenCL handles,
            // and host_slice points to the host allocation for readback.
            let event = unsafe {
                ocl_result(
                    self.queue.enqueue_read_buffer(
                        &binding.buffer,
                        CL_BLOCKING,
                        0,
                        host_slice,
                        &waits,
                    ),
                    "enqueue OpenCL read buffer",
                )?
            };
            self.store_events.push(event);
        }
        self.store_ns = elapsed_ns(&self.store_events);
        Ok(())
    }

    fn exec(&mut self) -> Result<()> {
        self.finished = false;
        self.compute_events.clear();

        let mut args = self._meta.args.clone();
        args.sort_by_key(|a| a.id);

        // Set all kernel arguments using clSetKernelArg with explicit indices.
        // ExecuteKernel::set_arg uses a sequential internal counter (0, 1, 2, …)
        // which is wrong when scalar and mmap args are interleaved at non-zero
        // indices.  Use set_kernel_arg directly for both kinds instead.
        for arg in &args {
            match arg.kind {
                XrtArgKind::Scalar { width } => {
                    let raw = normalized_scalar_bytes(
                        width,
                        self.scalars.get(&arg.id).map(std::vec::Vec::as_slice),
                    );
                    // SAFETY: self.kernel is a valid OpenCL kernel,
                    // arg.id is the correct argument index, and raw is
                    // a properly-sized byte buffer for the scalar value.
                    unsafe {
                        set_kernel_arg(
                            self.kernel.get(),
                            arg.id,
                            raw.len(),
                            raw.as_ptr().cast::<c_void>(),
                        )
                        .map_err(|code| FrtError::OpenCl {
                            code,
                            msg: format!("set OpenCL scalar kernel arg: error code {code}"),
                        })?;
                    };
                }
                XrtArgKind::Mmap { .. } => {
                    let binding = self.buffers.get(&arg.id).ok_or_else(|| {
                        FrtError::MetadataParse(format!(
                            "missing mmap arg binding for id {}",
                            arg.id
                        ))
                    })?;
                    // Pass a pointer to the cl_mem handle.  clSetKernelArg
                    // expects arg_value to point to the cl_mem value itself.
                    let cl_mem_handle = binding.buffer.get();
                    // SAFETY: self.kernel is a valid OpenCL kernel,
                    // arg.id is the correct argument index, and
                    // cl_mem_handle is a valid cl_mem obtained from
                    // the buffer created earlier.
                    unsafe {
                        set_kernel_arg(
                            self.kernel.get(),
                            arg.id,
                            std::mem::size_of_val(&cl_mem_handle),
                            (&raw const cl_mem_handle).cast::<c_void>(),
                        )
                        .map_err(|code| FrtError::OpenCl {
                            code,
                            msg: format!("set OpenCL mmap kernel arg: error code {code}"),
                        })?;
                    };
                }
                XrtArgKind::Stream { .. } => {
                    return Err(FrtError::MetadataParse(
                        "XRT/OpenCL stream arguments are not supported in this runtime path".into(),
                    ));
                }
            }
        }

        let waits: Vec<cl_event> = self.load_events.iter().map(Event::get).collect();
        let global_work_size: usize = 1;
        // SAFETY: self.kernel and self.queue are valid OpenCL handles,
        // global_work_size is a valid 1-element array, and waits contains
        // only events from prior enqueue operations on the same queue.
        let evt = unsafe {
            ocl_result(
                self.queue.enqueue_nd_range_kernel(
                    self.kernel.get(),
                    1,
                    std::ptr::null(),
                    (&raw const global_work_size).cast(),
                    std::ptr::null(),
                    &waits,
                ),
                "enqueue OpenCL kernel",
            )?
        };
        self.compute_events.push(evt);
        self.compute_ns = elapsed_ns(&self.compute_events);
        Ok(())
    }

    fn finish(&mut self) -> Result<()> {
        ocl_result(self.queue.finish(), "finish OpenCL queue")?;
        self.finished = true;
        Ok(())
    }

    fn kill(&mut self) -> Result<()> {
        // OpenCL kernels do not support forced cancellation in this runtime.
        self.finished = true;
        Ok(())
    }

    fn is_finished(&mut self) -> Result<bool> {
        if self.finished {
            return Ok(true);
        }
        if self.compute_events.is_empty() {
            return Ok(true);
        }
        let mut all_done = true;
        for evt in &self.compute_events {
            let status = ocl_result(
                evt.command_execution_status(),
                "query OpenCL event execution status",
            )?;
            if status.0 > 0 {
                all_done = false;
                break;
            }
        }
        if all_done {
            self.finished = true;
        }
        Ok(all_done)
    }

    fn args_info(&self) -> Vec<RuntimeArgInfo> {
        let mut args = Vec::with_capacity(self._meta.args.len());
        for arg in &self._meta.args {
            let (type_name, category) = match arg.kind {
                XrtArgKind::Scalar { width } => {
                    (scalar_type_name(width), RuntimeArgCategory::Scalar)
                }
                XrtArgKind::Mmap { .. } => ("mmap".to_owned(), RuntimeArgCategory::Mmap),
                XrtArgKind::Stream { .. } => ("stream".to_owned(), RuntimeArgCategory::Stream),
            };
            args.push(RuntimeArgInfo {
                index: arg.id,
                name: arg.name.clone(),
                type_name,
                category,
            });
        }
        args.sort_by_key(|a| a.index);
        args
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

fn select_device(meta: &XrtMetadata) -> Result<cl_device_id> {
    let requested_bdf = env_non_empty("FRT_XOCL_BDF").or_else(|| env_non_empty("XOCL_BDF"));
    let platforms = ocl_result(get_platforms(), "enumerate OpenCL platforms")?;
    for p in &platforms {
        let vendor = ocl_result(p.vendor(), "query OpenCL platform vendor")?;
        if !vendor.to_ascii_lowercase().contains("xilinx") {
            continue;
        }
        let devices = ocl_result(
            p.get_devices(CL_DEVICE_TYPE_ACCELERATOR),
            "enumerate Xilinx OpenCL accelerator devices",
        )?;
        if let Some(id) = pick_device(&devices, meta, requested_bdf.as_deref())? {
            return Ok(id);
        }
    }
    for p in &platforms {
        let devices = ocl_result(
            p.get_devices(CL_DEVICE_TYPE_ALL),
            "enumerate OpenCL devices",
        )?;
        if let Some(id) = pick_device(&devices, meta, requested_bdf.as_deref())? {
            return Ok(id);
        }
    }
    if let Some(bdf) = requested_bdf {
        return Err(FrtError::MetadataParse(format!(
            "no OpenCL device matching requested XOCL BDF '{bdf}'"
        )));
    }
    Err(FrtError::MetadataParse(
        "no OpenCL device available for XRT runtime".into(),
    ))
}

fn pick_device(
    devices: &[cl_device_id],
    meta: &XrtMetadata,
    requested_bdf: Option<&str>,
) -> Result<Option<cl_device_id>> {
    for id in devices {
        let dev = OclDevice::new(*id);
        if let Some(bdf) = requested_bdf {
            let got = device_bdf(*id);
            if got.as_deref() != Some(bdf) {
                continue;
            }
        }
        if !meta.platform.is_empty() {
            let name = ocl_result(dev.name(), "query OpenCL device name")?;
            if !platform_name_matches(&name, &meta.platform) {
                continue;
            }
        }
        return Ok(Some(*id));
    }
    Ok(None)
}

fn platform_name_matches(device_name: &str, target_platform: &str) -> bool {
    if device_name == target_platform {
        return true;
    }
    // Fuzzy match: compare vendor_board_iface_type prefix, then check that
    // the device's shell token exactly matches the target's shell token.
    let target: Vec<&str> = target_platform.split('_').collect();
    let device: Vec<&str> = device_name.split('_').collect();
    if target.len() < 5 || device.len() < 6 {
        return false;
    }
    for i in 0..4 {
        if target[i] != device[i] {
            return false;
        }
    }
    // Require exact shell revision match to avoid selecting the wrong board.
    target[4] == device[5]
}

use frt_shm::env_non_empty;

fn current_username() -> Option<String> {
    #[cfg(unix)]
    {
        // SAFETY: geteuid() has no preconditions.
        let uid = unsafe { libc::geteuid() };
        let mut pwd = std::mem::MaybeUninit::<libc::passwd>::zeroed();
        let mut result: *mut libc::passwd = std::ptr::null_mut();
        let mut buf = vec![0u8; 1024];

        loop {
            // SAFETY: getpwuid_r is called with a properly-sized buffer,
            // a zeroed MaybeUninit<passwd>, and a valid result pointer.
            // The buffer is resized on ERANGE and retried.
            let rc = unsafe {
                libc::getpwuid_r(
                    uid,
                    pwd.as_mut_ptr(),
                    buf.as_mut_ptr().cast::<libc::c_char>(),
                    buf.len(),
                    &raw mut result,
                )
            };
            if rc == 0 {
                if result.is_null() {
                    break;
                }
                // SAFETY: getpwuid_r returned 0 with a non-null result,
                // so pwd is fully initialized and pw_name (if non-null)
                // points to a NUL-terminated string inside buf.
                let pwd = unsafe { pwd.assume_init() };
                if pwd.pw_name.is_null() {
                    break;
                }
                // SAFETY: pw_name is non-null (checked above) and points
                // to a NUL-terminated string within buf.
                let name = unsafe { CStr::from_ptr(pwd.pw_name) };
                return name.to_str().ok().map(std::borrow::ToOwned::to_owned);
            }
            if rc == libc::ERANGE {
                buf.resize(buf.len().saturating_mul(2).max(1024), 0);
                continue;
            }
            break;
        }
    }

    if let Some(user) = env_non_empty("USER") {
        return Some(user);
    }

    None
}

fn normalized_scalar_bytes(width_bits: u32, raw: Option<&[u8]>) -> Vec<u8> {
    if let Some(raw) = raw.filter(|raw| !raw.is_empty()) {
        return raw.to_vec();
    }
    vec![0; (width_bits as usize).div_ceil(8).max(1)]
}

fn scalar_type_name(width_bits: u32) -> String {
    match width_bits {
        0..=32 => "uint32_t".to_owned(),
        33..=64 => "uint64_t".to_owned(),
        _ => format!("uint{width_bits}_t"),
    }
}

fn device_bdf(id: cl_device_id) -> Option<String> {
    // Xilinx extension used by legacy C++ runtime (`CL_DEVICE_PCIE_BDF`).
    const CL_DEVICE_PCIE_BDF: u32 = 0x4038;
    let bytes = OclDevice::new(id).get_data(CL_DEVICE_PCIE_BDF).ok()?;
    let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
    let text = String::from_utf8(bytes[..end].to_vec()).ok()?;
    let bdf = text.trim().to_owned();
    if bdf.is_empty() {
        None
    } else {
        Some(bdf)
    }
}

fn ocl_result<T>(res: opencl3::Result<T>, ctx: &str) -> Result<T> {
    res.map_err(|e| FrtError::OpenCl {
        code: e.0,
        msg: format!("{ctx}: {e}"),
    })
}

fn elapsed_ns(events: &[Event]) -> u64 {
    if events.is_empty() {
        return 0;
    }
    let mut start = u64::MAX;
    let mut end = 0u64;
    for e in events {
        if let (Ok(s), Ok(t)) = (e.profiling_command_start(), e.profiling_command_end()) {
            start = start.min(s);
            end = end.max(t);
        }
    }
    if start == u64::MAX || end < start {
        0
    } else {
        end - start
    }
}

#[cfg(test)]
mod tests {
    use super::{normalized_scalar_bytes, scalar_type_name};

    #[test]
    fn scalar_bytes_preserve_explicit_caller_size() {
        assert_eq!(normalized_scalar_bytes(16, Some(&[0x12])), vec![0x12]);
        assert_eq!(
            normalized_scalar_bytes(16, Some(&[0x12, 0x34, 0x56])),
            vec![0x12, 0x34, 0x56]
        );
        assert_eq!(
            normalized_scalar_bytes(128, Some(&[1, 2, 3, 4])),
            vec![1, 2, 3, 4]
        );
    }

    #[test]
    fn scalar_bytes_default_to_metadata_width_when_unset() {
        assert_eq!(normalized_scalar_bytes(16, None), vec![0x00, 0x00]);
        assert_eq!(
            normalized_scalar_bytes(128, None),
            vec![0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]
        );
    }

    #[test]
    fn scalar_type_names_expand_beyond_u64() {
        assert_eq!(scalar_type_name(1), "uint32_t");
        assert_eq!(scalar_type_name(64), "uint64_t");
        assert_eq!(scalar_type_name(128), "uint128_t");
    }
}
