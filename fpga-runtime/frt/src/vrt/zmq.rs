//! The libzmq C API used by SLASH simulation, loaded on first simulation use.
//!
//! The library is deliberately kept loaded for the process lifetime. This is
//! independent of the libvrtd loader: opening a simulation VBIN never resolves
//! or initializes a hardware VRT symbol.

use std::ffi::{c_char, c_int, c_void, CStr, CString, OsStr};
use std::ptr;
use std::sync::OnceLock;

use crate::error::{FrtError, Result};

const LIBRARY: &str = "libzmq.so.5";
const ZMQ_REQ: c_int = 3;
const ZMQ_SNDMORE: c_int = 2;
const ZMQ_LINGER: c_int = 17;
const ZMQ_RCVTIMEO: c_int = 27;
const ZMQ_SNDTIMEO: c_int = 28;
const IO_TIMEOUT_MS: c_int = 60_000;

/// A request is one JSON frame, optionally followed by one binary frame.
/// Keeping this interface below JSON lets tests inspect the exact wire frames.
pub(super) trait FrameTransport: Send {
    fn request(&mut self, frames: &[&[u8]]) -> Result<Vec<u8>>;
    fn close(&mut self);
}

struct Api {
    ctx_new: unsafe extern "C" fn() -> *mut c_void,
    socket: unsafe extern "C" fn(*mut c_void, c_int) -> *mut c_void,
    connect: unsafe extern "C" fn(*mut c_void, *const c_char) -> c_int,
    setsockopt: unsafe extern "C" fn(*mut c_void, c_int, *const c_void, usize) -> c_int,
    send: unsafe extern "C" fn(*mut c_void, *const c_void, usize, c_int) -> c_int,
    msg_recv: unsafe extern "C" fn(*mut Message, *mut c_void, c_int) -> c_int,
    msg_init: unsafe extern "C" fn(*mut Message) -> c_int,
    msg_data: unsafe extern "C" fn(*mut Message) -> *mut c_void,
    msg_size: unsafe extern "C" fn(*const Message) -> usize,
    msg_close: unsafe extern "C" fn(*mut Message) -> c_int,
    close: unsafe extern "C" fn(*mut c_void) -> c_int,
    ctx_term: unsafe extern "C" fn(*mut c_void) -> c_int,
    errno: unsafe extern "C" fn() -> c_int,
    strerror: unsafe extern "C" fn(c_int) -> *const c_char,
}

// SAFETY: Api contains immutable function pointers into a library that is
// intentionally never unloaded. libzmq provides the entry-point thread safety.
unsafe impl Send for Api {}
// SAFETY: as above; Api has no interior mutability.
unsafe impl Sync for Api {}

/// `zmq_msg_t` is 64 bytes with pointer alignment in the pinned C header.
/// Eight u64 words provide at least that alignment on supported hosts.
#[repr(C)]
struct Message {
    storage: [u64; 8],
}

impl Message {
    fn new() -> Self {
        Self { storage: [0; 8] }
    }
}

struct MessageGuard {
    api: &'static Api,
    message: Message,
}

impl Drop for MessageGuard {
    fn drop(&mut self) {
        // SAFETY: message was initialized successfully and is closed once.
        let rc = unsafe { (self.api.msg_close)(&raw mut self.message) };
        if rc != 0 {
            tracing::warn!(code = rc, "failed to close a libzmq message");
        }
    }
}

pub(super) struct ZmqTransport {
    api: &'static Api,
    context: *mut c_void,
    socket: *mut c_void,
}

// SAFETY: a transport is moved but never shared. Every socket operation takes
// &mut self, and both native handles are exclusively owned by this value.
unsafe impl Send for ZmqTransport {}

impl ZmqTransport {
    pub(super) fn connect(endpoint: &str) -> Result<Self> {
        let api = api()?;
        let endpoint_c = CString::new(endpoint).map_err(|error| {
            FrtError::Vrt(format!("invalid SLASH simulation endpoint: {error}"))
        })?;
        // SAFETY: no arguments; the returned context is owned below.
        let context = unsafe { (api.ctx_new)() };
        if context.is_null() {
            return Err(native_error(api, "create a libzmq context"));
        }
        let mut this = Self {
            api,
            context,
            socket: ptr::null_mut(),
        };
        // SAFETY: context is live and ZMQ_REQ is the pinned socket type.
        this.socket = unsafe { (api.socket)(this.context, ZMQ_REQ) };
        if this.socket.is_null() {
            return Err(this.error("create a SLASH simulation REQ socket"));
        }
        // A zero linger and bounded I/O keep failed launches and Drop from
        // hanging forever. A running RTL model still gets a minute per reply.
        this.set_option(ZMQ_LINGER, 0)?;
        this.set_option(ZMQ_RCVTIMEO, IO_TIMEOUT_MS)?;
        this.set_option(ZMQ_SNDTIMEO, IO_TIMEOUT_MS)?;
        // SAFETY: socket and endpoint are live; libzmq copies the endpoint.
        let rc = unsafe { (api.connect)(this.socket, endpoint_c.as_ptr()) };
        if rc != 0 {
            return Err(this.error("connect the SLASH simulation REQ socket"));
        }
        Ok(this)
    }

