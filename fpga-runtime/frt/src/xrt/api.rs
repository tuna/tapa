//! XRT's C API, loaded from `libxrt_coreutil` when a design first runs on
//! hardware or in hardware emulation. Neither libfrt nor the host program
//! links XRT, so software simulation and cosimulation need no XRT install.

use std::ffi::{c_char, c_int, c_uint, c_void};
use std::sync::OnceLock;

use crate::dl::dl_sym;
use crate::error::{FrtError, Result};

/// An opaque XRT handle (device, kernel, run, buffer or graph).
pub type Handle = *mut c_void;

/// `xclBOSyncDirection`.
pub(super) const SYNC_TO_DEVICE: c_int = 0;
pub(super) const SYNC_FROM_DEVICE: c_int = 1;

/// `ert_cmd_state` values a run reports.
pub(super) const RUN_COMPLETED: c_int = 4;
pub(super) const RUN_ERROR: c_int = 5;
pub(super) const RUN_ABORT: c_int = 6;
pub(super) const RUN_TIMEOUT: c_int = 8;
pub(super) const RUN_NORESPONSE: c_int = 9;
pub(super) const RUN_SKERROR: c_int = 10;
pub(super) const RUN_SKCRASHED: c_int = 11;

/// The entry points frt uses, resolved once per process.
pub struct Api {
    pub device_open: unsafe extern "C" fn(c_uint) -> Handle,
    pub device_open_by_bdf: unsafe extern "C" fn(*const c_char) -> Handle,
    pub device_close: unsafe extern "C" fn(Handle) -> c_int,
    pub device_load_xclbin_file: unsafe extern "C" fn(Handle, *const c_char) -> c_int,
    pub device_get_xclbin_uuid: unsafe extern "C" fn(Handle, *mut u8) -> c_int,
    pub kernel_open: unsafe extern "C" fn(Handle, *const u8, *const c_char) -> Handle,
    pub kernel_close: unsafe extern "C" fn(Handle) -> c_int,
    pub kernel_arg_group_id: unsafe extern "C" fn(Handle, c_int) -> c_int,
    pub bo_alloc: unsafe extern "C" fn(Handle, usize, u64, u32) -> Handle,
    pub bo_alloc_user_ptr: unsafe extern "C" fn(Handle, *mut c_void, usize, u64, u32) -> Handle,
    pub bo_free: unsafe extern "C" fn(Handle) -> c_int,
    pub bo_write: unsafe extern "C" fn(Handle, *const c_void, usize, usize) -> c_int,
    pub bo_read: unsafe extern "C" fn(Handle, *mut c_void, usize, usize) -> c_int,
    pub bo_sync: unsafe extern "C" fn(Handle, c_int, usize, usize) -> c_int,
    pub run_open: unsafe extern "C" fn(Handle) -> Handle,
    pub run_close: unsafe extern "C" fn(Handle) -> c_int,
    /// Variadic: XRT reads the value by the argument's type in the xclbin
    /// (see [`super::device`]'s scalar lowering).
    pub run_set_arg: unsafe extern "C" fn(Handle, c_int, ...) -> c_int,
    pub run_start: unsafe extern "C" fn(Handle) -> c_int,
    pub run_state: unsafe extern "C" fn(Handle) -> c_int,
    pub run_wait: unsafe extern "C" fn(Handle) -> c_int,
    pub graph_open: unsafe extern "C" fn(Handle, *const u8, *const c_char) -> Handle,
    pub graph_close: unsafe extern "C" fn(Handle),
    pub graph_reset: unsafe extern "C" fn(Handle) -> c_int,
    /// Iterations: `-1` runs forever.
    pub graph_run: unsafe extern "C" fn(Handle, c_int) -> c_int,
    /// Waits `cycles` AI Engine cycles since the run started, then stops
    /// the graph; `0` waits for it to finish.
    pub graph_wait: unsafe extern "C" fn(Handle, u64) -> c_int,
    pub graph_update_rtp:
        unsafe extern "C" fn(Handle, *const c_char, *const c_char, usize) -> c_int,
}

// SAFETY: plain function pointers into a library loaded for the process's
// lifetime, callable from any thread.
unsafe impl Send for Api {}
// SAFETY: as above; the struct is immutable after loading.
unsafe impl Sync for Api {}

