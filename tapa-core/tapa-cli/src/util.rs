//! Small shared helpers.

use std::path::PathBuf;

use camino::Utf8PathBuf;

use crate::error::CliError;

/// Parse a clock period in nanoseconds as a finite, positive value.
pub fn parse_clock_period_ns(value: &str) -> Result<f64, String> {
    let period = value
        .parse::<f64>()
        .map_err(|_| format!("clock period `{value}` is not a number"))?;
    if period.is_finite() && period > 0.0 {
        Ok(period)
    } else {
        Err(format!(
            "clock period must be finite and greater than zero, got `{value}`"
        ))
    }
}

/// Convert a path to a [`Utf8PathBuf`] via a lossy conversion. The
/// synth/pack paths all originate from TAPA-controlled directories, so
/// the lossy branch is a last resort rather than a real data path.
pub fn utf8(p: impl AsRef<std::path::Path>) -> Utf8PathBuf {
    Utf8PathBuf::from(p.as_ref().to_string_lossy().into_owned())
}

/// Render a compile-time-known minijinja template. Template parse
/// and render failures are programming errors (the templates are
/// `include_str!` constants), so they panic rather than propagate.
pub fn render_template(name: &str, src: &str, ctx: minijinja::Value) -> String {
    let mut env = minijinja::Environment::new();
    env.add_template(name, src).expect("template parses");
    env.get_template(name)
        .expect("template exists")
        .render(ctx)
        .expect("render succeeds")
}

/// Build a dedicated rayon pool of `workers` threads, run `f` inside
/// it, and return the `Vec` `f` collects. Callers collect with
/// indexed `par_iter().map().collect()`, so entry order is stable
/// regardless of completion order. Pool-construction failure becomes
/// a caller-shaped [`CliError`] (`label` names the workload in the
/// message, `make_err` picks the step's error variant) instead of a
/// panic — a failed pool is a user-visible resource problem, not a
/// programming bug.
pub fn run_in_pool<T, F>(
    workers: usize,
    label: &str,
    make_err: impl FnOnce(String) -> CliError,
    f: F,
) -> crate::error::Result<Vec<T>>
where
    F: FnOnce() -> Vec<T> + Send,
    T: Send,
{
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(workers)
        .build()
        .map_err(|error| make_err(format!("cannot create {label} pool: {error}")))?;
    Ok(pool.install(f))
}

/// Resolve the first Xilinx HLS/Vitis installation root (preferring
/// `XILINX_HLS`) whose `include/` subdir exists. Used both to add
/// `-isystem` flags (`tapa g++`) and to seed vendor include probing
/// (`tapacc` cflags).
pub fn vendor_hls_root() -> Option<PathBuf> {
    for env_name in ["XILINX_HLS", "XILINX_VITIS"] {
        if let Some(root) = std::env::var_os(env_name) {
            let root = PathBuf::from(root);
            if root.join("include").exists() {
                return Some(root);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_in_pool_collects_each_item_in_stable_order() {
        use rayon::prelude::*;

        let out = run_in_pool(4, "test workload", CliError::Codegen, || {
            (0..64usize)
                .into_par_iter()
                .map(|i| i * 2)
                .collect::<Vec<_>>()
        })
        .expect("pool must build and run");
        assert_eq!(
            out,
            (0..64usize).map(|i| i * 2).collect::<Vec<_>>(),
            "indexed collect must preserve submission order",
        );
    }

    #[test]
    fn clock_period_must_be_finite_and_positive() {
        let parsed = parse_clock_period_ns("3.33").expect("valid period");
        assert!((parsed - 3.33).abs() < f64::EPSILON);
        for invalid in ["fast", "0", "-1", "NaN", "inf", "-inf"] {
            assert!(
                parse_clock_period_ns(invalid).is_err(),
                "accepted {invalid}"
            );
        }
    }
}
