use crate::device::{BufferAccess, RuntimeArgCategory, RuntimeArgInfo};
use crate::instance::{Instance, Simulator};
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int};
use std::path::Path;
use std::sync::Mutex;

struct FrtInstanceHandle {
    instance: Instance,
    args_cache: Vec<RuntimeArgInfo>,
    arg_name_cache: Option<CString>,
    arg_type_cache: Option<CString>,
}

static LAST_ERROR: Mutex<Option<CString>> = Mutex::new(None);

fn set_last_error(msg: impl Into<String>) {
    let mut text = msg.into();
    if text.contains('\0') {
        text = text.replace('\0', " ");
    }
    if let Ok(mut guard) = LAST_ERROR.lock() {
        *guard = CString::new(text).ok();
    }
}

fn clear_last_error() {
    if let Ok(mut guard) = LAST_ERROR.lock() {
        *guard = None;
    }
}

fn to_str<'a>(ptr: *const c_char, field: &str) -> Result<Option<&'a str>, String> {
    if ptr.is_null() {
        return Ok(None);
    }
    let c = unsafe { CStr::from_ptr(ptr) };
    c.to_str()
        .map(Some)
        .map_err(|e| format!("invalid utf-8 in {field}: {e}"))
}

fn env_bool(name: &str) -> bool {
    match std::env::var(name) {
        Ok(v) => matches!(v.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"),
        Err(_) => false,
    }
}

fn parse_simulator(sim: Option<&str>) -> Simulator {
    match sim.unwrap_or("xsim") {
        "verilator" => Simulator::Verilator,
        "xsim-legacy" | "xsim_legacy" | "legacy-xsim" => Simulator::Xsim { legacy: true },
        _ => Simulator::Xsim {
            legacy: env_bool("FRT_XSIM_LEGACY"),
        },
    }
}

fn parse_buffer_access(tag: c_int) -> BufferAccess {
    match tag {
        0 => BufferAccess::PlaceHolder,
        1 => BufferAccess::ReadOnly,
        2 => BufferAccess::WriteOnly,
        3 => BufferAccess::ReadWrite,
        _ => BufferAccess::ReadWrite,
    }
}

fn cat_to_c_int(cat: RuntimeArgCategory) -> c_int {
    match cat {
        RuntimeArgCategory::Scalar => 0,
        RuntimeArgCategory::Mmap => 1,
        RuntimeArgCategory::Stream => 2,
        RuntimeArgCategory::Streams => 3,
    }
}

fn open_instance(path: &str, sim: Option<&str>) -> Result<Instance, String> {
    let p = Path::new(path);
    match p.extension().and_then(|e| e.to_str()) {
        Some("xo") | Some("zip") => {
            Instance::open_cosim(p, parse_simulator(sim)).map_err(|e| e.to_string())
        }
        _ => Instance::open(p).map_err(|e| e.to_string()),
    }
}

#[no_mangle]
pub extern "C" fn frt_last_error_message() -> *const c_char {
    if let Ok(guard) = LAST_ERROR.lock() {
        if let Some(s) = guard.as_ref() {
            return s.as_ptr();
        }
    }
    std::ptr::null()
}

#[no_mangle]
pub extern "C" fn frt_instance_open(
    path: *const c_char,
    simulator: *const c_char,
) -> *mut std::ffi::c_void {
    clear_last_error();
    let path = match to_str(path, "path") {
        Ok(Some(s)) => s,
        Ok(None) => {
            set_last_error("path is null");
            return std::ptr::null_mut();
        }
        Err(e) => {
            set_last_error(e);
            return std::ptr::null_mut();
        }
    };
    let sim = match to_str(simulator, "simulator") {
        Ok(v) => v,
        Err(e) => {
            set_last_error(e);
            return std::ptr::null_mut();
        }
    };

    let instance = match open_instance(path, sim) {
        Ok(i) => i,
        Err(e) => {
            set_last_error(e);
            return std::ptr::null_mut();
        }
    };
    let handle = FrtInstanceHandle {
        instance,
        args_cache: Vec::new(),
        arg_name_cache: None,
        arg_type_cache: None,
    };
    Box::into_raw(Box::new(handle)) as *mut std::ffi::c_void
}

#[no_mangle]
pub extern "C" fn frt_instance_close(handle: *mut std::ffi::c_void) {
    if handle.is_null() {
        return;
    }
    unsafe {
        let _ = Box::from_raw(handle as *mut FrtInstanceHandle);
    }
}

#[no_mangle]
pub extern "C" fn frt_instance_get_arg_count(
    handle: *mut std::ffi::c_void,
    out_count: *mut u32,
) -> c_int {
    clear_last_error();
    if handle.is_null() {
        set_last_error("handle is null");
        return -1;
    }
    if out_count.is_null() {
        set_last_error("out_count is null");
        return -1;
    }
    let h = unsafe { &mut *(handle as *mut FrtInstanceHandle) };
    h.args_cache = h.instance.args_info();
    unsafe {
        *out_count = h.args_cache.len() as u32;
    }
    0
}

#[no_mangle]
pub extern "C" fn frt_instance_get_arg(
    handle: *mut std::ffi::c_void,
    ordinal: u32,
    out_index: *mut u32,
    out_cat: *mut c_int,
    out_name: *mut *const c_char,
    out_type: *mut *const c_char,
) -> c_int {
    clear_last_error();
    if handle.is_null() {
        set_last_error("handle is null");
        return -1;
    }
    if out_index.is_null() || out_cat.is_null() || out_name.is_null() || out_type.is_null() {
        set_last_error("output pointer is null");
        return -1;
    }
    let h = unsafe { &mut *(handle as *mut FrtInstanceHandle) };
    if h.args_cache.is_empty() {
        h.args_cache = h.instance.args_info();
    }
    let Some(arg) = h.args_cache.get(ordinal as usize).cloned() else {
        set_last_error(format!("arg ordinal out of range: {ordinal}"));
        return -1;
    };
    let Ok(name_cstr) = CString::new(arg.name) else {
        set_last_error("arg name contains interior nul byte");
        return -1;
    };
    let Ok(type_cstr) = CString::new(arg.type_name) else {
        set_last_error("arg type contains interior nul byte");
        return -1;
    };
    h.arg_name_cache = Some(name_cstr);
    h.arg_type_cache = Some(type_cstr);

    unsafe {
        *out_index = arg.index;
        *out_cat = cat_to_c_int(arg.category);
        *out_name = h
            .arg_name_cache
            .as_ref()
            .map(|s| s.as_ptr())
            .unwrap_or(std::ptr::null());
        *out_type = h
            .arg_type_cache
            .as_ref()
            .map(|s| s.as_ptr())
            .unwrap_or(std::ptr::null());
    }
    0
}

#[no_mangle]
pub extern "C" fn frt_instance_set_scalar_bytes(
    handle: *mut std::ffi::c_void,
    index: u32,
    value: *const u8,
    size: usize,
) -> c_int {
    clear_last_error();
    if handle.is_null() {
        set_last_error("handle is null");
        return -1;
    }
    if value.is_null() && size != 0 {
        set_last_error("value is null");
        return -1;
    }
    let bytes = if size == 0 {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(value, size) }
    };
    let h = unsafe { &mut *(handle as *mut FrtInstanceHandle) };
    if let Err(e) = h.instance.set_scalar_arg_bytes(index, bytes) {
        set_last_error(e.to_string());
        return -1;
    }
    0
}