/// XRT's core library by its soname.
const LIBRARY: &str = "libxrt_coreutil.so.2";

/// The XRT API, loading `libxrt_coreutil` on first use.
pub fn api() -> Result<&'static Api> {
    static API: OnceLock<std::result::Result<Api, String>> = OnceLock::new();
    API.get_or_init(load)
        .as_ref()
        .map_err(|reason| FrtError::NoXrtRuntime(reason.clone()))
}

/// Where the library is looked for, in order: the install `XILINX_XRT`
/// names, the loader's search path, then XRT's default prefix. XRT's
/// `setup.sh` puts its `lib` on `LD_LIBRARY_PATH`, but that only reaches
/// processes started after it is sourced.
fn candidates() -> Vec<String> {
    let installed = std::env::var_os("XILINX_XRT")
        .filter(|root| !root.is_empty())
        .map(|root| std::path::Path::new(&root).join("lib").join(LIBRARY));
    crate::dl::candidates_with_override(
        installed.as_deref().map(std::path::Path::as_os_str),
        [LIBRARY.to_owned(), format!("/opt/xilinx/xrt/lib/{LIBRARY}")],
    )
}

fn load() -> std::result::Result<Api, String> {
    let lib = crate::dl::open_library(
        &candidates(),
        libc::RTLD_NOW | libc::RTLD_GLOBAL,
        &format!("cannot load {LIBRARY} (is XRT installed and `XILINX_XRT` set?)"),
    )?;
    // Each field's type is the C signature of the XRT symbol of that name
    // (xrt_device.h, xrt_kernel.h, xrt_bo.h, xrt_graph.h).
    Ok(Api {
        device_open: dl_sym!(lib, LIBRARY, "xrtDeviceOpen"),
        device_open_by_bdf: dl_sym!(lib, LIBRARY, "xrtDeviceOpenByBDF"),
        device_close: dl_sym!(lib, LIBRARY, "xrtDeviceClose"),
        device_load_xclbin_file: dl_sym!(lib, LIBRARY, "xrtDeviceLoadXclbinFile"),
        device_get_xclbin_uuid: dl_sym!(lib, LIBRARY, "xrtDeviceGetXclbinUUID"),
        kernel_open: dl_sym!(lib, LIBRARY, "xrtPLKernelOpen"),
        kernel_close: dl_sym!(lib, LIBRARY, "xrtKernelClose"),
        kernel_arg_group_id: dl_sym!(lib, LIBRARY, "xrtKernelArgGroupId"),
        bo_alloc: dl_sym!(lib, LIBRARY, "xrtBOAlloc"),
        bo_alloc_user_ptr: dl_sym!(lib, LIBRARY, "xrtBOAllocUserPtr"),
        bo_free: dl_sym!(lib, LIBRARY, "xrtBOFree"),
        bo_write: dl_sym!(lib, LIBRARY, "xrtBOWrite"),
        bo_read: dl_sym!(lib, LIBRARY, "xrtBORead"),
        bo_sync: dl_sym!(lib, LIBRARY, "xrtBOSync"),
        run_open: dl_sym!(lib, LIBRARY, "xrtRunOpen"),
        run_close: dl_sym!(lib, LIBRARY, "xrtRunClose"),
        run_set_arg: dl_sym!(lib, LIBRARY, "xrtRunSetArg"),
        run_start: dl_sym!(lib, LIBRARY, "xrtRunStart"),
        run_state: dl_sym!(lib, LIBRARY, "xrtRunState"),
        run_wait: dl_sym!(lib, LIBRARY, "xrtRunWait"),
        graph_open: dl_sym!(lib, LIBRARY, "xrtGraphOpen"),
        graph_close: dl_sym!(lib, LIBRARY, "xrtGraphClose"),
        graph_reset: dl_sym!(lib, LIBRARY, "xrtGraphReset"),
        graph_run: dl_sym!(lib, LIBRARY, "xrtGraphRun"),
        graph_wait: dl_sym!(lib, LIBRARY, "xrtGraphWait"),
        graph_update_rtp: dl_sym!(lib, LIBRARY, "xrtGraphUpdateRTP"),
    })
}
