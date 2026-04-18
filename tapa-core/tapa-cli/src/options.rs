//! Mirror of `tapa.util.Options` — global per-process knobs.

/// Default `--clang-format-quota-in-bytes` (matches the Python value).
pub const DEFAULT_CLANG_FORMAT_QUOTA: u64 = 1_000_000;

#[derive(Debug, Clone, Copy)]
pub struct Options {
    pub clang_format_quota_in_bytes: u64,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            clang_format_quota_in_bytes: DEFAULT_CLANG_FORMAT_QUOTA,
        }
    }
}
