use crate::device::{BufferAccess, RuntimeArgCategory, RuntimeArgInfo};
use crate::env_bool;
use crate::instance::{Instance, Simulator};
use std::cell::RefCell;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int};
use std::path::Path;

struct FrtInstanceHandle {
    instance: Instance,
    args_cache: Vec<RuntimeArgInfo>,
    arg_name_cache: Option<CString>,
    arg_type_cache: Option<CString>,
}

/// Obtain a shared reference to the `FrtInstanceHandle` behind an opaque pointer.
///
/// Returns `None` (and sets the last-error message) when `handle` is null.
fn with_handle_ref(handle: *const std::ffi::c_void) -> Option<&'static FrtInstanceHandle> {
    if handle.is_null() {
        set_last_error("handle is null");
        return None;
    }
    // SAFETY: handle was created by frt_instance_open via Box::into_raw, and
    // the caller guarantees the pointer remains valid for the duration of
    // this call.
    Some(unsafe { &*handle.cast::<FrtInstanceHandle>() })
}

/// Obtain an exclusive reference to the `FrtInstanceHandle` behind an opaque pointer.
///
/// Returns `None` (and sets the last-error message) when `handle` is null.
fn with_handle_mut(handle: *mut std::ffi::c_void) -> Option<&'static mut FrtInstanceHandle> {
    if handle.is_null() {
        set_last_error("handle is null");
        return None;
    }
    // SAFETY: handle was created by frt_instance_open via Box::into_raw, and
    // the caller guarantees exclusive access for the duration of this call.
    Some(unsafe { &mut *handle.cast::<FrtInstanceHandle>() })
}

thread_local! {
    /// Per-thread slot for the most recent FFI error message.
    ///
    /// A thread-local slot (rather than a global mutex) lets concurrent
    /// instances on different threads record errors without racing, and it
    /// keeps the `CString` alive in the slot so `frt_last_error_message`
    /// can hand out a pointer that stays valid until the slot is
    /// overwritten on the same thread.
    static LAST_ERROR: RefCell<Option<CString>> = const { RefCell::new(None) };
}

/// Record an error message on this thread's slot, replacing any previous
/// one. Embedded NUL bytes are blanked so the message survives `CString`.
pub(crate) fn set_last_error(msg: impl Into<String>) {
    let mut text = msg.into();
    if text.contains('\0') {
        text = text.replace('\0', " ");
    }
    LAST_ERROR.with(|cell| *cell.borrow_mut() = CString::new(text).ok());
}

/// Drop any previously recorded error on this thread's slot. Exported
/// functions call this on entry so a stale message from an earlier call is
/// not misattributed to a later, successful one.
pub(crate) fn clear_last_error() {
    LAST_ERROR.with(|cell| *cell.borrow_mut() = None);
}

fn to_str<'a>(ptr: *const c_char, field: &str) -> Result<Option<&'a str>, String> {
    if ptr.is_null() {
        return Ok(None);
    }
    // SAFETY: ptr is non-null (checked above) and the caller guarantees it
    // points to a valid NUL-terminated C string.
    let c = unsafe { CStr::from_ptr(ptr) };
    c.to_str()
        .map(Some)
        .map_err(|e| format!("invalid utf-8 in {field}: {e}"))
}

fn parse_simulator(sim: Option<&str>) -> Result<Simulator, String> {
    match sim.unwrap_or("xsim") {
        "verilator" => Ok(Simulator::Verilator),
        "xsim-legacy" | "xsim_legacy" | "legacy-xsim" => Ok(Simulator::Xsim { legacy: true }),
        "xsim" => Ok(Simulator::Xsim {
            legacy: env_bool(frt_shm::env::FRT_XSIM_LEGACY),
        }),
        other => Err(format!("unknown simulator '{other}'")),
    }
}

fn parse_buffer_access(tag: c_int) -> BufferAccess {
    // frt tags are host-relative; tapa tags are kernel-relative, so they are
    // swapped. kReadOnly(1) means "host reads = kernel writes" → stores_to_host;
    // kWriteOnly(2) means "host writes = kernel reads" → loads_from_host.
    // This matches the old C++ TapaFastCosimDevice::SetBufferArg convention.
    match tag {
        0 => BufferAccess::PlaceHolder,
        1 => BufferAccess::WriteOnly,
        2 => BufferAccess::ReadOnly,
        _ => BufferAccess::ReadWrite,
    }
}

fn cat_to_c_int(cat: RuntimeArgCategory) -> c_int {
    cat as c_int
}

fn open_instance(path: &str, sim: Option<&str>) -> Result<Instance, String> {
    let p = Path::new(path);
    match p.extension().and_then(|e| e.to_str()) {
        Some("xo" | "zip") => {
            Instance::open_cosim(p, &parse_simulator(sim)?).map_err(|e| e.to_string())
        }
        _ => Instance::open(p).map_err(|e| e.to_string()),
    }
}

