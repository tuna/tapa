//! Verbosity → log-level mapping. Mirrors
//! `tapa.util.setup_logging`'s `(quiet - verbose) * 10 + INFO` formula
//! clamped to `[DEBUG, CRITICAL]`.

use log::LevelFilter;

/// Translate `--verbose -v` count and `--quiet -q` count to a `LevelFilter`.
///
/// Mirrors Python's `(quiet - verbose) * 10 + INFO`, clamped to
/// `[DEBUG, CRITICAL]`. CRITICAL has no `log` crate analogue so it folds
/// into `Error`. DEBUG is the floor: more verbose flags cannot reach
/// `Trace` — Python's `logging` module has no level below DEBUG.
pub fn level_for(verbose: u8, quiet: u8) -> LevelFilter {
    let raw = i32::from(quiet) - i32::from(verbose);
    let raw = raw.clamp(-1, 3);
    match raw {
        -1 => LevelFilter::Debug,
        0 => LevelFilter::Info,
        1 => LevelFilter::Warn,
        _ => LevelFilter::Error,
    }
}

/// Initialize the global logger using `env_logger`. Safe to call once per
/// process; subsequent calls are no-ops.
pub fn install(verbose: u8, quiet: u8) {
    let level = level_for(verbose, quiet);
    let mut builder = env_logger::Builder::new();
    builder.filter_level(level);
    builder.format_timestamp_secs();
    let _ = builder.try_init();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verbose_bumps_below_info() {
        assert_eq!(level_for(0, 0), LevelFilter::Info);
        assert_eq!(level_for(1, 0), LevelFilter::Debug);
        // Python clamps at DEBUG, so further -v stays at Debug.
        assert_eq!(level_for(2, 0), LevelFilter::Debug);
    }

    #[test]
    fn quiet_bumps_above_info() {
        assert_eq!(level_for(0, 1), LevelFilter::Warn);
        assert_eq!(level_for(0, 2), LevelFilter::Error);
    }

    #[test]
    fn verbose_and_quiet_subtract() {
        assert_eq!(level_for(2, 2), LevelFilter::Info);
        assert_eq!(level_for(3, 1), LevelFilter::Debug);
    }

    #[test]
    fn extreme_values_clamp() {
        assert_eq!(level_for(0, 99), LevelFilter::Error);
        assert_eq!(level_for(99, 0), LevelFilter::Debug);
    }
}
