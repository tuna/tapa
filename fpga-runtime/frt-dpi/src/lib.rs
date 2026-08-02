pub mod axi;
pub mod context;
pub mod stream;

pub use context::{DpiContext, DpiError};
use std::sync::OnceLock;

static CTX: OnceLock<DpiContext> = OnceLock::new();

pub fn get_or_init() -> &'static DpiContext {
    CTX.get_or_init(|| {
        let ctx = DpiContext::from_env().unwrap_or_else(|e| {
            eprintln!("frt-dpi: failed to init DpiContext: {e}");
            #[allow(clippy::exit, reason = "fatal: no recovery path inside a DPI callback")]
            std::process::exit(1);
        });
        if frt_shm::env_bool(frt_shm::env::FRT_STREAM_DEBUG) {
            eprintln!(
                "frt-dpi: init with {} buffers, {} streams",
                ctx.buffers.len(),
                ctx.streams.len()
            );
            for name in ctx.streams.keys() {
                eprintln!("frt-dpi:   stream '{name}'");
            }
        }
        ctx
    })
}

/// Emit the eight `tapa_*` DPI entry points with backend-specific marshalling.
///
/// The two backend cdylibs (frt-dpi-verilator, frt-dpi-xsim) expose the same
/// symbols to the simulators but differ in wire representation, so each
/// invokes this macro once with its own conversions:
///
/// - `mut arr` / `const arr` — FFI type of the byte-array parameter and the
///   function extracting its payload pointer (`*mut u8` / `*const u8`).
///   Verilator passes raw pointers through (`::core::convert::identity`);
///   xsim resolves an `svOpenArrayHandle` via `svGetArrayPtr`.
/// - `flag` / `ret` — the wire type carrying booleans in and out (`bool`
///   for Verilator, `u8` for xsim) and the lossless conversions.
///
/// Adding a DPI function means adding one body below; both backends then
/// export it.
#[macro_export]
macro_rules! dpi_export {
    (
        mut arr: $mut_arr:ty as $mut_ptr:expr;
        const arr: $const_arr:ty as $const_ptr:expr;
        flag: $flag:ty as $flag_of:expr;
        ret: $ret:ty as $ret_of:expr;
    ) => {
        #[no_mangle]
        pub unsafe extern "C" fn tapa_axi_read(
            port: *const ::libc::c_char,
            addr: u64,
            width: u32,
            out: $mut_arr,
        ) {
            // SAFETY: `port` is a C string provided by the DPI caller; it
            // remains valid for the duration of this call.
            let port = unsafe { ::std::ffi::CStr::from_ptr(port) }
                .to_str()
                .unwrap_or("");
            $crate::axi::axi_read_impl($crate::get_or_init(), port, addr, width, $mut_ptr(out));
        }

        #[no_mangle]
        pub unsafe extern "C" fn tapa_axi_write(
            port: *const ::libc::c_char,
            addr: u64,
            width: u32,
            data: $const_arr,
        ) {
            // SAFETY: `port` is a C string provided by the DPI caller; it
            // remains valid for the duration of this call.
            let port = unsafe { ::std::ffi::CStr::from_ptr(port) }
                .to_str()
                .unwrap_or("");
            $crate::axi::axi_write_impl($crate::get_or_init(), port, addr, width, $const_ptr(data));
        }

        #[no_mangle]
        pub unsafe extern "C" fn tapa_stream_try_read(
            port: *const ::libc::c_char,
            out: $mut_arr,
        ) -> $ret {
            // SAFETY: `port` is a C string provided by the DPI caller; it
            // remains valid for the duration of this call.
            let port = unsafe { ::std::ffi::CStr::from_ptr(port) }
                .to_str()
                .unwrap_or("");
            $ret_of($crate::stream::stream_try_read_impl(
                $crate::get_or_init(),
                port,
                $mut_ptr(out),
            ))
        }

        #[no_mangle]
        pub unsafe extern "C" fn tapa_stream_try_write(
            port: *const ::libc::c_char,
            data: $const_arr,
        ) -> $ret {
            // SAFETY: `port` is a C string provided by the DPI caller; it
            // remains valid for the duration of this call.
            let port = unsafe { ::std::ffi::CStr::from_ptr(port) }
                .to_str()
                .unwrap_or("");
            $ret_of($crate::stream::stream_try_write_impl(
                $crate::get_or_init(),
                port,
                $const_ptr(data),
            ))
        }

        #[no_mangle]
        pub unsafe extern "C" fn tapa_stream_can_write(port: *const ::libc::c_char) -> $ret {
            // SAFETY: `port` is a C string provided by the DPI caller; it
            // remains valid for the duration of this call.
            let port = unsafe { ::std::ffi::CStr::from_ptr(port) }
                .to_str()
                .unwrap_or("");
            $ret_of($crate::stream::stream_can_write_impl(
                $crate::get_or_init(),
                port,
            ))
        }

        #[no_mangle]
        pub unsafe extern "C" fn tapa_stream_istream_step(
            port: *const ::libc::c_char,
            consume: $flag,
            out: $mut_arr,
        ) -> $ret {
            // SAFETY: `port` is a C string provided by the DPI caller; it
            // remains valid for the duration of this call.
            let port = unsafe { ::std::ffi::CStr::from_ptr(port) }
                .to_str()
                .unwrap_or("");
            $ret_of($crate::stream::stream_istream_step_impl(
                $crate::get_or_init(),
                port,
                $flag_of(consume),
                $mut_ptr(out),
            ))
        }

        #[no_mangle]
        pub unsafe extern "C" fn tapa_stream_ostream_step(
            port: *const ::libc::c_char,
            write: $flag,
            data: $const_arr,
        ) -> $ret {
            // SAFETY: `port` is a C string provided by the DPI caller; it
            // remains valid for the duration of this call.
            let port = unsafe { ::std::ffi::CStr::from_ptr(port) }
                .to_str()
                .unwrap_or("");
            $ret_of($crate::stream::stream_ostream_step_impl(
                $crate::get_or_init(),
                port,
                $flag_of(write),
                $const_ptr(data),
            ))
        }

        #[no_mangle]
        pub unsafe extern "C" fn tapa_hls_stream_ostream_step(
            port: *const ::libc::c_char,
            write: $flag,
            data: $const_arr,
        ) -> $ret {
            // SAFETY: `port` is a C string provided by the DPI caller; it
            // remains valid for the duration of this call.
            let port = unsafe { ::std::ffi::CStr::from_ptr(port) }
                .to_str()
                .unwrap_or("");
            $ret_of($crate::stream::stream_hls_ostream_step_impl(
                $crate::get_or_init(),
                port,
                $flag_of(write),
                $const_ptr(data),
            ))
        }
    };
}