#[no_mangle]
pub extern "C" fn frt_instance_set_buffer_arg(
    handle: *mut std::ffi::c_void,
    index: u32,
    ptr: *mut u8,
    bytes: usize,
) -> c_int {
    frt_instance_set_buffer_arg_typed(handle, index, ptr, bytes, 3)
}

#[no_mangle]
pub extern "C" fn frt_instance_set_buffer_arg_typed(
    handle: *mut std::ffi::c_void,
    index: u32,
    ptr: *mut u8,
    bytes: usize,
    tag: c_int,
) -> c_int {
    clear_last_error();
    if handle.is_null() {
        set_last_error("handle is null");
        return -1;
    }
    if ptr.is_null() && bytes != 0 {
        set_last_error("buffer ptr is null");
        return -1;
    }
    let access = parse_buffer_access(tag);
    let h = unsafe { &mut *(handle as *mut FrtInstanceHandle) };
    if let Err(e) = h
        .instance
        .set_buffer_arg_raw_with_access(index, ptr, bytes, access)
    {
        set_last_error(e.to_string());
        return -1;
    }
    0
}

#[no_mangle]
pub extern "C" fn frt_instance_set_stream_arg(
    handle: *mut std::ffi::c_void,
    index: u32,
    shm_path: *const c_char,
) -> c_int {
    clear_last_error();
    if handle.is_null() {
        set_last_error("handle is null");
        return -1;
    }
    let path = match to_str(shm_path, "shm_path") {
        Ok(Some(s)) => s,
        Ok(None) => "",
        Err(e) => {
            set_last_error(e);
            return -1;
        }
    };
    let h = unsafe { &mut *(handle as *mut FrtInstanceHandle) };
    if let Err(e) = h.instance.set_stream_arg_raw(index, path) {
        set_last_error(e.to_string());
        return -1;
    }
    0
}

