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
