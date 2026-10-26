use super::api::{
    api, Api, Handle, RUN_ABORT, RUN_COMPLETED, RUN_ERROR, RUN_NORESPONSE, RUN_SKCRASHED,
    RUN_SKERROR, RUN_SUBMITTED, RUN_TIMEOUT, SYNC_FROM_DEVICE, SYNC_TO_DEVICE,
};
use super::metadata::{
    extract_embedded_xml, extract_platform_vbnv, parse_aie_graph, parse_embedded_xml, AieGraph,
};
use crate::device::{BufferAccess, Device};
use crate::error::{FrtError, Result};
use frt_cosim::metadata::kernel_xml::{KernelXml, XclbinTarget};
use frt_cosim::metadata::normalized_scalar_bytes;
use frt_cosim::metadata::ArgKind;
use frt_cosim::runner::environ::xilinx_environ;
use std::collections::HashMap;
use std::ffi::{c_int, c_void, CStr, CString};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

/// A host buffer and the XRT buffer object the kernel sees. A page-aligned
/// host buffer backs its buffer object directly; any other is staged
/// through a device-side copy.
struct BufferBinding {
    ptr: *mut u8,
    bytes: usize,
    access: BufferAccess,
    bo: Handle,
    staged: bool,
}

enum ExecutionState {
    Idle,
    Running(Instant),
    Finished,
}

/// The AI Engine graph an xclbin carries, open for control.
struct Graph {
    handle: Handle,
    meta: AieGraph,
    /// Whether it ran since the xclbin was loaded: a run-forever graph is
    /// reset before it runs again, so its kernels restart from their entry.
    has_run: bool,
}

/// XRT's user-pointer buffers must start on a 4 KiB boundary.
const USER_PTR_ALIGNMENT: usize = 4096;

pub struct XrtDevice {
    meta: KernelXml,
    api: &'static Api,
    device: Handle,
    kernel: Handle,
    run: Handle,
    graph: Option<Graph>,
    scalars: HashMap<u32, Vec<u8>>,
    buffers: HashMap<u32, BufferBinding>,
    execution_state: ExecutionState,
    load_ns: u64,
    compute_ns: u64,
    store_ns: u64,
}

// SAFETY: XrtDevice is only accessed from a single thread at a time; the
// XRT handles it owns are reference-counted objects inside XRT.
unsafe impl Send for XrtDevice {}

/// Fails with what XRT was asked to do when a call returns non-zero.
fn check(rc: c_int, action: &str) -> Result<()> {
    if rc == 0 {
        Ok(())
    } else {
        Err(FrtError::Xrt(format!(
            "cannot {action} (XRT returned {rc})"
        )))
    }
}

fn non_null(handle: Handle, action: &str) -> Result<Handle> {
    if handle.is_null() {
        Err(FrtError::Xrt(format!("cannot {action}")))
    } else {
        Ok(handle)
    }
}

fn elapsed_ns(since: Instant) -> u64 {
    u64::try_from(since.elapsed().as_nanos()).unwrap_or(u64::MAX)
}

