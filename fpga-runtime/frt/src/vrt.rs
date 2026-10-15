//! VRT hardware execution through the optional C++ adapter `libfrt_vrt.so`.
//!
//! The adapter is built against the board host's VRT installation. Loading it
//! only for VBINs keeps software simulation and XRT independent of VRT's ABI.

use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::path::Path;

use crate::device::{BufferAccess, Device};
use crate::error::{FrtError, Result};

type Handle = *mut c_void;
type Operation = unsafe extern "C" fn(Handle) -> c_int;
type Time = unsafe extern "C" fn(Handle) -> u64;

struct Api {
    library: Handle,
    open: unsafe extern "C" fn(*const c_char, *const c_char, *const c_char) -> Handle,
    error: unsafe extern "C" fn() -> *const c_char,
    close: Operation,
    scalar: unsafe extern "C" fn(Handle, u32, *const u8, usize) -> c_int,
    buffer: unsafe extern "C" fn(Handle, u32, *mut u8, usize, c_int) -> c_int,
    write: Operation,
    read: Operation,
    exec: Operation,
    finish: Operation,
    poll: Operation,
    load_ns: Time,
    compute_ns: Time,
    store_ns: Time,
}

fn cstring(value: &str) -> Result<CString> {
    CString::new(value).map_err(|error| FrtError::Vrt(error.to_string()))
}

