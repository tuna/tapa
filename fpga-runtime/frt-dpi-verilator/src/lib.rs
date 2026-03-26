use frt_dpi::{axi, get_or_init, stream};

#[no_mangle]
pub unsafe extern "C" fn tapa_axi_read(
    port: *const libc::c_char,
    addr: u64,
    width: u32,
    out: *mut u8,
) {
    let port = std::ffi::CStr::from_ptr(port).to_str().unwrap_or("");
    axi::axi_read_impl(get_or_init(), port, addr, width, out);
}

#[no_mangle]
pub unsafe extern "C" fn tapa_axi_write(
    port: *const libc::c_char,
    addr: u64,
    width: u32,
    data: *const u8,
) {
    let port = std::ffi::CStr::from_ptr(port).to_str().unwrap_or("");
    axi::axi_write_impl(get_or_init(), port, addr, width, data);
}

#[no_mangle]
pub unsafe extern "C" fn tapa_stream_try_read(port: *const libc::c_char, out: *mut u8) -> bool {
    let port = std::ffi::CStr::from_ptr(port).to_str().unwrap_or("");
    stream::stream_try_read_impl(get_or_init(), port, out)
}

#[no_mangle]
pub unsafe extern "C" fn tapa_stream_try_write(port: *const libc::c_char, data: *const u8) -> bool {
    let port = std::ffi::CStr::from_ptr(port).to_str().unwrap_or("");
    stream::stream_try_write_impl(get_or_init(), port, data)
}