/// Whether an `ert_cmd_state` is a terminal failure rather than a pending
/// state or successful completion. ERT assigns SUBMITTED a value above the
/// terminal errors, so range comparisons are not a valid state classifier.
fn is_run_error(state: c_int) -> bool {
    matches!(
        state,
        RUN_ERROR | RUN_ABORT | RUN_TIMEOUT | RUN_NORESPONSE | RUN_SKERROR | RUN_SKCRASHED
    )
}

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
        bootstrap_emulation_env_once(&meta)?;
        let api = api()?;

        let requested_bdf = env_non_empty(frt_shm::env::FRT_XOCL_BDF);
        let bdf = requested_bdf
            .as_deref()
            .map(|value| {
                CString::new(value)
                    .map_err(|e| FrtError::Xrt(format!("invalid device BDF `{value}`: {e}")))
            })
            .transpose()?;
        let path = CString::new(xclbin_path.to_string_lossy().into_owned())
            .map_err(|e| FrtError::Xrt(format!("xclbin path: {e}")))?;
        // XRT has no C device-name enumeration API. Without an explicit BDF,
        // use xclbin acceptance as the platform check instead of assuming
        // device 0. A failed load closes the candidate before the next index.
        let mut index = 0;
        let mut last_error;
        let mut this = loop {
            let device = if let Some(bdf) = &bdf {
                // SAFETY: a valid C string.
                unsafe { (api.device_open_by_bdf)(bdf.as_ptr()) }
            } else {
                // SAFETY: plain index argument.
                unsafe { (api.device_open)(index) }
            };
            let device = non_null(device, "open a device")?;
            let candidate = Self {
                meta: meta.clone(),
                api,
                device,
                kernel: std::ptr::null_mut(),
                run: std::ptr::null_mut(),
                graph: None,
                scalars: HashMap::new(),
                buffers: HashMap::new(),
                execution_state: ExecutionState::Idle,
                load_ns: 0,
                compute_ns: 0,
                store_ns: 0,
            };
            // SAFETY: live device handle and a valid C string.
            let rc = unsafe { (api.device_load_xclbin_file)(candidate.device, path.as_ptr()) };
            if rc == 0 {
                break candidate;
            }
            last_error = rc;
            if bdf.is_some() {
                return Err(FrtError::Xrt(format!(
                    "cannot load the xclbin on the requested device (XRT returned {rc})"
                )));
            }
            drop(candidate);
            index += 1;
            if index >= 64 {
                return Err(FrtError::Xrt(format!(
                    "cannot load the xclbin on any available device (last XRT returned {last_error})"
                )));
            }
        };
        let mut uuid = [0u8; 16];
        // SAFETY: XRT writes the 16-byte xclbin UUID.
        let rc = unsafe { (api.device_get_xclbin_uuid)(this.device, uuid.as_mut_ptr()) };
        check(rc, "read the xclbin UUID")?;
        let name = CString::new(this.meta.top_name.clone())
            .map_err(|e| FrtError::Xrt(format!("kernel name: {e}")))?;
        // SAFETY: live device, the UUID it reported, a valid C string.
        let kernel = unsafe { (api.kernel_open)(this.device, uuid.as_ptr(), name.as_ptr()) };
        this.kernel = non_null(kernel, &format!("open kernel `{}`", this.meta.top_name))?;
        // SAFETY: live kernel handle.
        let run = unsafe { (api.run_open)(this.kernel) };
        this.run = non_null(run, "create a kernel run")?;
        if let Some(meta) = parse_aie_graph(&bytes)? {
            let name = CString::new(meta.name.clone())
                .map_err(|e| FrtError::Xrt(format!("graph name: {e}")))?;
            // SAFETY: live device, the UUID it reported, a valid C string.
            let handle = unsafe { (api.graph_open)(this.device, uuid.as_ptr(), name.as_ptr()) };
            let handle = non_null(handle, &format!("open the AI Engine graph `{}`", meta.name))?;
            this.graph = Some(Graph {
                handle,
                meta,
                has_run: false,
            });
        }
        Ok(this)
    }

    /// Starts the AI Engine graph, if any, with each runtime parameter set
    /// to the low bytes of the scalar argument it carries (`values`, by
    /// argument index): one iteration, or forever when some kernel never
    /// returns.
    fn start_graph(&mut self, values: &HashMap<u32, Vec<u8>>) -> Result<()> {
        let Some(graph) = &mut self.graph else {
            return Ok(());
        };
        if graph.meta.run_forever && graph.has_run {
            // SAFETY: live graph handle.
            let rc = unsafe { (self.api.graph_reset)(graph.handle) };
            check(rc, "reset the AI Engine graph")?;
        }
        for rtp in &graph.meta.rtps {
            let mut value = values.get(&rtp.arg).cloned().ok_or_else(|| {
                FrtError::Xrt(format!(
                    "runtime parameter `{}` carries argument {}, which is not a scalar",
                    rtp.alias, rtp.arg
                ))
            })?;
            value.resize(rtp.bytes, 0);
            let alias = CString::new(rtp.alias.clone())
                .map_err(|e| FrtError::Xrt(format!("runtime parameter name: {e}")))?;
            // SAFETY: live graph handle; `value` holds `value.len()` bytes.
            let rc = unsafe {
                (self.api.graph_update_rtp)(
                    graph.handle,
                    alias.as_ptr(),
                    value.as_ptr().cast(),
                    value.len(),
                )
            };
            check(rc, &format!("update runtime parameter `{}`", rtp.alias))?;
        }
        let iterations = if graph.meta.run_forever { -1 } else { 1 };
        // SAFETY: live graph handle.
        let rc = unsafe { (self.api.graph_run)(graph.handle, iterations) };
        check(rc, "run the AI Engine graph")?;
        graph.has_run = true;
        Ok(())
    }

    /// Once the PL kernel is done: waits for a terminating graph to finish,
    /// or stops a run-forever one. The latter has run for more than one
    /// cycle, so waiting one cycle stops it at once.
    fn stop_graph(&self) -> Result<()> {
        let Some(graph) = &self.graph else {
            return Ok(());
        };
        let cycles = u64::from(graph.meta.run_forever);
        // SAFETY: live graph handle.
        let rc = unsafe { (self.api.graph_wait)(graph.handle, cycles) };
        check(rc, "wait for the AI Engine graph")
    }

    /// Stops and resets a graph after a PL failure. A bounded wait is not
    /// appropriate here: the graph may be blocked on PL input that will never
    /// arrive, and a retry needs its kernels reset in either case.
    fn abort_graph(&self) -> Result<()> {
        let Some(graph) = &self.graph else {
            return Ok(());
        };
        // SAFETY: live graph handle.
        let rc = unsafe { (self.api.graph_reset)(graph.handle) };
        check(rc, "reset the AI Engine graph after a kernel failure")
    }
}

