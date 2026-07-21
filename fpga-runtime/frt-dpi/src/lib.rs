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

/// The single enumeration of the `tapa_*` DPI entry points.
///
/// Shared by the verilator and xsim backends: each backend defines a
/// `dpi_fn` macro with arms for these shapes and expands the inventory
/// with its own marshalling. Adding a DPI function is one line here;
/// both backends then emit it.
///
/// - `(fn name(arg: ty, ...; mut arr) => impl)` — scalar args + mutable byte array
/// - `(fn name(arg: ty, ...; const arr) => impl)` — scalar args + const byte array
/// - `(fn name(mut arr) -> ret => impl)` — mutable array, scalar return
/// - `(fn name(const arr) -> ret => impl)` — const array, scalar return
/// - `(fn name() -> ret => impl)` — no array, scalar return
/// - `(fn name(flag: flag, mut/const arr) -> ret => impl)` — bool flag + array
#[macro_export]
macro_rules! dpi_inventory {
    ($adapter:ident) => {
        $adapter!(fn tapa_axi_read(addr: u64, width: u32; mut out) => $crate::axi::axi_read_impl);
        $adapter!(fn tapa_axi_write(addr: u64, width: u32; const data) => $crate::axi::axi_write_impl);
        $adapter!(fn tapa_stream_try_read(mut out) -> ret => $crate::stream::stream_try_read_impl);
        $adapter!(fn tapa_stream_try_write(const data) -> ret => $crate::stream::stream_try_write_impl);
        $adapter!(fn tapa_stream_can_write() -> ret => $crate::stream::stream_can_write_impl);
        $adapter!(fn tapa_stream_istream_step(consume: flag, mut out) -> ret => $crate::stream::stream_istream_step_impl);
        $adapter!(fn tapa_stream_ostream_step(write: flag, const data) -> ret => $crate::stream::stream_ostream_step_impl);
        $adapter!(fn tapa_hls_stream_ostream_step(write: flag, const data) -> ret => $crate::stream::stream_hls_ostream_step_impl);
    };
}