    fn set_option(&mut self, option: c_int, value: c_int) -> Result<()> {
        // SAFETY: socket is live, option expects a C int, and value is borrowed
        // only for this call.
        let rc = unsafe {
            (self.api.setsockopt)(
                self.socket,
                option,
                (&raw const value).cast(),
                std::mem::size_of::<c_int>(),
            )
        };
        if rc == 0 {
            Ok(())
        } else {
            Err(self.error("configure the SLASH simulation REQ socket"))
        }
    }

    fn error(&self, action: &str) -> FrtError {
        native_error(self.api, action)
    }

    fn receive(&mut self) -> Result<Vec<u8>> {
        let mut message = Message::new();
        // SAFETY: message points to 64 suitably aligned writable bytes.
        let rc = unsafe { (self.api.msg_init)(&raw mut message) };
        if rc != 0 {
            return Err(self.error("initialize a libzmq reply message"));
        }
        let mut guard = MessageGuard {
            api: self.api,
            message,
        };
        // SAFETY: socket and initialized message are live and exclusively used.
        let rc = unsafe { (self.api.msg_recv)(&raw mut guard.message, self.socket, 0) };
        if rc < 0 {
            return Err(self.error("receive a SLASH simulation reply"));
        }
        // SAFETY: the message remains initialized in guard.
        let size = unsafe { (self.api.msg_size)(&raw const guard.message) };
        if size == 0 {
            return Ok(Vec::new());
        }
        // SAFETY: the message remains initialized in guard.
        let data = unsafe { (self.api.msg_data)(&raw mut guard.message) };
        if data.is_null() {
            return Err(FrtError::Vrt(
                "libzmq returned a nonempty reply with no data".into(),
            ));
        }
        // SAFETY: libzmq owns `size` readable bytes until guard closes the
        // message; to_vec copies them before that happens.
        Ok(unsafe { std::slice::from_raw_parts(data.cast::<u8>(), size) }.to_vec())
    }

    fn shutdown(&mut self) {
        if !self.socket.is_null() {
            // SAFETY: this is the owned socket, closed once before its context.
            let rc = unsafe { (self.api.close)(self.socket) };
            if rc != 0 {
                tracing::warn!(code = rc, "failed to close the SLASH simulation socket");
            }
            self.socket = ptr::null_mut();
        }
        if !self.context.is_null() {
            // SAFETY: all sockets are closed and this context is terminated once.
            let rc = unsafe { (self.api.ctx_term)(self.context) };
            if rc != 0 {
                tracing::warn!(
                    code = rc,
                    "failed to terminate the SLASH simulation context"
                );
            }
            self.context = ptr::null_mut();
        }
    }
}

impl FrameTransport for ZmqTransport {
    fn request(&mut self, frames: &[&[u8]]) -> Result<Vec<u8>> {
        if self.socket.is_null() {
            return Err(FrtError::Vrt("SLASH simulation transport is closed".into()));
        }
        if frames.is_empty() || frames.len() > 2 {
            return Err(FrtError::Vrt(format!(
                "SLASH simulation requests need one or two frames, got {}",
                frames.len()
            )));
        }
        for (index, frame) in frames.iter().enumerate() {
            let flags = if index + 1 == frames.len() {
                0
            } else {
                ZMQ_SNDMORE
            };
            // SAFETY: socket is live and frame is readable for its full length.
            // libzmq copies request data before returning from zmq_send.
            let rc =
                unsafe { (self.api.send)(self.socket, frame.as_ptr().cast(), frame.len(), flags) };
            if rc < 0 {
                return Err(self.error("send a SLASH simulation request frame"));
            }
            if usize::try_from(rc).ok() != Some(frame.len()) {
                return Err(FrtError::Vrt(format!(
                    "libzmq sent {rc} of {} bytes in request frame {index}",
                    frame.len()
                )));
            }
        }
        self.receive()
    }

    fn close(&mut self) {
        self.shutdown();
    }
}

impl Drop for ZmqTransport {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn native_error(api: &Api, action: &str) -> FrtError {
    // SAFETY: zmq_errno takes no arguments and is valid after a failed call.
    let errno = unsafe { (api.errno)() };
    // SAFETY: strerror returns a borrowed C string for this libzmq error.
    let text = unsafe { (api.strerror)(errno) };
    let reason = if text.is_null() {
        "unknown libzmq error".to_owned()
    } else {
        // SAFETY: a non-null strerror result is NUL-terminated.
        unsafe { CStr::from_ptr(text) }
            .to_string_lossy()
            .into_owned()
    };
    FrtError::Vrt(format!("cannot {action}: {reason} (libzmq errno {errno})"))
}

fn api() -> Result<&'static Api> {
    static API: OnceLock<std::result::Result<Api, String>> = OnceLock::new();
    API.get_or_init(load)
        .as_ref()
        .map_err(|reason| FrtError::Vrt(reason.clone()))
}