impl Api {
    fn load() -> Result<Self> {
        let path = std::env::var(frt_shm::env::FRT_VRT_LIBRARY)
            .unwrap_or_else(|_| "libfrt_vrt.so".to_owned());
        let name = cstring(&path)?;
        // SAFETY: name is a live NUL-terminated string; symbols are resolved
        // eagerly and remain loaded until the last native instance is closed.
        let library = unsafe { libc::dlopen(name.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
        if library.is_null() {
            return Err(FrtError::Vrt(format!(
                "cannot load `{path}`; build fpga-runtime/frt/vrt against the installed \
                 VRT and set FRT_VRT_LIBRARY to libfrt_vrt.so"
            )));
        }
        macro_rules! symbol {
            ($name:literal, $ty:ty) => {{
                // SAFETY: library is a successful dlopen handle, and concat
                // supplies a NUL-terminated static symbol name.
                let pointer = unsafe {
                    libc::dlsym(library, concat!("frt_vrt_", $name, "\0").as_ptr().cast())
                };
                if pointer.is_null() {
                    // SAFETY: no instance was opened; this load owns the handle.
                    unsafe { libc::dlclose(library) };
                    return Err(FrtError::Vrt(
                        concat!("missing VRT adapter symbol: ", $name).into(),
                    ));
                }
                // SAFETY: bridge.cpp exports this symbol with exactly this C
                // signature. The library is held by Api for every call.
                unsafe { std::mem::transmute::<Handle, $ty>(pointer) }
            }};
        }
        Ok(Self {
            library,
            open: symbol!(
                "open",
                unsafe extern "C" fn(*const c_char, *const c_char, *const c_char) -> Handle
            ),
            error: symbol!("error", unsafe extern "C" fn() -> *const c_char),
            close: symbol!("close", Operation),
            scalar: symbol!(
                "scalar",
                unsafe extern "C" fn(Handle, u32, *const u8, usize) -> c_int
            ),
            buffer: symbol!(
                "buffer",
                unsafe extern "C" fn(Handle, u32, *mut u8, usize, c_int) -> c_int
            ),
            write: symbol!("write", Operation),
            read: symbol!("read", Operation),
            exec: symbol!("exec", Operation),
            finish: symbol!("finish", Operation),
            poll: symbol!("poll", Operation),
            load_ns: symbol!("load_ns", Time),
            compute_ns: symbol!("compute_ns", Time),
            store_ns: symbol!("store_ns", Time),
        })
    }

    fn check(&self, code: c_int) -> Result<()> {
        if code >= 0 {
            return Ok(());
        }
        // SAFETY: the adapter owns a thread-local, NUL-terminated error string
        // which remains valid until the next adapter call on this thread.
        let error = unsafe { (self.error)() };
        // SAFETY: bridge.cpp always returns its thread-local string, not null.
        let message = unsafe { CStr::from_ptr(error) }
            .to_string_lossy()
            .into_owned();
        Err(FrtError::Vrt(message))
    }
}

impl Drop for Api {
    fn drop(&mut self) {
        // SAFETY: VrtDevice closes its instance before dropping this owner.
        unsafe { libc::dlclose(self.library) };
    }
}

pub struct VrtDevice {
    api: Api,
    handle: Handle,
}

// SAFETY: the native instance is exclusively owned and every mutating method
// requires &mut self. It may move between host scheduler threads but no VRT
// object is accessed concurrently through this handle.
unsafe impl Send for VrtDevice {}

impl VrtDevice {
    pub fn open(path: &Path) -> Result<Self> {
        let required = |name| {
            std::env::var(name)
                .ok()
                .filter(|value| !value.is_empty())
                .ok_or_else(|| FrtError::Vrt(format!("set {name} before opening a VBIN")))
        };
        let bdf = cstring(&required(frt_shm::env::FRT_VRT_BDF)?)?;
        let kernel = cstring(&required(frt_shm::env::FRT_VRT_KERNEL)?)?;
        let path = cstring(
            path.to_str()
                .ok_or_else(|| FrtError::Vrt("non-UTF-8 VBIN path".into()))?,
        )?;
        let api = Api::load()?;
        // SAFETY: all three strings remain live through open, which copies
        // them. The returned native instance is owned by this VrtDevice.
        let handle = unsafe { (api.open)(path.as_ptr(), bdf.as_ptr(), kernel.as_ptr()) };
        if handle.is_null() {
            api.check(-1)?;
        }
        Ok(Self { api, handle })
    }

    fn call(&mut self, operation: Operation) -> Result<()> {
        // SAFETY: the handle is owned and live, and Api holds the loaded code.
        self.api.check(unsafe { operation(self.handle) })
    }

    fn time(&self, operation: Time) -> u64 {
        // SAFETY: time getters only read counters on this live native instance.
        unsafe { operation(self.handle) }
    }
}

impl Drop for VrtDevice {
    fn drop(&mut self) {
        // SAFETY: close consumes the native instance once, after waiting for
        // outstanding device accesses. Api remains live throughout the call.
        let result = unsafe { (self.api.close)(self.handle) };
        if let Err(error) = self.api.check(result) {
            eprintln!("VRT close failed; native allocations were retained: {error}");
        }
    }
}

impl Device for VrtDevice {
    fn set_scalar_arg(&mut self, index: u32, value: &[u8]) -> Result<()> {
        // SAFETY: value is live for this call; the adapter copies its bytes.
        self.api
            .check(unsafe { (self.api.scalar)(self.handle, index, value.as_ptr(), value.len()) })
    }

    fn set_buffer_arg(
        &mut self,
        index: u32,
        ptr: *mut u8,
        bytes: usize,
        access: BufferAccess,
    ) -> Result<()> {
        // SAFETY: the FRT caller owns this host region until finish/close, as
        // required by the same raw-buffer API used by XRT and cosimulation.
        self.api
            .check(unsafe { (self.api.buffer)(self.handle, index, ptr, bytes, access as c_int) })
    }

    fn set_stream_arg(&mut self, _index: u32, _shm_path: &str) -> Result<()> {
        Err(FrtError::Vrt(
            "VBIN execution does not support host stream arguments".into(),
        ))
    }

    fn write_to_device(&mut self) -> Result<()> {
        self.call(self.api.write)
    }
    fn read_from_device(&mut self) -> Result<()> {
        self.call(self.api.read)
    }
    fn exec(&mut self) -> Result<()> {
        self.call(self.api.exec)
    }
    fn finish(&mut self) -> Result<()> {
        self.call(self.api.finish)
    }

    fn kill(&mut self) -> Result<()> {
        Err(FrtError::Vrt(
            "VRT does not support forced kernel cancellation".into(),
        ))
    }

    fn is_finished(&mut self) -> Result<bool> {
        // SAFETY: poll accesses only the owned native instance.
        let result = unsafe { (self.api.poll)(self.handle) };
        self.api.check(result)?;
        Ok(result != 0)
    }

    fn load_ns(&self) -> u64 {
        self.time(self.api.load_ns)
    }
    fn compute_ns(&self) -> u64 {
        self.time(self.api.compute_ns)
    }
    fn store_ns(&self) -> u64 {
        self.time(self.api.store_ns)
    }
}
