use crate::device::BufferAccess;
use crate::env_bool;
use crate::instance::{Instance, Simulator};
use std::cell::RefCell;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int};
use std::path::Path;

struct FrtInstanceHandle {
    instance: Instance,
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

/// Shared body for the `frt_instance_*` wrappers around fallible,
/// argument-free `Instance` methods: a null handle or an error is recorded
/// via `set_last_error` and mapped to `-1`, success to `0`.
fn instance_method_call(
    handle: *mut std::ffi::c_void,
    method: impl FnOnce(&mut Instance) -> crate::error::Result<()>,
) -> c_int {
    clear_last_error();
    let Some(h) = with_handle_mut(handle) else {
        return -1;
    };
    if let Err(e) = method(&mut h.instance) {
        set_last_error(e.to_string());
        return -1;
    }
    0
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
    let handle = FrtInstanceHandle { instance };
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
pub extern "C" fn frt_instance_write_to_device(handle: *mut std::ffi::c_void) -> c_int {
    instance_method_call(handle, Instance::write_to_device)
}

#[no_mangle]
pub extern "C" fn frt_instance_read_from_device(handle: *mut std::ffi::c_void) -> c_int {
    instance_method_call(handle, Instance::read_from_device)
}

#[no_mangle]
pub extern "C" fn frt_instance_exec(handle: *mut std::ffi::c_void) -> c_int {
    instance_method_call(handle, Instance::exec)
}

#[no_mangle]
pub extern "C" fn frt_instance_finish(handle: *mut std::ffi::c_void) -> c_int {
    instance_method_call(handle, Instance::finish)
}

#[no_mangle]
pub extern "C" fn frt_instance_kill(handle: *mut std::ffi::c_void) -> c_int {
    instance_method_call(handle, Instance::kill)
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
