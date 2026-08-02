#[cfg(target_os = "linux")]
mod imp {
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
    ///
    /// # Safety
    /// `h` must be a valid svOpenArrayHandle provided by the xsim DPI caller.
    unsafe fn sv_array_ptr(h: SvOpenArrayHandle) -> *mut u8 {
        // SAFETY: `h` is a valid svOpenArrayHandle from the xsim DPI caller;
        // `svGetArrayPtr` returns the underlying data pointer.
        unsafe { (get_sv_get_array_ptr())(h).cast::<u8>() }
    }

    // Marshals an svOpenArrayHandle payload pointer for writes.
    fn arr_mut_ptr(h: SvOpenArrayHandle) -> *mut u8 {
        // SAFETY: the DPI caller hands us a handle that stays valid for the
        // duration of the exported call.
        unsafe { sv_array_ptr(h) }
    }

    // Marshals an svOpenArrayHandle payload pointer for reads.
    fn arr_const_ptr(h: SvOpenArrayHandle) -> *const u8 {
        // SAFETY: the DPI caller hands us a handle that stays valid for the
        // duration of the exported call.
        unsafe { sv_array_ptr(h).cast_const() }
    }

    // Decodes an `svBit`-style flag: nonzero is set.
    fn flag_is_set(f: u8) -> bool {
        f != 0
    }

    // xsim marshalling: svOpenArrayHandle arrays, `u8` flags/returns.
    frt_dpi::dpi_export! {
        mut arr: SvOpenArrayHandle as arr_mut_ptr;
        const arr: SvOpenArrayHandle as arr_const_ptr;
        flag: u8 as flag_is_set;
        ret: u8 as u8::from;
    }
}
