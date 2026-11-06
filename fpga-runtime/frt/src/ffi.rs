use crate::device::{BufferAccess, Device};
use crate::env_bool;
use crate::instance::{open, ExecutionMode, Simulator};
use std::cell::RefCell;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int};
use std::path::Path;

struct FrtInstanceHandle {
    device: Box<dyn Device>,
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
/// argument-free `Device` methods: a null handle or an error is recorded
/// via `set_last_error` and mapped to `-1`, success to `0`.
fn instance_method_call(
    handle: *mut std::ffi::c_void,
    method: impl FnOnce(&mut dyn Device) -> crate::error::Result<()>,
) -> c_int {
    clear_last_error();
    let Some(h) = with_handle_mut(handle) else {
        return -1;
    };
    if let Err(e) = method(&mut *h.device) {
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
pub fn set_last_error(msg: impl Into<String>) {
    let mut text = msg.into();
    if text.contains('\0') {
        text = text.replace('\0', " ");
    }
    LAST_ERROR.with(|cell| *cell.borrow_mut() = CString::new(text).ok());
}

/// Drop any previously recorded error on this thread's slot. Exported
/// functions call this on entry so a stale message from an earlier call is
/// not misattributed to a later, successful one.
pub fn clear_last_error() {
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
        .map_err(|e| format!("invalid UTF-8 in {field}: {e}"))
}

/// Copy `text` into the caller's buffer as a NUL-terminated C string,
/// truncating to `buf_len - 1` bytes when the buffer is too small; a null
/// buffer or a zero length writes nothing. Callers that cannot accept a
/// truncated value must check the length before calling.
pub fn write_c_string(buf: *mut c_char, buf_len: usize, text: &str) {
    if buf.is_null() || buf_len == 0 {
        return;
    }
    let bytes = text.as_bytes();
    let len = bytes.len().min(buf_len - 1);
    // SAFETY: `buf` is non-null with `buf_len` writable bytes and
    // `len < buf_len`; `bytes` is a valid slice and the two ranges do not
    // overlap.
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf.cast::<u8>(), len) };
    // SAFETY: `len < buf_len`, so `buf.add(len)` is in bounds.
    let nul = unsafe { buf.add(len) };
    // SAFETY: `nul` is a valid, writable byte inside the caller's buffer.
    unsafe { nul.write(0) };
}

fn parse_simulator(sim: Option<&str>) -> Result<Simulator, String> {
    match sim.unwrap_or("xsim") {
        "verilator" => Ok(Simulator::Verilator),
        "xsim-legacy" | "xsim_legacy" | "legacy-xsim" => Ok(Simulator::Xsim { legacy: true }),
        "xsim" => Ok(Simulator::Xsim {
            legacy: env_bool(frt_shm::env::FRT_XSIM_LEGACY),
        }),
        other => Err(format!(
            "unknown simulator `{other}`; expected `xsim` or `verilator`"
        )),
    }
}

fn open_instance(path: &str, sim: Option<&str>) -> Result<Box<dyn Device>, String> {
    let path = Path::new(path);
    // Hardware ignores the simulator name, so only a cosim package may
    // reject an unknown one; a stray -cosim_simulator flag must not break
    // a bitstream run.
    let simulator = match ExecutionMode::of(path).map_err(|e| e.to_string())? {
        ExecutionMode::Hardware | ExecutionMode::Vrt | ExecutionMode::AieSim => {
            Simulator::Xsim { legacy: false }
        }
        ExecutionMode::Cosim => parse_simulator(sim)?,
    };
    open(path, &simulator).map_err(|e| e.to_string())
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

    let device = match open_instance(path, sim) {
        Ok(d) => d,
        Err(e) => {
            set_last_error(e);
            return std::ptr::null_mut();
        }
    };
    let handle = FrtInstanceHandle { device };
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
    if !matches!(h.device.is_finished(), Ok(true)) {
        let _ = h.device.kill();
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
    if let Err(e) = h.device.set_scalar_arg(index, bytes) {
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
        set_last_error("the buffer pointer is null");
        return -1;
    }
    let Some(access) = BufferAccess::from_wire(tag) else {
        set_last_error(format!("unknown buffer access tag {tag}"));
        return -1;
    };
    let Some(h) = with_handle_mut(handle) else {
        return -1;
    };
    if let Err(e) = h.device.set_buffer_arg(index, ptr, bytes, access) {
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
    if let Err(e) = h.device.set_stream_arg(index, path) {
        set_last_error(e.to_string());
        return -1;
    }
    0
}

/// The number of stream arguments the host binds, which
/// [`frt_instance_host_stream`] enumerates; -1 on an invalid handle.
#[no_mangle]
pub extern "C" fn frt_instance_host_stream_count(handle: *const std::ffi::c_void) -> c_int {
    clear_last_error();
    let Some(h) = with_handle_ref(handle) else {
        return -1;
    };
    c_int::try_from(h.device.host_streams().len()).unwrap_or(-1)
}

/// The `nth` stream argument the host binds, as its argument index.
///
/// Its NUL-terminated name goes to `name_out` (truncated to `name_len`
/// bytes) and `*kernel_reads` says whether the kernel reads it. Returns -1
/// when `nth` is out of range or the handle is invalid.
#[no_mangle]
pub extern "C" fn frt_instance_host_stream(
    handle: *const std::ffi::c_void,
    nth: u32,
    name_out: *mut c_char,
    name_len: usize,
    kernel_reads: *mut c_int,
) -> c_int {
    clear_last_error();
    let Some(h) = with_handle_ref(handle) else {
        return -1;
    };
    let streams = h.device.host_streams();
    let Some(stream) = streams.get(nth as usize) else {
        set_last_error(format!(
            "host stream {nth} out of range ({} declared)",
            streams.len()
        ));
        return -1;
    };
    write_c_string(name_out, name_len, &stream.name);
    if !kernel_reads.is_null() {
        // SAFETY: the caller provides a writable `c_int`.
        unsafe { *kernel_reads = c_int::from(stream.kernel_reads) };
    }
    c_int::try_from(stream.index).unwrap_or(-1)
}

/// The number of upper task instances the instance was built with inlined,
/// which [`frt_instance_inlined`] enumerates; -1 on an invalid handle.
#[no_mangle]
pub extern "C" fn frt_instance_inlined_count(handle: *const std::ffi::c_void) -> c_int {
    clear_last_error();
    let Some(h) = with_handle_ref(handle) else {
        return -1;
    };
    c_int::try_from(h.device.inlined_instances().len()).unwrap_or(-1)
}

/// The `nth` upper task instance the instance was built with inlined, as
/// its position among its parent's invokes.
///
/// The NUL-terminated name of the inlined instance that invokes it (empty
/// for the top) goes to `parent_out`, and its own to `name_out`, each
/// truncated to its length. Returns -1 when `nth` is out of range or the
/// handle is invalid.
#[no_mangle]
pub extern "C" fn frt_instance_inlined(
    handle: *const std::ffi::c_void,
    nth: u32,
    parent_out: *mut c_char,
    parent_len: usize,
    name_out: *mut c_char,
    name_len: usize,
) -> c_int {
    clear_last_error();
    let Some(h) = with_handle_ref(handle) else {
        return -1;
    };
    let inlined = h.device.inlined_instances();
    let Some(instance) = inlined.get(nth as usize) else {
        set_last_error(format!(
            "inlined instance {nth} out of range ({} declared)",
            inlined.len()
        ));
        return -1;
    };
    write_c_string(parent_out, parent_len, &instance.parent);
    write_c_string(name_out, name_len, &instance.name);
    c_int::try_from(instance.position).unwrap_or(-1)
}

/// How the instance meets the host's software tasks when it runs only the
/// design's AI Engine graph: 1 relays every stream live, so the host runs
/// only the tasks off the graph's side and closes each stream the instance
/// reads once its software producer returns; 2 replays, so the host runs
/// the whole design and binds a copy of each stream, closed once the
/// software run ends. 0 for any other instance; -1 on an invalid handle.
#[no_mangle]
pub extern "C" fn frt_instance_aie_simulation(handle: *const std::ffi::c_void) -> c_int {
    clear_last_error();
    let Some(h) = with_handle_ref(handle) else {
        return -1;
    };
    match h.device.aie_simulation() {
        None => 0,
        Some(crate::device::AieSimulation::Live) => 1,
        Some(crate::device::AieSimulation::Replay) => 2,
    }
}

#[no_mangle]
pub extern "C" fn frt_instance_write_to_device(handle: *mut std::ffi::c_void) -> c_int {
    instance_method_call(handle, |device| device.write_to_device())
}

#[no_mangle]
pub extern "C" fn frt_instance_read_from_device(handle: *mut std::ffi::c_void) -> c_int {
    instance_method_call(handle, |device| device.read_from_device())
}

#[no_mangle]
pub extern "C" fn frt_instance_exec(handle: *mut std::ffi::c_void) -> c_int {
    instance_method_call(handle, |device| device.exec())
}

#[no_mangle]
pub extern "C" fn frt_instance_finish(handle: *mut std::ffi::c_void) -> c_int {
    instance_method_call(handle, |device| device.finish())
}

#[no_mangle]
pub extern "C" fn frt_instance_kill(handle: *mut std::ffi::c_void) -> c_int {
    instance_method_call(handle, |device| device.kill())
}

#[no_mangle]
pub extern "C" fn frt_instance_is_finished(handle: *mut std::ffi::c_void) -> c_int {
    clear_last_error();
    let Some(h) = with_handle_mut(handle) else {
        return 0;
    };
    match h.device.is_finished() {
        Ok(done) => i32::from(done),
        Err(e) => {
            set_last_error(e.to_string());
            -1
        }
    }
}

/// The launched compute's duration; an invalid handle maps to 0 (the error
/// is still recorded for `frt_last_error_message`).
#[no_mangle]
pub extern "C" fn frt_instance_compute_ns(handle: *mut std::ffi::c_void) -> u64 {
    with_handle_ref(handle.cast_const()).map_or(0, |h| h.device.compute_ns())
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
        assert_eq!(
            err,
            "unknown simulator `not-a-simulator`; expected `xsim` or `verilator`"
        );
    }
}