fn candidates() -> Vec<String> {
    candidates_with_override(std::env::var_os(frt_shm::env::FRT_ZMQ_LIBRARY).as_deref())
}

fn candidates_with_override(override_path: Option<&OsStr>) -> Vec<String> {
    let mut paths = Vec::new();
    if let Some(path) = override_path.filter(|path| !path.is_empty()) {
        paths.push(path.to_string_lossy().into_owned());
    }
    paths.push(LIBRARY.to_owned());
    paths
}

fn open_library() -> std::result::Result<*mut c_void, String> {
    let mut reasons = Vec::new();
    for candidate in candidates() {
        let name = CString::new(candidate.clone())
            .map_err(|error| format!("bad libzmq path `{candidate}`: {error}"))?;
        // SAFETY: name is NUL-terminated. A successful handle is intentionally
        // leaked so all resolved entry points remain valid.
        let library = unsafe { libc::dlopen(name.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
        if !library.is_null() {
            return Ok(library);
        }
        // SAFETY: dlerror returns null or a borrowed thread-local C string.
        let error = unsafe { libc::dlerror() };
        let reason = if error.is_null() {
            "unknown loader error".to_owned()
        } else {
            // SAFETY: a non-null dlerror result is a C string, copied here.
            unsafe { CStr::from_ptr(error) }
                .to_string_lossy()
                .into_owned()
        };
        reasons.push(format!("{candidate}: {reason}"));
    }
    Err(format!(
        "cannot load {LIBRARY}; install libzmq or set {} to its path: {}",
        frt_shm::env::FRT_ZMQ_LIBRARY,
        reasons.join("; ")
    ))
}

fn load() -> std::result::Result<Api, String> {
    let library = open_library()?;
    macro_rules! sym {
        ($name:literal) => {
            // SAFETY: each initialized field has the symbol's pinned C type.
            unsafe { resolve(library, $name) }?
        };
    }
    Ok(Api {
        ctx_new: sym!("zmq_ctx_new"),
        socket: sym!("zmq_socket"),
        connect: sym!("zmq_connect"),
        setsockopt: sym!("zmq_setsockopt"),
        send: sym!("zmq_send"),
        msg_recv: sym!("zmq_msg_recv"),
        msg_init: sym!("zmq_msg_init"),
        msg_data: sym!("zmq_msg_data"),
        msg_size: sym!("zmq_msg_size"),
        msg_close: sym!("zmq_msg_close"),
        close: sym!("zmq_close"),
        ctx_term: sym!("zmq_ctx_term"),
        errno: sym!("zmq_errno"),
        strerror: sym!("zmq_strerror"),
    })
}

/// Resolve one symbol as its pinned C function type.
///
/// # Safety
/// `F` must exactly match the named symbol's C signature.
unsafe fn resolve<F: Copy>(library: *mut c_void, name: &str) -> std::result::Result<F, String> {
    assert_eq!(
        std::mem::size_of::<F>(),
        std::mem::size_of::<*mut c_void>(),
        "F must be a function pointer"
    );
    let symbol = CString::new(name).map_err(|error| format!("bad symbol `{name}`: {error}"))?;
    // SAFETY: library is a live dlopen handle and symbol is NUL-terminated.
    let pointer = unsafe { libc::dlsym(library, symbol.as_ptr()) };
    if pointer.is_null() {
        return Err(format!(
            "{LIBRARY} has no `{name}` required by the pinned C API"
        ));
    }
    // SAFETY: the caller guarantees F is the symbol's function pointer type;
    // the size assertion rules out a data pointer or differently sized value.
    Ok(unsafe { std::mem::transmute_copy::<*mut c_void, F>(&pointer) })
}

#[cfg(test)]
mod tests {
    use super::{candidates_with_override, Message, LIBRARY};
    use std::ffi::OsStr;

    #[test]
    fn an_explicit_library_precedes_only_the_versioned_soname() {
        assert_eq!(
            candidates_with_override(Some(OsStr::new("/sim/libzmq-custom.so"))),
            vec!["/sim/libzmq-custom.so", LIBRARY]
        );
        assert_eq!(candidates_with_override(None), vec![LIBRARY]);
        assert_eq!(
            candidates_with_override(Some(OsStr::new(""))),
            vec![LIBRARY]
        );
    }

    #[test]
    fn message_matches_the_pinned_opaque_c_layout() {
        assert_eq!(std::mem::size_of::<Message>(), 64);
        assert!(std::mem::align_of::<Message>() >= std::mem::align_of::<usize>());
    }
}
