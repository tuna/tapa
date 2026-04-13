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
