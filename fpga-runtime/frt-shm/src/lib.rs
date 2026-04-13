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

/// Parse an environment variable as a boolean flag with opt-out semantics.
///
/// Returns `true` unless the variable is explicitly set to a falsy value
/// (`"0"`, `"false"`, `"FALSE"`, `"no"`, `"NO"`).  Unset or any other
/// value is treated as enabled.  This is the inverse of [`env_bool`].
pub fn env_bool_default_true(name: &str) -> bool {
    match std::env::var(name) {
        Ok(v) => !matches!(v.as_str(), "0" | "false" | "FALSE" | "no" | "NO"),
        Err(_) => true,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Serialize env-var tests to avoid cross-test pollution.
    static ENV_LOCK: Mutex<()> = Mutex::new(());
    const TEST_VAR: &str = "FRT_SHM_TEST_BOOL_DEFAULT_TRUE";

    #[test]
    fn env_bool_default_true_returns_true_when_unset() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::remove_var(TEST_VAR);
        assert!(env_bool_default_true(TEST_VAR));
    }

    #[test]
    fn env_bool_default_true_disabled_by_falsy_values() {
        let _g = ENV_LOCK.lock().unwrap();
        for value in ["0", "false", "FALSE", "no", "NO"] {
            std::env::set_var(TEST_VAR, value);
            assert!(
                !env_bool_default_true(TEST_VAR),
                "expected '{value}' to disable"
            );
        }
        std::env::remove_var(TEST_VAR);
    }

    #[test]
    fn env_bool_default_true_stays_enabled_for_truthy_values() {
        let _g = ENV_LOCK.lock().unwrap();
        for value in ["1", "true", "TRUE", "yes", "YES", "maybe"] {
            std::env::set_var(TEST_VAR, value);
            assert!(
                env_bool_default_true(TEST_VAR),
                "expected '{value}' to keep enabled"
            );
        }
        std::env::remove_var(TEST_VAR);
    }
}