impl Drop for XrtDevice {
    fn drop(&mut self) {
        // Each handle is live or null and closed once, children first.
        if let Some(graph) = &self.graph {
            // SAFETY: live graph handle.
            unsafe { (self.api.graph_close)(graph.handle) };
        }
        if !self.run.is_null() {
            // SAFETY: live run handle.
            unsafe { (self.api.run_close)(self.run) };
        }
        for binding in self.buffers.values() {
            // SAFETY: live buffer handle, no longer used by the closed run.
            unsafe { (self.api.bo_free)(binding.bo) };
        }
        if !self.kernel.is_null() {
            // SAFETY: live kernel handle with no open run.
            unsafe { (self.api.kernel_close)(self.kernel) };
        }
        // SAFETY: live device handle with no open children.
        unsafe { (self.api.device_close)(self.device) };
    }
}

/// A scalar as XRT's variadic `xrtRunSetArg` reads it: by the argument's
/// type in the xclbin (`int` and `uint` as a 32-bit word, `float` and
/// `double` as a `double`), else by its size (4 bytes as a 32-bit word,
/// anything up to 8 bytes as a `size_t`). `int` and `unsigned int` share
/// one variadic slot, so a 32-bit word passes either.
#[derive(Debug, Clone, Copy, PartialEq)]
enum VaScalar {
    U32(u32),
    U64(u64),
    Double(f64),
}

fn va_scalar(id: u32, host_type: &str, bytes: &[u8]) -> Result<VaScalar> {
    let le = |n: usize| {
        let mut word = [0u8; 8];
        word[..n.min(bytes.len())].copy_from_slice(&bytes[..n.min(bytes.len())]);
        u64::from_le_bytes(word)
    };
    let low32 = || u32::try_from(le(4) & 0xffff_ffff).expect("masked to 32 bits");
    Ok(match host_type {
        "int" | "uint" => VaScalar::U32(low32()),
        "float" => VaScalar::Double(f64::from(f32::from_bits(low32()))),
        "double" => VaScalar::Double(f64::from_bits(le(8))),
        _ => match bytes.len() {
            4 => VaScalar::U32(low32()),
            n if n <= 8 => VaScalar::U64(le(8)),
            n => {
                return Err(FrtError::Xrt(format!(
                    "scalar argument {id} is {n} bytes wide; XRT passes scalars of at \
                     most 8 bytes"
                )))
            }
        },
    })
}