/// Return the error message recorded by the most recent failed `frt_*`
/// call on this thread, or null if the last call succeeded (or none ran).
///
/// The returned pointer borrows from a per-thread slot; it is valid until
/// the next `frt_*` call on this thread (or thread exit, whichever
/// comes first). Each thread has an independent error slot. Copy the
/// string if it must outlive that point.
#[no_mangle]
pub extern "C" fn frt_last_error_message() -> *const c_char {
    LAST_ERROR.with(|cell| {
        cell.borrow()
            .as_ref()
            .map_or(std::ptr::null(), |s: &CString| s.as_ptr())
    })
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
    Box::into_raw(Box::new(handle)).cast::<std::ffi::c_void>()
}

#[no_mangle]
pub extern "C" fn frt_instance_close(handle: *mut std::ffi::c_void) {
    if handle.is_null() {
        return;
    }
    // SAFETY: handle was created by frt_instance_open via Box::into_raw, and
    // this is the only place that reclaims ownership.  After this call the
    // handle must not be used again.
    let mut h = unsafe { Box::from_raw(handle.cast::<FrtInstanceHandle>()) };
    if !matches!(h.instance.is_finished(), Ok(true)) {
        let _ = h.instance.kill();
    }
}

#[no_mangle]
pub extern "C" fn frt_instance_get_arg_count(
    handle: *mut std::ffi::c_void,
    out_count: *mut u32,
) -> c_int {
    clear_last_error();
    if out_count.is_null() {
        set_last_error("out_count is null");
        return -1;
    }
    let Some(h) = with_handle_mut(handle) else {
        return -1;
    };
    h.args_cache = h.instance.args_info();
    // SAFETY: out_count is non-null (checked above) and the caller guarantees
    // it points to a valid writable u32.
    unsafe { *out_count = h.args_cache.len() as u32 };
    0
}

/// Read the metadata of the kernel argument at `ordinal`.
///
/// `out_name` and `out_type` receive pointers to NUL-terminated strings
/// owned by the instance handle. Each pointer is valid until the next
/// `frt_instance_get_arg` call on the same instance handle, or until the
/// instance is closed with `frt_instance_close`; callers must copy the
/// strings (and only access the handle from one thread at a time) to use
/// them beyond that point.
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
    if out_index.is_null() || out_cat.is_null() || out_name.is_null() || out_type.is_null() {
        set_last_error("output pointer is null");
        return -1;
    }
    let Some(h) = with_handle_mut(handle) else {
        return -1;
    };
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

    // SAFETY: all four output pointers are non-null (checked above) and the
    // caller guarantees they point to valid writable locations.
    unsafe { *out_index = arg.index };
    // SAFETY: out_cat is non-null (checked above).
    unsafe { *out_cat = cat_to_c_int(arg.category) };
    // SAFETY: out_name is non-null (checked above).
    unsafe {
        *out_name = h
            .arg_name_cache
            .as_ref()
            .map_or(std::ptr::null(), |s| s.as_ptr());
    };
    // SAFETY: out_type is non-null (checked above).
    unsafe {
        *out_type = h
            .arg_type_cache
            .as_ref()
            .map_or(std::ptr::null(), |s| s.as_ptr());
    };
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
    if value.is_null() && size != 0 {
        set_last_error("value is null");
        return -1;
    }
    let bytes = if size == 0 {
        &[][..]
    } else {
        // SAFETY: value is non-null (checked above) and size > 0, and the
        // caller guarantees [value..value+size) is a valid readable region.
        unsafe { std::slice::from_raw_parts(value, size) }
    };
    let Some(h) = with_handle_mut(handle) else {
        return -1;
    };
    if let Err(e) = h.instance.set_scalar_arg_bytes(index, bytes) {
        set_last_error(e.to_string());
        return -1;
    }
    0
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
    if ptr.is_null() && bytes != 0 {
        set_last_error("buffer ptr is null");
        return -1;
    }
    let access = parse_buffer_access(tag);
    let Some(h) = with_handle_mut(handle) else {
        return -1;
    };
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
    let path = match to_str(shm_path, "shm_path") {
        Ok(Some(s)) => s,
        Ok(None) => "",
        Err(e) => {
            set_last_error(e);
            return -1;
        }
    };
    let Some(h) = with_handle_mut(handle) else {
        return -1;
    };
    if let Err(e) = h.instance.set_stream_arg_raw(index, path) {
        set_last_error(e.to_string());
        return -1;
    }
    0
}

#[no_mangle]
pub extern "C" fn frt_instance_suspend_buffer(handle: *mut std::ffi::c_void, index: u32) -> usize {
    clear_last_error();
    let Some(h) = with_handle_mut(handle) else {
        return 0;
    };
    h.instance.suspend_buffer(index)
}

