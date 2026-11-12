//! A stand-in for XRT's C API in tests, which run without XRT.
//!
//! Every device, kernel, run and graph handle points at one [`Fake`], which
//! logs each call made through it and holds the kernel's registers; a
//! buffer handle points at a [`Bo`] that knows its `Fake`. No device
//! opens, and every other call succeeds.

use std::collections::HashMap;
use std::ffi::{c_char, c_int, c_uint, c_void};
use std::sync::Mutex;

use super::api::{Api, GmioApi, Handle};

pub(super) struct Fake {
    calls: Mutex<Vec<String>>,
    registers: Mutex<HashMap<u32, u32>>,
}

impl Fake {
    /// A fake that lives for the rest of the test process, as the handles
    /// a device leaves to its threads may.
    pub fn leak() -> &'static Self {
        Box::leak(Box::new(Self {
            calls: Mutex::new(Vec::new()),
            registers: Mutex::new(HashMap::new()),
        }))
    }

    pub fn handle(&'static self) -> Handle {
        std::ptr::from_ref(self).cast_mut().cast()
    }

    /// The calls so far, each as its name and arguments.
    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().expect("calls").clone()
    }

    /// How many calls so far are to `name`.
    pub fn count(&self, name: &str) -> usize {
        self.calls()
            .iter()
            .filter(|call| call.split(' ').next() == Some(name))
            .count()
    }

    pub fn set_register(&self, offset: u32, value: u32) {
        self.registers
            .lock()
            .expect("registers")
            .insert(offset, value);
    }

    fn log(&self, call: String) {
        self.calls.lock().expect("calls").push(call);
    }
}

/// A buffer object: its fake and its bytes.
struct Bo {
    fake: &'static Fake,
    data: Vec<u8>,
}

/// The fake behind a device, kernel, run or graph handle.
fn fake(handle: Handle) -> &'static Fake {
    // SAFETY: every such handle the fake API hands out is `Fake::handle`.
    unsafe { &*handle.cast::<Fake>() }
}

/// The buffer behind a buffer handle.
fn bo(handle: Handle) -> &'static mut Bo {
    // SAFETY: every buffer handle is a leaked `Bo`, used by one thread at a
    // time as XRT's buffer objects are.
    unsafe { &mut *handle.cast::<Bo>() }
}

fn new_bo(device: Handle, bytes: usize) -> Handle {
    let bo = Box::new(Bo {
        fake: fake(device),
        data: vec![0; bytes],
    });
    Box::into_raw(bo).cast()
}

