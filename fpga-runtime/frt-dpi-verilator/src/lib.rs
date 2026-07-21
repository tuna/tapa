use frt_dpi::get_or_init;

// Verilator marshalling: raw byte pointers, native `bool` flags/returns.
macro_rules! dpi_fn {
    (fn $name:ident($($arg:ident : $ty:ty),* ; mut $arr:ident) => $impl_fn:expr) => {
        #[no_mangle]
        pub unsafe extern "C" fn $name(port: *const libc::c_char, $($arg: $ty,)* $arr: *mut u8) {
            // SAFETY: `port` is a C string provided by the DPI caller;
            // it remains valid for the duration of this call.
            unsafe {
                let port = std::ffi::CStr::from_ptr(port).to_str().unwrap_or("");
                $impl_fn(get_or_init(), port, $($arg,)* $arr);
            }
        }
    };
    (fn $name:ident($($arg:ident : $ty:ty),* ; const $arr:ident) => $impl_fn:expr) => {
        #[no_mangle]
        pub unsafe extern "C" fn $name(port: *const libc::c_char, $($arg: $ty,)* $arr: *const u8) {
            // SAFETY: `port` is a C string provided by the DPI caller;
            // it remains valid for the duration of this call.
            unsafe {
                let port = std::ffi::CStr::from_ptr(port).to_str().unwrap_or("");
                $impl_fn(get_or_init(), port, $($arg,)* $arr);
            }
        }
    };
    (fn $name:ident(mut $arr:ident) -> ret => $impl_fn:expr) => {
        #[no_mangle]
        pub unsafe extern "C" fn $name(port: *const libc::c_char, $arr: *mut u8) -> bool {
            // SAFETY: `port` is a C string provided by the DPI caller;
            // it remains valid for the duration of this call.
            unsafe {
                let port = std::ffi::CStr::from_ptr(port).to_str().unwrap_or("");
                $impl_fn(get_or_init(), port, $arr)
            }
        }
    };
    (fn $name:ident(const $arr:ident) -> ret => $impl_fn:expr) => {
        #[no_mangle]
        pub unsafe extern "C" fn $name(port: *const libc::c_char, $arr: *const u8) -> bool {
            // SAFETY: `port` is a C string provided by the DPI caller;
            // it remains valid for the duration of this call.
            unsafe {
                let port = std::ffi::CStr::from_ptr(port).to_str().unwrap_or("");
                $impl_fn(get_or_init(), port, $arr)
            }
        }
    };
    (fn $name:ident() -> ret => $impl_fn:expr) => {
        #[no_mangle]
        pub unsafe extern "C" fn $name(port: *const libc::c_char) -> bool {
            // SAFETY: `port` is a C string provided by the DPI caller;
            // it remains valid for the duration of this call.
            unsafe {
                let port = std::ffi::CStr::from_ptr(port).to_str().unwrap_or("");
                $impl_fn(get_or_init(), port)
            }
        }
    };
    (fn $name:ident($flag:ident : flag, mut $arr:ident) -> ret => $impl_fn:expr) => {
        #[no_mangle]
        pub unsafe extern "C" fn $name(port: *const libc::c_char, $flag: bool, $arr: *mut u8) -> bool {
            // SAFETY: `port` is a C string provided by the DPI caller;
            // it remains valid for the duration of this call.
            unsafe {
                let port = std::ffi::CStr::from_ptr(port).to_str().unwrap_or("");
                $impl_fn(get_or_init(), port, $flag, $arr)
            }
        }
    };
    (fn $name:ident($flag:ident : flag, const $arr:ident) -> ret => $impl_fn:expr) => {
        #[no_mangle]
        pub unsafe extern "C" fn $name(port: *const libc::c_char, $flag: bool, $arr: *const u8) -> bool {
            // SAFETY: `port` is a C string provided by the DPI caller;
            // it remains valid for the duration of this call.
            unsafe {
                let port = std::ffi::CStr::from_ptr(port).to_str().unwrap_or("");
                $impl_fn(get_or_init(), port, $flag, $arr)
            }
        }
    };
}

frt_dpi::dpi_inventory!(dpi_fn);

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