#[no_mangle]
pub extern "C" fn frt_instance_write_to_device(handle: *mut std::ffi::c_void) -> c_int {
    clear_last_error();
    let Some(h) = with_handle_mut(handle) else {
        return -1;
    };
    if let Err(e) = h.instance.write_to_device() {
        set_last_error(e.to_string());
        return -1;
    }
    0
}

#[no_mangle]
pub extern "C" fn frt_instance_read_from_device(handle: *mut std::ffi::c_void) -> c_int {
    clear_last_error();
    let Some(h) = with_handle_mut(handle) else {
        return -1;
    };
    if let Err(e) = h.instance.read_from_device() {
        set_last_error(e.to_string());
        return -1;
    }
    0
}

#[no_mangle]
pub extern "C" fn frt_instance_exec(handle: *mut std::ffi::c_void) -> c_int {
    clear_last_error();
    let Some(h) = with_handle_mut(handle) else {
        return -1;
    };
    if let Err(e) = h.instance.exec() {
        set_last_error(e.to_string());
        return -1;
    }
    0
}

#[no_mangle]
pub extern "C" fn frt_instance_pause(handle: *mut std::ffi::c_void) -> c_int {
    clear_last_error();
    let Some(h) = with_handle_mut(handle) else {
        return -1;
    };
    if let Err(e) = h.instance.pause() {
        set_last_error(e.to_string());
        return -1;
    }
    0
}

#[no_mangle]
pub extern "C" fn frt_instance_resume(handle: *mut std::ffi::c_void) -> c_int {
    clear_last_error();
    let Some(h) = with_handle_mut(handle) else {
        return -1;
    };
    if let Err(e) = h.instance.resume() {
        set_last_error(e.to_string());
        return -1;
    }
    0
}

#[no_mangle]
pub extern "C" fn frt_instance_finish(handle: *mut std::ffi::c_void) -> c_int {
    clear_last_error();
    let Some(h) = with_handle_mut(handle) else {
        return -1;
    };
    if let Err(e) = h.instance.finish() {
        set_last_error(e.to_string());
        return -1;
    }
    0
}

#[no_mangle]
pub extern "C" fn frt_instance_kill(handle: *mut std::ffi::c_void) -> c_int {
    clear_last_error();
    let Some(h) = with_handle_mut(handle) else {
        return -1;
    };
    if let Err(e) = h.instance.kill() {
        set_last_error(e.to_string());
        return -1;
    }
    0
}

#[no_mangle]
pub extern "C" fn frt_instance_is_finished(handle: *mut std::ffi::c_void) -> c_int {
    clear_last_error();
    let Some(h) = with_handle_mut(handle) else {
        return 0;
    };
    match h.instance.is_finished() {
        Ok(done) => i32::from(done),
        Err(e) => {
            set_last_error(e.to_string());
            -1
        }
    }
}

#[no_mangle]
pub extern "C" fn frt_instance_load_ns(handle: *mut std::ffi::c_void) -> u64 {
    let Some(h) = with_handle_ref(handle.cast_const()) else {
        return 0;
    };
    h.instance.load_ns()
}

#[no_mangle]
pub extern "C" fn frt_instance_compute_ns(handle: *mut std::ffi::c_void) -> u64 {
    let Some(h) = with_handle_ref(handle.cast_const()) else {
        return 0;
    };
    h.instance.compute_ns()
}

#[no_mangle]
pub extern "C" fn frt_instance_store_ns(handle: *mut std::ffi::c_void) -> u64 {
    let Some(h) = with_handle_ref(handle.cast_const()) else {
        return 0;
    };
    h.instance.store_ns()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simulator_names_parse() {
        assert!(matches!(
            parse_simulator(Some("verilator")),
            Ok(Simulator::Verilator)
        ));
        assert!(matches!(
            parse_simulator(Some("xsim")),
            Ok(Simulator::Xsim { .. })
        ));
        assert!(
            matches!(parse_simulator(None), Ok(Simulator::Xsim { .. })),
            "no name defaults to xsim",
        );
    }

    /// The legacy xsim spellings select the legacy mode directly; support
    /// for older Vivado versions is kept deliberately.
    #[test]
    fn legacy_simulator_spellings_parse_to_legacy_xsim() {
        for legacy in ["xsim-legacy", "xsim_legacy", "legacy-xsim"] {
            assert!(
                matches!(
                    parse_simulator(Some(legacy)),
                    Ok(Simulator::Xsim { legacy: true })
                ),
                "{legacy} must select the legacy xsim mode",
            );
        }
    }

    #[test]
    fn garbage_simulator_name_errors() {
        let err = parse_simulator(Some("not-a-simulator")).expect_err("garbage must error");
        assert_eq!(err, "unknown simulator 'not-a-simulator'");
    }
}
