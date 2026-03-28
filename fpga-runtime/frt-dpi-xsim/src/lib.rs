#[cfg(target_os = "linux")]
mod imp {
    use frt_dpi::{axi, get_or_init, stream};

    // svOpenArrayHandle is an opaque pointer type from svdpi.h.
    // In IEEE 1800 DPI-C, SV open arrays (e.g. `byte unsigned out[]`)
    // are passed as svOpenArrayHandle, NOT as raw pointers.
    type SvOpenArrayHandle = *mut libc::c_void;

    extern "C" {
        /// Returns a raw pointer to the array data inside an svOpenArrayHandle.
        /// For `byte unsigned[]`, this gives a `*mut u8`.
        fn svGetArrayPtr(h: SvOpenArrayHandle) -> *mut libc::c_void;
    }

    #[no_mangle]
    pub unsafe extern "C" fn tapa_axi_read(
        port: *const libc::c_char,
        addr: u64,
        width: u32,
        out: SvOpenArrayHandle,
    ) {
        let port = std::ffi::CStr::from_ptr(port).to_str().unwrap_or("");
        let ptr = svGetArrayPtr(out) as *mut u8;
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
        let ptr = svGetArrayPtr(data) as *const u8;
        axi::axi_write_impl(get_or_init(), port, addr, width, ptr);
    }

    #[no_mangle]
    pub unsafe extern "C" fn tapa_stream_try_read(
        port: *const libc::c_char,
        out: SvOpenArrayHandle,
    ) -> bool {
        let port = std::ffi::CStr::from_ptr(port).to_str().unwrap_or("");
        let ptr = svGetArrayPtr(out) as *mut u8;
        stream::stream_try_read_impl(get_or_init(), port, ptr)
    }

    #[no_mangle]
    pub unsafe extern "C" fn tapa_stream_try_write(
        port: *const libc::c_char,
        data: SvOpenArrayHandle,
    ) -> bool {
        let port = std::ffi::CStr::from_ptr(port).to_str().unwrap_or("");
        let ptr = svGetArrayPtr(data) as *const u8;
        stream::stream_try_write_impl(get_or_init(), port, ptr)
    }

    #[no_mangle]
    pub unsafe extern "C" fn tapa_stream_can_write(port: *const libc::c_char) -> bool {
        let port = std::ffi::CStr::from_ptr(port).to_str().unwrap_or("");
        stream::stream_can_write_impl(get_or_init(), port)
    }
}
