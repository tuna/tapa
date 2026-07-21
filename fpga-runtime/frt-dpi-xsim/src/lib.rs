#[cfg(target_os = "linux")]
mod imp {
    use frt_dpi::get_or_init;
    use std::sync::OnceLock;

    // svOpenArrayHandle is an opaque pointer type from svdpi.h.
    // In IEEE 1800 DPI-C, SV open arrays (e.g. `byte unsigned out[]`)
    // are passed as svOpenArrayHandle, NOT as raw pointers.
    type SvOpenArrayHandle = *mut libc::c_void;
    type SvGetArrayPtrFn = unsafe extern "C" fn(SvOpenArrayHandle) -> *mut libc::c_void;

    /// Resolve `svGetArrayPtr` from the xsim runtime via dlsym at first call.
    /// The function is provided by `libxv_simulator_kernel.so` which is already
    /// loaded in the xsim process when it `dlopen()`s this DPI library.
    fn get_sv_get_array_ptr() -> SvGetArrayPtrFn {
        static FUNC: OnceLock<SvGetArrayPtrFn> = OnceLock::new();
        *FUNC.get_or_init(|| {
            // SAFETY: `dlsym(RTLD_DEFAULT, ...)` looks up a symbol in the already-loaded
            // xsim runtime. The symbol name is a valid NUL-terminated string.
            let sym = unsafe { libc::dlsym(libc::RTLD_DEFAULT, c"svGetArrayPtr".as_ptr().cast()) };
            assert!(
                !sym.is_null(),
                "frt-dpi-xsim: cannot resolve svGetArrayPtr from xsim runtime"
            );
            // SAFETY: `svGetArrayPtr` has the signature `svOpenArrayHandle -> *mut c_void`
            // which matches `SvGetArrayPtrFn`. The symbol was just successfully resolved.
            unsafe { std::mem::transmute(sym) }
        })
    }

    /// Extract the raw byte pointer from an svOpenArrayHandle.
    unsafe fn sv_array_ptr(h: SvOpenArrayHandle) -> *mut u8 {
        // SAFETY: `h` is a valid svOpenArrayHandle from the xsim DPI caller;
        // `svGetArrayPtr` returns the underlying data pointer.
        unsafe { (get_sv_get_array_ptr())(h).cast::<u8>() }
    }

    // xsim marshalling: svOpenArrayHandle arrays, `u8` flags/returns.
    macro_rules! dpi_fn {
        (fn $name:ident($($arg:ident : $ty:ty),* ; mut $arr:ident) => $impl_fn:expr) => {
            #[no_mangle]
            pub unsafe extern "C" fn $name(
                port: *const libc::c_char, $($arg: $ty,)* $arr: SvOpenArrayHandle,
            ) {
                // SAFETY: `port` is a DPI-provided C string valid for this call;
                // `$arr` is an xsim-provided svOpenArrayHandle.
                unsafe {
                    let port = std::ffi::CStr::from_ptr(port).to_str().unwrap_or("");
                    let ptr = sv_array_ptr($arr);
                    $impl_fn(get_or_init(), port, $($arg,)* ptr);
                }
            }
        };
        (fn $name:ident($($arg:ident : $ty:ty),* ; const $arr:ident) => $impl_fn:expr) => {
            #[no_mangle]
            pub unsafe extern "C" fn $name(
                port: *const libc::c_char, $($arg: $ty,)* $arr: SvOpenArrayHandle,
            ) {
                // SAFETY: `port` is a DPI-provided C string valid for this call;
                // `$arr` is an xsim-provided svOpenArrayHandle.
                unsafe {
                    let port = std::ffi::CStr::from_ptr(port).to_str().unwrap_or("");
                    let ptr: *const u8 = sv_array_ptr($arr).cast_const();
                    $impl_fn(get_or_init(), port, $($arg,)* ptr);
                }
            }
        };
        (fn $name:ident(mut $arr:ident) -> ret => $impl_fn:expr) => {
            #[no_mangle]
            pub unsafe extern "C" fn $name(
                port: *const libc::c_char, $arr: SvOpenArrayHandle,
            ) -> u8 {
                // SAFETY: `port` is a DPI-provided C string valid for this call;
                // `$arr` is an xsim-provided svOpenArrayHandle.
                unsafe {
                    let port = std::ffi::CStr::from_ptr(port).to_str().unwrap_or("");
                    let ptr = sv_array_ptr($arr);
                    $impl_fn(get_or_init(), port, ptr) as u8
                }
            }
        };
        (fn $name:ident(const $arr:ident) -> ret => $impl_fn:expr) => {
            #[no_mangle]
            pub unsafe extern "C" fn $name(
                port: *const libc::c_char, $arr: SvOpenArrayHandle,
            ) -> u8 {
                // SAFETY: `port` is a DPI-provided C string valid for this call;
                // `$arr` is an xsim-provided svOpenArrayHandle.
                unsafe {
                    let port = std::ffi::CStr::from_ptr(port).to_str().unwrap_or("");
                    let ptr: *const u8 = sv_array_ptr($arr).cast_const();
                    $impl_fn(get_or_init(), port, ptr) as u8
                }
            }
        };
        (fn $name:ident() -> ret => $impl_fn:expr) => {
            #[no_mangle]
            pub unsafe extern "C" fn $name(port: *const libc::c_char) -> u8 {
                // SAFETY: `port` is a DPI-provided C string valid for this call.
                unsafe {
                    let port = std::ffi::CStr::from_ptr(port).to_str().unwrap_or("");
                    $impl_fn(get_or_init(), port) as u8
                }
            }
        };
        (fn $name:ident($flag:ident : flag, mut $arr:ident) -> ret => $impl_fn:expr) => {
            #[no_mangle]
            pub unsafe extern "C" fn $name(
                port: *const libc::c_char, $flag: u8, $arr: SvOpenArrayHandle,
            ) -> u8 {
                // SAFETY: `port` is a DPI-provided C string valid for this call;
                // `$arr` is an xsim-provided svOpenArrayHandle.
                unsafe {
                    let port = std::ffi::CStr::from_ptr(port).to_str().unwrap_or("");
                    let ptr = sv_array_ptr($arr);
                    $impl_fn(get_or_init(), port, $flag != 0, ptr) as u8
                }
            }
        };
        (fn $name:ident($flag:ident : flag, const $arr:ident) -> ret => $impl_fn:expr) => {
            #[no_mangle]
            pub unsafe extern "C" fn $name(
                port: *const libc::c_char, $flag: u8, $arr: SvOpenArrayHandle,
            ) -> u8 {
                // SAFETY: `port` is a DPI-provided C string valid for this call;
                // `$arr` is an xsim-provided svOpenArrayHandle.
                unsafe {
                    let port = std::ffi::CStr::from_ptr(port).to_str().unwrap_or("");
                    let ptr: *const u8 = sv_array_ptr($arr).cast_const();
                    $impl_fn(get_or_init(), port, $flag != 0, ptr) as u8
                }
            }
        };
    }

    frt_dpi::dpi_inventory!(dpi_fn);
}
