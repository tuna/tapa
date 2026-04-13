use frt_dpi::{axi, get_or_init, stream};

macro_rules! dpi_fn {
    (fn $name:ident($($arg:ident : $ty:ty),*) => $impl_fn:expr) => {
        #[no_mangle]
        pub unsafe extern "C" fn $name(port: *const libc::c_char, $($arg: $ty),*) {
            // SAFETY: `port` is a C string provided by the DPI caller;
            // it remains valid for the duration of this call.
            unsafe {
                let port = std::ffi::CStr::from_ptr(port).to_str().unwrap_or("");
                $impl_fn(get_or_init(), port, $($arg),*);
            }
        }
    };
    (fn $name:ident($($arg:ident : $ty:ty),*) -> $ret:ty => $impl_fn:expr) => {
        #[no_mangle]
        pub unsafe extern "C" fn $name(port: *const libc::c_char, $($arg: $ty),*) -> $ret {
            // SAFETY: `port` is a C string provided by the DPI caller;
            // it remains valid for the duration of this call.
            unsafe {
                let port = std::ffi::CStr::from_ptr(port).to_str().unwrap_or("");
                $impl_fn(get_or_init(), port, $($arg),*)
            }
        }
    };
}

dpi_fn!(fn tapa_axi_read(addr: u64, width: u32, out: *mut u8) => axi::axi_read_impl);
dpi_fn!(fn tapa_axi_write(addr: u64, width: u32, data: *const u8) => axi::axi_write_impl);
dpi_fn!(fn tapa_stream_try_read(out: *mut u8) -> bool => stream::stream_try_read_impl);
dpi_fn!(fn tapa_stream_try_write(data: *const u8) -> bool => stream::stream_try_write_impl);
dpi_fn!(fn tapa_stream_can_write() -> bool => stream::stream_can_write_impl);
dpi_fn!(fn tapa_stream_istream_step(consume: bool, out: *mut u8) -> bool => stream::stream_istream_step_impl);
dpi_fn!(fn tapa_stream_ostream_step(write: bool, data: *const u8) -> bool => stream::stream_ostream_step_impl);
dpi_fn!(fn tapa_hls_stream_ostream_step(write: bool, data: *const u8) -> bool => stream::stream_hls_ostream_step_impl);

// Floating-point DPI support for Xilinx IP behavioral models.
// Called from generated SystemVerilog via `import "DPI-C"`.
macro_rules! fp_op {
    ($name:ident, $uint:ty, $float:ty, $op:tt) => {
        #[no_mangle]
        pub extern "C" fn $name(a: $uint, b: $uint) -> $uint {
            (<$float>::from_bits(a) $op <$float>::from_bits(b)).to_bits()
        }
    };
}
fp_op!(fp32_add, u32, f32, +);
fp_op!(fp32_sub, u32, f32, -);
fp_op!(fp32_mul, u32, f32, *);
fp_op!(fp64_add, u64, f64, +);
fp_op!(fp64_sub, u64, f64, -);
fp_op!(fp64_mul, u64, f64, *);
