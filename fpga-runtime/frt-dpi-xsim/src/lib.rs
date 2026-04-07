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
        *FUNC.get_or_init(|| unsafe {
            let sym = libc::dlsym(libc::RTLD_DEFAULT, b"svGetArrayPtr\0".as_ptr() as *const _);
            if sym.is_null() {
                panic!("frt-dpi-xsim: cannot resolve svGetArrayPtr from xsim runtime");
            }
            std::mem::transmute(sym)
        })
    }

    /// Extract the raw byte pointer from an svOpenArrayHandle.
    unsafe fn sv_array_ptr(h: SvOpenArrayHandle) -> *mut u8 {
        (get_sv_get_array_ptr())(h) as *mut u8
    }

    #[no_mangle]
    pub unsafe extern "C" fn tapa_axi_read(
        port: *const libc::c_char,
        addr: u64,
        width: u32,
        out: SvOpenArrayHandle,
    ) {
        let port = std::ffi::CStr::from_ptr(port).to_str().unwrap_or("");
        let ptr = sv_array_ptr(out);
        axi::axi_read_impl(get_or_init(), port, addr, width, ptr);
    }

    #[no_mangle]
    pub unsafe extern "C" fn tapa_axi_write(
        port: *const libc::c_char,
        addr: u64,
        width: u32,
        data: SvOpenArrayHandle,
    ) {
        let port = std::ffi::CStr::from_ptr(port).to_str().unwrap_or("");
        let ptr = sv_array_ptr(data) as *const u8;
        axi::axi_write_impl(get_or_init(), port, addr, width, ptr);
    }

    // SV `bit` maps to `svBit` (unsigned char) in DPI-C, not C `_Bool`.
    // Use u8 to match the exact ABI expected by xsim.
    #[no_mangle]
    pub unsafe extern "C" fn tapa_stream_try_read(
        port: *const libc::c_char,
        out: SvOpenArrayHandle,
    ) -> u8 {
        let port = std::ffi::CStr::from_ptr(port).to_str().unwrap_or("");
        let ptr = sv_array_ptr(out);
        stream::stream_try_read_impl(get_or_init(), port, ptr) as u8
    }

    #[no_mangle]
    pub unsafe extern "C" fn tapa_stream_try_write(
        port: *const libc::c_char,
        data: SvOpenArrayHandle,
    ) -> u8 {
        let port = std::ffi::CStr::from_ptr(port).to_str().unwrap_or("");
        let ptr = sv_array_ptr(data) as *const u8;
        stream::stream_try_write_impl(get_or_init(), port, ptr) as u8
    }

    #[no_mangle]
    pub unsafe extern "C" fn tapa_stream_can_write(port: *const libc::c_char) -> u8 {
        let port = std::ffi::CStr::from_ptr(port).to_str().unwrap_or("");
        stream::stream_can_write_impl(get_or_init(), port) as u8
    }

}