/// One-shot process-env bootstrap for XRT emulation.
///
/// XRT configures itself from the process environment
/// (`XCL_EMULATION_MODE`, `EMCONFIG_PATH`, …) — there is no API
/// alternative, so the `setenv` calls themselves cannot go away. What
/// can be bounded is *when* they happen: everything is written exactly
/// once per process, before the first XRT device is opened, and never
/// again — so once XRT's own worker threads (or the caller's) are
/// running, no thread mutates the environment under them. Concurrent
/// first-time opens on several threads serialize on the `OnceLock`;
/// later opens (whatever their xclbin's mode) reuse the first
/// bootstrap's outcome, which the `XCL_EMULATION_MODE`-already-set
/// early-out made true of the old per-open code path as well.
fn bootstrap_emulation_env_once(meta: &KernelXml) -> Result<()> {
    static BOOTSTRAP: std::sync::OnceLock<std::result::Result<(), String>> =
        std::sync::OnceLock::new();
    BOOTSTRAP
        .get_or_init(|| {
            apply_emulation_mode_env(meta.target);
            ensure_xrt_emulation_bootstrap(meta).map_err(|e| e.to_string())
        })
        .clone()
        .map_err(FrtError::MetadataParse)
}

/// The `XCL_EMULATION_MODE` value an xclbin of this mode asks for, or `None`
/// when it runs on real hardware and wants the variable left alone.
fn emulation_mode_env_value(target: XclbinTarget) -> Option<&'static str> {
    match target {
        XclbinTarget::HwEmu => Some("hw_emu"),
        XclbinTarget::SwEmu => Some("sw_emu"),
        XclbinTarget::Flat => None,
    }
}

fn apply_emulation_mode_env(target: XclbinTarget) {
    if std::env::var_os("XCL_EMULATION_MODE").is_some() {
        return;
    }
    if let Some(value) = emulation_mode_env_value(target) {
        std::env::set_var("XCL_EMULATION_MODE", value);
    }
}

