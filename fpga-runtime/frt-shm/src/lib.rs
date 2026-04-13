pub mod env;
pub mod mmap_segment;
pub mod queue;

pub use mmap_segment::MmapSegment;
pub use queue::SharedMemoryQueue;

/// Parse an environment variable as a boolean flag.
///
/// Returns `true` for `"1"`, `"true"`, `"TRUE"`, `"yes"`, `"YES"`;
/// `false` for everything else (including unset).
pub fn env_bool(name: &str) -> bool {
    match std::env::var(name) {
        Ok(v) => matches!(v.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"),
        Err(_) => false,
    }
}

/// Read an environment variable, returning `None` if unset, empty, or
/// whitespace-only.  Only allocates when the value is non-empty.
pub fn env_non_empty(name: &str) -> Option<String> {
    std::env::var(name).ok().and_then(|v| {
        let trimmed = v.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_owned())
        }
    })
}