unsafe extern "C" fn device_open(_: c_uint) -> Handle {
    std::ptr::null_mut()
}
unsafe extern "C" fn device_open_by_bdf(_: *const c_char) -> Handle {
    std::ptr::null_mut()
}
unsafe extern "C" fn device_close(device: Handle) -> c_int {
    fake(device).log("device_close".into());
    0
}
unsafe extern "C" fn device_load_xclbin_file(device: Handle, _: *const c_char) -> c_int {
    fake(device).log("device_load_xclbin_file".into());
    0
}
unsafe extern "C" fn device_get_xclbin_uuid(_: Handle, _: *mut u8) -> c_int {
    0
}
unsafe extern "C" fn kernel_open(device: Handle, _: *const u8, _: *const c_char) -> Handle {
    device
}
unsafe extern "C" fn kernel_arg_offset(_: Handle, index: c_int) -> u32 {
    0x10 + 8 * index.cast_unsigned()
}
unsafe extern "C" fn kernel_read_register(kernel: Handle, offset: u32, value: *mut u32) -> c_int {
    let registers = fake(kernel).registers.lock().expect("registers");
    // SAFETY: the caller passes a writable word.
    unsafe { *value = registers.get(&offset).copied().unwrap_or(0) };
    0
}
unsafe extern "C" fn kernel_write_register(kernel: Handle, offset: u32, value: u32) -> c_int {
    fake(kernel).set_register(offset, value);
    0
}
unsafe extern "C" fn kernel_close(kernel: Handle) -> c_int {
    fake(kernel).log("kernel_close".into());
    0
}
unsafe extern "C" fn kernel_arg_group_id(_: Handle, _: c_int) -> c_int {
    0
}
unsafe extern "C" fn bo_alloc(device: Handle, bytes: usize, _: u64, _: u32) -> Handle {
    fake(device).log(format!("bo_alloc {bytes}"));
    new_bo(device, bytes)
}
unsafe extern "C" fn bo_alloc_user_ptr(
    device: Handle,
    _: *mut c_void,
    bytes: usize,
    _: u64,
    _: u32,
) -> Handle {
    fake(device).log(format!("bo_alloc_user_ptr {bytes}"));
    new_bo(device, bytes)
}
unsafe extern "C" fn bo_free(handle: Handle) -> c_int {
    bo(handle).fake.log("bo_free".into());
    0
}
unsafe extern "C" fn bo_write(handle: Handle, _: *const c_void, bytes: usize, at: usize) -> c_int {
    bo(handle).fake.log(format!("bo_write {bytes} {at}"));
    0
}
unsafe extern "C" fn bo_read(handle: Handle, _: *mut c_void, bytes: usize, at: usize) -> c_int {
    bo(handle).fake.log(format!("bo_read {bytes} {at}"));
    0
}
unsafe extern "C" fn bo_sync(handle: Handle, direction: c_int, bytes: usize, at: usize) -> c_int {
    bo(handle)
        .fake
        .log(format!("bo_sync {direction} {bytes} {at}"));
    0
}
unsafe extern "C" fn bo_map(handle: Handle) -> *mut c_void {
    bo(handle).data.as_mut_ptr().cast()
}
unsafe extern "C" fn bo_address(handle: Handle) -> u64 {
    handle as u64
}
unsafe extern "C" fn run_open(kernel: Handle) -> Handle {
    kernel
}
unsafe extern "C" fn run_close(run: Handle) -> c_int {
    fake(run).log("run_close".into());
    0
}
/// The arguments' types vary with the argument (see `va_scalar`); only
/// the index is logged.
unsafe extern "C" fn run_set_arg(run: Handle, index: c_int, _: ...) -> c_int {
    fake(run).log(format!("run_set_arg {index}"));
    0
}
unsafe extern "C" fn run_start(run: Handle) -> c_int {
    fake(run).log("run_start".into());
    0
}
unsafe extern "C" fn run_state(_: Handle) -> c_int {
    super::api::RUN_COMPLETED
}
unsafe extern "C" fn run_wait(_: Handle) -> c_int {
    super::api::RUN_COMPLETED
}
unsafe extern "C" fn graph_open(device: Handle, _: *const u8, _: *const c_char) -> Handle {
    device
}
unsafe extern "C" fn graph_close(graph: Handle) {
    fake(graph).log("graph_close".into());
}
unsafe extern "C" fn graph_reset(graph: Handle) -> c_int {
    fake(graph).log("graph_reset".into());
    0
}
unsafe extern "C" fn graph_run(graph: Handle, iterations: c_int) -> c_int {
    fake(graph).log(format!("graph_run {iterations}"));
    0
}
unsafe extern "C" fn graph_wait(graph: Handle, cycles: u64) -> c_int {
    fake(graph).log(format!("graph_wait {cycles}"));
    0
}
unsafe extern "C" fn graph_update_rtp(
    graph: Handle,
    name: *const c_char,
    _: *const c_char,
    _: usize,
) -> c_int {
    // SAFETY: the caller passes a C string.
    let name = unsafe { std::ffi::CStr::from_ptr(name) }.to_string_lossy();
    fake(graph).log(format!("graph_update_rtp {name}"));
    0
}
unsafe extern "C" fn sync_bo_nb(
    device: Handle,
    _: Handle,
    _: *const c_char,
    direction: c_int,
    bytes: usize,
    _: usize,
) -> c_int {
    fake(device).log(format!("sync_bo_nb {direction} {bytes}"));
    0
}
unsafe extern "C" fn gmio_wait(_: Handle, _: *const c_char) -> c_int {
    0
}
unsafe extern "C" fn graph_read_rtp(
    _: Handle,
    _: *const c_char,
    value: *mut c_char,
    bytes: usize,
) -> c_int {
    // SAFETY: the caller passes `bytes` writable bytes.
    unsafe { std::ptr::write_bytes(value, 0, bytes) };
    0
}

pub(super) static API: Api = Api {
    device_open,
    device_open_by_bdf,
    device_close,
    device_load_xclbin_file,
    device_get_xclbin_uuid,
    kernel_open,
    kernel_open_exclusive: kernel_open,
    kernel_arg_offset,
    kernel_read_register,
    kernel_write_register,
    kernel_close,
    kernel_arg_group_id,
    bo_alloc,
    bo_alloc_user_ptr,
    bo_free,
    bo_write,
    bo_read,
    bo_sync,
    bo_map,
    bo_address,
    run_open,
    run_close,
    run_set_arg,
    run_start,
    run_state,
    run_wait,
    graph_open,
    graph_close,
    graph_reset,
    graph_run,
    graph_wait,
    graph_update_rtp,
};

pub(super) static GMIO: GmioApi = GmioApi {
    sync_bo_nb,
    wait: gmio_wait,
    graph_read_rtp,
};
