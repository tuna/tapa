#[cfg(target_os = "linux")]
mod imp {
    use frt_dpi::{axi, get_or_init, stream};
    use std::sync::OnceLock;

    // svOpenArrayHandle is an opaque pointer type from svdpi.h.
    // In IEEE 1800 DPI-C, SV open arrays (e.g. `byte unsigned out[]`)
    // are passed as svOpenArrayHandle, NOT as raw pointers.
    type SvOpenArrayHandle = *mut libc::c_void;
    type SvGetArrayPtrFn = unsafe extern "C" fn(SvOpenArrayHandle) -> *mut libc::c_void;

    /// Resolve svGetArrayPtr from the xsim runtime via dlsym at first call.
    /// The function is provided by libxv_simulator_kernel.so which is already
    /// loaded in the xsim process when it dlopen()s this DPI library.
    fn get_sv_get_array_ptr() -> SvGetArrayPtrFn {
        static FUNC: OnceLock<SvGetArrayPtrFn> = OnceLock::new();
        *FUNC.get_or_init(|| {
            // SAFETY: `dlsym(RTLD_DEFAULT, ...)` looks up a symbol in the already-loaded
            // xsim runtime. The symbol name is a valid NUL-terminated string.
            let sym =
                unsafe { libc::dlsym(libc::RTLD_DEFAULT, b"svGetArrayPtr\0".as_ptr() as *const _) };
            if sym.is_null() {
                panic!("frt-dpi-xsim: cannot resolve svGetArrayPtr from xsim runtime");
            }
            // SAFETY: `svGetArrayPtr` has the signature `svOpenArrayHandle -> *mut c_void`
            // which matches `SvGetArrayPtrFn`. The symbol was just successfully resolved.
            unsafe { std::mem::transmute(sym) }
        })
    }

    /// Extract the raw byte pointer from an svOpenArrayHandle.
    unsafe fn sv_array_ptr(h: SvOpenArrayHandle) -> *mut u8 {
        (get_sv_get_array_ptr())(h) as *mut u8
    }

    macro_rules! dpi_fn {
        // AXI: array handle is converted to raw pointer
        (fn $name:ident($($arg:ident : $ty:ty),*; mut $arr:ident) => $impl_fn:expr) => {
            #[no_mangle]
            pub unsafe extern "C" fn $name(
                port: *const libc::c_char, $($arg: $ty,)* $arr: SvOpenArrayHandle,
            ) {
                let port = std::ffi::CStr::from_ptr(port).to_str().unwrap_or("");
                let ptr = sv_array_ptr($arr);
                $impl_fn(get_or_init(), port, $($arg,)* ptr);
            }
        };
        (fn $name:ident($($arg:ident : $ty:ty),*; const $arr:ident) => $impl_fn:expr) => {
            #[no_mangle]
            pub unsafe extern "C" fn $name(
                port: *const libc::c_char, $($arg: $ty,)* $arr: SvOpenArrayHandle,
            ) {
                let port = std::ffi::CStr::from_ptr(port).to_str().unwrap_or("");
                let ptr = sv_array_ptr($arr) as *const u8;
                $impl_fn(get_or_init(), port, $($arg,)* ptr);
            }
        };
        // Stream: array handle converted, u8 args converted from bools, return u8
        (fn $name:ident(mut $arr:ident) -> u8 => $impl_fn:expr) => {
            #[no_mangle]
            pub unsafe extern "C" fn $name(
                port: *const libc::c_char, $arr: SvOpenArrayHandle,
            ) -> u8 {
                let port = std::ffi::CStr::from_ptr(port).to_str().unwrap_or("");
                let ptr = sv_array_ptr($arr);
                $impl_fn(get_or_init(), port, ptr) as u8
            }
        };
        (fn $name:ident(const $arr:ident) -> u8 => $impl_fn:expr) => {
            #[no_mangle]
            pub unsafe extern "C" fn $name(
                port: *const libc::c_char, $arr: SvOpenArrayHandle,
            ) -> u8 {
                let port = std::ffi::CStr::from_ptr(port).to_str().unwrap_or("");
                let ptr = sv_array_ptr($arr) as *const u8;
                $impl_fn(get_or_init(), port, ptr) as u8
            }
        };
        (fn $name:ident($flag:ident : u8, mut $arr:ident) -> u8 => $impl_fn:expr) => {
            #[no_mangle]
            pub unsafe extern "C" fn $name(
                port: *const libc::c_char, $flag: u8, $arr: SvOpenArrayHandle,
            ) -> u8 {
                let port = std::ffi::CStr::from_ptr(port).to_str().unwrap_or("");
                let ptr = sv_array_ptr($arr);
                $impl_fn(get_or_init(), port, $flag != 0, ptr) as u8
            }
        };
        (fn $name:ident($flag:ident : u8, const $arr:ident) -> u8 => $impl_fn:expr) => {
            #[no_mangle]
            pub unsafe extern "C" fn $name(
                port: *const libc::c_char, $flag: u8, $arr: SvOpenArrayHandle,
            ) -> u8 {
                let port = std::ffi::CStr::from_ptr(port).to_str().unwrap_or("");
                let ptr = sv_array_ptr($arr) as *const u8;
                $impl_fn(get_or_init(), port, $flag != 0, ptr) as u8
            }
        };
        // No-array variant (e.g. can_write)
        (fn $name:ident() -> u8 => $impl_fn:expr) => {
            #[no_mangle]
            pub unsafe extern "C" fn $name(port: *const libc::c_char) -> u8 {
                let port = std::ffi::CStr::from_ptr(port).to_str().unwrap_or("");
                $impl_fn(get_or_init(), port) as u8
            }
        };
    }

    dpi_fn!(fn tapa_axi_read(addr: u64, width: u32; mut out) => axi::axi_read_impl);
    dpi_fn!(fn tapa_axi_write(addr: u64, width: u32; const data) => axi::axi_write_impl);
    dpi_fn!(fn tapa_stream_try_read(mut out) -> u8 => stream::stream_try_read_impl);
    dpi_fn!(fn tapa_stream_try_write(const data) -> u8 => stream::stream_try_write_impl);
    dpi_fn!(fn tapa_stream_can_write() -> u8 => stream::stream_can_write_impl);
    dpi_fn!(fn tapa_stream_istream_step(consume: u8, mut out) -> u8 => stream::stream_istream_step_impl);
    dpi_fn!(fn tapa_stream_ostream_step(write: u8, const data) -> u8 => stream::stream_ostream_step_impl);
    dpi_fn!(fn tapa_hls_stream_ostream_step(write: u8, const data) -> u8 => stream::stream_hls_ostream_step_impl);
}
