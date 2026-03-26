pub mod axi;
pub mod context;
pub mod stream;

pub use context::{DpiContext, DpiError};
use std::sync::OnceLock;

static CTX: OnceLock<DpiContext> = OnceLock::new();

pub fn get_or_init() -> &'static DpiContext {
    CTX.get_or_init(|| {
        DpiContext::from_env().unwrap_or_else(|e| {
            eprintln!("frt-dpi: failed to init DpiContext: {e}");
            std::process::exit(1);
        })
    })
}