fn ensure_xrt_emulation_bootstrap(meta: &KernelXml) -> Result<()> {
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

    // Versal guests use the platform's QEMU configuration, not an Alveo
    // host-side emconfig.json or the emconfigutil installed on that host.
    if cfg!(target_arch = "aarch64") {
        return Ok(());
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
        if ptr.is_null() {
            return Err(FrtError::MetadataParse(format!(
                "null host pointer for non-empty buffer arg {index}"
            )));
        }
        let arg = c_int::try_from(index)
            .map_err(|e| FrtError::Xrt(format!("argument index {index}: {e}")))?;
        // SAFETY: live kernel handle.
        let group = unsafe { (self.api.kernel_arg_group_id)(self.kernel, arg) };
        let group = u32::try_from(group)
            .map_err(|e| FrtError::Xrt(format!("argument {index} has no memory bank: {e}")))?;
        let staged = !(ptr as usize).is_multiple_of(USER_PTR_ALIGNMENT);
        let bo = if staged {
            // SAFETY: live device handle.
            unsafe { (self.api.bo_alloc)(self.device, bytes, 0, group) }
        } else {
            // SAFETY: live device; `ptr` holds `bytes` bytes that outlive the
            // run.
            unsafe {
                (self.api.bo_alloc_user_ptr)(self.device, ptr.cast::<c_void>(), bytes, 0, group)
            }
        };
        let bo = non_null(bo, &format!("allocate the buffer of argument {index}"))?;
        // Bind now rather than in `exec`: XRT derives the buffer's bank from
        // the argument it is bound to, and `write_to_device` runs first.
        // SAFETY: live run and buffer handles; a buffer argument reads a
        // buffer handle.
        let rc = unsafe { (self.api.run_set_arg)(self.run, arg, bo) };
        if let Err(error) = check(rc, &format!("bind argument {index}")) {
            // SAFETY: `bo` was allocated above and ownership was never
            // transferred to `self.buffers`.
            unsafe { (self.api.bo_free)(bo) };
            return Err(error);
        }
        if let Some(old) = self.buffers.insert(
            index,
            BufferBinding {
                ptr,
                bytes,
                access,
                bo,
                staged,
            },
        ) {
            // SAFETY: the replaced buffer handle is no longer bound.
            unsafe { (self.api.bo_free)(old.bo) };
        }
        Ok(())
    }

    fn set_stream_arg(&mut self, index: u32, _shm_path: &str) -> Result<()> {
        Err(FrtError::MetadataParse(format!(
            "argument {index}: stream arguments cannot be bound from the host in the \
             XRT runtime"
        )))
    }

    fn write_to_device(&mut self) -> Result<()> {
        let start = Instant::now();
        for (index, binding) in &self.buffers {
            if !binding.access.loads_from_host() {
                continue;
            }
            if binding.staged {
                // SAFETY: `ptr` holds `bytes` bytes; live buffer handle.
                let rc = unsafe {
                    (self.api.bo_write)(binding.bo, binding.ptr.cast(), binding.bytes, 0)
                };
                check(rc, &format!("stage argument {index}"))?;
            }
            // SAFETY: live buffer handle of `bytes` bytes.
            let rc = unsafe { (self.api.bo_sync)(binding.bo, SYNC_TO_DEVICE, binding.bytes, 0) };
            check(rc, &format!("write argument {index} to the device"))?;
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
            // SAFETY: live buffer handle of `bytes` bytes.
            let rc = unsafe { (self.api.bo_sync)(binding.bo, SYNC_FROM_DEVICE, binding.bytes, 0) };
            check(rc, &format!("read argument {index} from the device"))?;
            if binding.staged {
                // SAFETY: `ptr` holds `bytes` writable bytes.
                let rc =
                    unsafe { (self.api.bo_read)(binding.bo, binding.ptr.cast(), binding.bytes, 0) };
                check(rc, &format!("unstage argument {index}"))?;
            }
        }
        self.store_ns = elapsed_ns(start);
        Ok(())
    }

    fn exec(&mut self) -> Result<()> {
        let mut args = self.meta.args.clone();
        args.sort_by_key(|a| a.id);
        let mut values = HashMap::new();
        for arg in &args {
            match arg.kind {
                ArgKind::Scalar { width } => {
                    let raw = normalized_scalar_bytes(
                        width,
                        self.scalars.get(&arg.id).map(std::vec::Vec::as_slice),
                    );
                    let host_type = self.meta.host_types.get(&arg.id).map_or("", String::as_str);
                    let index = c_int::try_from(arg.id)
                        .map_err(|e| FrtError::Xrt(format!("argument index {}: {e}", arg.id)))?;
                    // Each value has the type XRT reads for this argument
                    // (see `va_scalar`).
                    let rc = match va_scalar(arg.id, host_type, &raw)? {
                        // SAFETY: live run handle; XRT reads a 32-bit word.
                        VaScalar::U32(v) => unsafe { (self.api.run_set_arg)(self.run, index, v) },
                        // SAFETY: live run handle; XRT reads a `size_t`.
                        VaScalar::U64(v) => unsafe { (self.api.run_set_arg)(self.run, index, v) },
                        // SAFETY: live run handle; XRT reads a `double`.
                        VaScalar::Double(v) => unsafe {
                            (self.api.run_set_arg)(self.run, index, v)
                        },
                    };
                    check(rc, &format!("set scalar argument {}", arg.id))?;
                    values.insert(arg.id, raw);
                }
                ArgKind::Mmap { .. } => {
                    // Already bound by `set_buffer_arg`; only its presence
                    // still needs checking here.
                    if !self.buffers.contains_key(&arg.id) {
                        return Err(FrtError::MetadataParse(format!(
                            "missing mmap arg binding for id {}",
                            arg.id
                        )));
                    }
                }
                // Wired inside the device (an AI Engine PLIO, say).
                ArgKind::Stream { .. } => {}
            }
        }
        // The graph runs first: it consumes the PL kernel's streams.
        if let Err(error) = self.start_graph(&values) {
            self.abort_graph()?;
            return Err(error);
        }
        // SAFETY: live run handle with every argument set.
        let rc = unsafe { (self.api.run_start)(self.run) };
        if let Err(error) = check(rc, "start the kernel") {
            self.abort_graph()?;
            return Err(error);
        }
        self.execution_state = ExecutionState::Running(Instant::now());
        Ok(())
    }

    fn finish(&mut self) -> Result<()> {
        if let ExecutionState::Running(start) = self.execution_state {
            // SAFETY: live run handle.
            let state = unsafe { (self.api.run_wait)(self.run) };
            self.compute_ns = elapsed_ns(start);
            self.execution_state = ExecutionState::Finished;
            if state != RUN_COMPLETED {
                self.abort_graph()?;
                return Err(FrtError::Xrt(format!(
                    "the kernel run ended in state {state}, not completed"
                )));
            }
            self.stop_graph()?;
        }
        Ok(())
    }

    fn kill(&mut self) -> Result<()> {
        self.execution_state = ExecutionState::Finished;
        self.abort_graph()
    }

    fn is_finished(&mut self) -> Result<bool> {
        let ExecutionState::Running(start) = self.execution_state else {
            return Ok(matches!(self.execution_state, ExecutionState::Finished));
        };
        // SAFETY: live run handle.
        let state = unsafe { (self.api.run_state)(self.run) };
        if state == RUN_COMPLETED {
            self.compute_ns = elapsed_ns(start);
            self.execution_state = ExecutionState::Finished;
            self.stop_graph()?;
            return Ok(true);
        }
        if state == RUN_SUBMITTED {
            return Ok(false);
        }
        if is_run_error(state) {
            self.compute_ns = elapsed_ns(start);
            self.execution_state = ExecutionState::Finished;
            self.abort_graph()?;
            return Err(FrtError::Xrt(format!(
                "the kernel run failed in state {state}"
            )));
        }
        // SUBMITTED (7) and any other nonterminal state is still pending;
        // the numeric ERT values do not form a pending/completed/error order.
        Ok(false)
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

#[cfg(test)]
mod tests {
    use super::{emulation_mode_env_value, is_run_error, va_scalar, VaScalar};
    use crate::xrt::api::{RUN_COMPLETED, RUN_SUBMITTED};
    use frt_cosim::metadata::kernel_xml::XclbinTarget;

    #[test]
    fn only_emulation_xclbins_ask_for_an_emulation_mode() {
        assert_eq!(
            emulation_mode_env_value(XclbinTarget::HwEmu),
            Some("hw_emu")
        );
        assert_eq!(
            emulation_mode_env_value(XclbinTarget::SwEmu),
            Some("sw_emu")
        );
        assert_eq!(emulation_mode_env_value(XclbinTarget::Flat), None);
    }

    /// `xrtRunSetArg` is variadic and reads each scalar by the type the
    /// xclbin gives it; a value passed as anything else is garbage.
    #[test]
    fn submitted_is_pending_not_an_error() {
        assert!(!is_run_error(RUN_SUBMITTED));
        assert!(!is_run_error(RUN_COMPLETED));
        for state in [5, 6, 8, 9, 10, 11] {
            assert!(is_run_error(state), "state {state} must be an error");
        }
    }

    #[test]
    fn scalars_take_the_variadic_type_xrt_reads() {
        let float = 1.5f32.to_le_bytes();
        assert_eq!(
            va_scalar(0, "float", &float).unwrap(),
            VaScalar::Double(1.5)
        );
        assert_eq!(
            va_scalar(0, "int", &(-3i32).to_le_bytes()).unwrap(),
            VaScalar::U32((-3i32).cast_unsigned())
        );
        assert_eq!(
            va_scalar(0, "uint", &[7, 0, 0, 0]).unwrap(),
            VaScalar::U32(7)
        );
        assert_eq!(
            va_scalar(0, "uint64_t", &5u64.to_le_bytes()).unwrap(),
            VaScalar::U64(5)
        );
        assert_eq!(
            va_scalar(0, "unsigned int", &[9, 0, 0, 0]).unwrap(),
            VaScalar::U32(9)
        );
        assert_eq!(
            va_scalar(0, "uint16_t", &[1, 2]).unwrap(),
            VaScalar::U64(0x201)
        );
        assert!(va_scalar(4, "ap_uint<128>", &[0; 16])
            .unwrap_err()
            .to_string()
            .contains("argument 4 is 16 bytes"));
    }
}