#[no_mangle]
pub extern "C" fn frt_instance_suspend_buffer(handle: *mut std::ffi::c_void, index: u32) -> usize {
    clear_last_error();
    if handle.is_null() {
        set_last_error("handle is null");
        return 0;
    }
    let h = unsafe { &mut *(handle as *mut FrtInstanceHandle) };
    h.instance.suspend_buffer(index)
}

#[no_mangle]
pub extern "C" fn frt_instance_write_to_device(handle: *mut std::ffi::c_void) -> c_int {
    clear_last_error();
    if handle.is_null() {
        set_last_error("handle is null");
        return -1;
    }
    let h = unsafe { &mut *(handle as *mut FrtInstanceHandle) };
    if let Err(e) = h.instance.write_to_device() {
        set_last_error(e.to_string());
        return -1;
    }
    0
}

#[no_mangle]
pub extern "C" fn frt_instance_read_from_device(handle: *mut std::ffi::c_void) -> c_int {
    clear_last_error();
    if handle.is_null() {
        set_last_error("handle is null");
        return -1;
    }
    let h = unsafe { &mut *(handle as *mut FrtInstanceHandle) };
    if let Err(e) = h.instance.read_from_device() {
        set_last_error(e.to_string());
        return -1;
    }
    0
}

#[no_mangle]
pub extern "C" fn frt_instance_exec(handle: *mut std::ffi::c_void) -> c_int {
    clear_last_error();
    if handle.is_null() {
        set_last_error("handle is null");
        return -1;
    }
    let h = unsafe { &mut *(handle as *mut FrtInstanceHandle) };
    if let Err(e) = h.instance.exec() {
        set_last_error(e.to_string());
        return -1;
    }
    0
}

#[no_mangle]
pub extern "C" fn frt_instance_finish(handle: *mut std::ffi::c_void) -> c_int {
    clear_last_error();
    if handle.is_null() {
        set_last_error("handle is null");
        return -1;
    }
    let h = unsafe { &mut *(handle as *mut FrtInstanceHandle) };
    if let Err(e) = h.instance.finish() {
        set_last_error(e.to_string());
        return -1;
    }
    0
}

#[no_mangle]
pub extern "C" fn frt_instance_kill(handle: *mut std::ffi::c_void) -> c_int {
    clear_last_error();
    if handle.is_null() {
        set_last_error("handle is null");
        return -1;
    }
    let h = unsafe { &mut *(handle as *mut FrtInstanceHandle) };
    if let Err(e) = h.instance.kill() {
        set_last_error(e.to_string());
        return -1;
    }
    0
}

#[no_mangle]
pub extern "C" fn frt_instance_is_finished(handle: *mut std::ffi::c_void) -> c_int {
    if handle.is_null() {
        return 0;
    }
    let h = unsafe { &mut *(handle as *mut FrtInstanceHandle) };
    match h.instance.is_finished() {
        Ok(done) => {
            if done {
                1
            } else {
                0
            }
        }
        Err(e) => {
            set_last_error(e.to_string());
            1
        }
    }
}

#[no_mangle]
pub extern "C" fn frt_instance_load_ns(handle: *mut std::ffi::c_void) -> u64 {
    if handle.is_null() {
        return 0;
    }
    let h = unsafe { &*(handle as *mut FrtInstanceHandle) };
    h.instance.load_ns()
}

#[no_mangle]
pub extern "C" fn frt_instance_compute_ns(handle: *mut std::ffi::c_void) -> u64 {
    if handle.is_null() {
        return 0;
    }
    let h = unsafe { &*(handle as *mut FrtInstanceHandle) };
    h.instance.compute_ns()
}

#[no_mangle]
pub extern "C" fn frt_instance_store_ns(handle: *mut std::ffi::c_void) -> u64 {
    if handle.is_null() {
        return 0;
    }
    let h = unsafe { &*(handle as *mut FrtInstanceHandle) };
    h.instance.store_ns()
}
