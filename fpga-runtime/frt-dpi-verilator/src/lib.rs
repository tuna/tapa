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

#[no_mangle]
pub unsafe extern "C" fn tapa_stream_can_write(port: *const libc::c_char) -> bool {
    let port = std::ffi::CStr::from_ptr(port).to_str().unwrap_or("");
    stream::stream_can_write_impl(get_or_init(), port)
}

#[no_mangle]
pub unsafe extern "C" fn tapa_stream_istream_step(
    port: *const libc::c_char,
    consume: bool,
    out: *mut u8,
) -> bool {
    let port = std::ffi::CStr::from_ptr(port).to_str().unwrap_or("");
    stream::stream_istream_step_impl(get_or_init(), port, consume, out)
}

#[no_mangle]
pub unsafe extern "C" fn tapa_stream_ostream_step(
    port: *const libc::c_char,
    write: bool,
    data: *const u8,
) -> bool {
    let port = std::ffi::CStr::from_ptr(port).to_str().unwrap_or("");
    stream::stream_ostream_step_impl(get_or_init(), port, write, data)
}

#[no_mangle]
pub unsafe extern "C" fn tapa_hls_stream_ostream_step(
    port: *const libc::c_char,
    write: bool,
    data: *const u8,
) -> bool {
    let port = std::ffi::CStr::from_ptr(port).to_str().unwrap_or("");
    stream::stream_hls_ostream_step_impl(get_or_init(), port, write, data)
}
