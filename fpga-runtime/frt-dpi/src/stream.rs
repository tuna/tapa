use crate::context::DpiContext;
use std::sync::atomic::{AtomicU64, Ordering};

fn stream_debug_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| frt_shm::env_bool("FRT_STREAM_DEBUG"))
}

/// Returns `false` only for explicit falsy values; `true` otherwise (opt-out).
fn env_opt_out(value: Option<&str>) -> bool {
    match value {
        Some(v) => !matches!(v, "0" | "false" | "FALSE" | "no" | "NO"),
        None => true,
    }
}

fn cosim_yield_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| env_opt_out(std::env::var("FRT_COSIM_YIELD").ok().as_deref()))
}

fn maybe_yield() {
    if cosim_yield_enabled() {
        std::thread::yield_now();
    }
}

static READ_OK: AtomicU64 = AtomicU64::new(0);
static READ_MISS: AtomicU64 = AtomicU64::new(0);
static WRITE_OK: AtomicU64 = AtomicU64::new(0);
static WRITE_FULL: AtomicU64 = AtomicU64::new(0);
static LAST_REPORT: AtomicU64 = AtomicU64::new(0);
/// Counter-based pre-check: only call `SystemTime::now()` every N misses.
static MISS_COUNTER: AtomicU64 = AtomicU64::new(0);
const MISS_CHECK_INTERVAL: u64 = 1_000_000;

fn maybe_report_progress(port: &str, q: &frt_shm::SharedMemoryQueue) {
    // Fast path: only check the wall clock every MISS_CHECK_INTERVAL misses.
    if !MISS_COUNTER
        .fetch_add(1, Ordering::Relaxed)
        .is_multiple_of(MISS_CHECK_INTERVAL)
    {
        return;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let last = LAST_REPORT.load(Ordering::Relaxed);
    if now > last + 10
        && LAST_REPORT
            .compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
    {
        let rok = READ_OK.load(Ordering::Relaxed);
        let rmiss = READ_MISS.load(Ordering::Relaxed);
        let wok = WRITE_OK.load(Ordering::Relaxed);
        let wfull = WRITE_FULL.load(Ordering::Relaxed);
        let (head, tail) = q.head_tail();
        if rok + rmiss + wok + wfull > 0 {
            eprintln!("frt-dpi: progress[{port}]: read_ok={rok} read_empty={rmiss} write_ok={wok} write_full={wfull} q_head={head} q_tail={tail}");
        }
    }
}

pub fn stream_try_read_impl(ctx: &DpiContext, port: &str, out: *mut u8) -> bool {
    let Some(stream) = ctx.streams.get(port) else {
        eprintln!("frt-dpi: stream_try_read: unknown port '{port}'");
        return false;
    };
    let Ok(mut s) = stream.inner.lock() else {
        return false;
    };
    if s.queue.is_empty() {
        READ_MISS.fetch_add(1, Ordering::Relaxed);
        maybe_report_progress(port, &s.queue);
        maybe_yield();
        return false;
    }
    let dpi = stream.dpi_width_bytes;
    // SAFETY: `out` is a DPI-provided buffer of at least `dpi` bytes.
    let buf = unsafe { std::slice::from_raw_parts_mut(out, dpi) };
    buf.fill(0);
    s.queue.pop_into(buf);
    READ_OK.fetch_add(1, Ordering::Relaxed);
    if stream_debug_enabled() {
        eprintln!(
            "frt-dpi: stream_try_read '{port}': got {} bytes",
            s.queue.width()
        );
    }
    true
}

pub fn stream_istream_step_impl(ctx: &DpiContext, port: &str, consume: bool, out: *mut u8) -> bool {
    let Some(stream) = ctx.streams.get(port) else {
        eprintln!("frt-dpi: stream_istream_step: unknown port '{port}'");
        return false;
    };
    let Ok(mut s) = stream.inner.lock() else {
        return false;
    };

    if s.last_istream_valid && consume {
        // Consume the previously-peeked front element.
        let mut discard = [0u8; 256];
        let w = s.queue.width();
        if w <= discard.len() {
            if s.queue.pop_into(&mut discard[..w]) {
                READ_OK.fetch_add(1, Ordering::Relaxed);
            }
        } else if s.queue.pop().is_some() {
            READ_OK.fetch_add(1, Ordering::Relaxed);
        }
    }

    if s.queue.is_empty() {
        s.last_istream_valid = false;
        READ_MISS.fetch_add(1, Ordering::Relaxed);
        maybe_report_progress(port, &s.queue);
        maybe_yield();
        return false;
    }
    let dpi = stream.dpi_width_bytes;
    // SAFETY: `out` is a DPI-provided buffer of at least `dpi` bytes.
    let buf = unsafe { std::slice::from_raw_parts_mut(out, dpi) };
    buf.fill(0);
    s.queue.peek_into(buf);
    s.last_istream_valid = true;
    true
}

pub fn stream_try_write_impl(ctx: &DpiContext, port: &str, data: *const u8) -> bool {
    let Some(stream) = ctx.streams.get(port) else {
        eprintln!("frt-dpi: stream_try_write: unknown port '{port}'");
        return false;
    };
    let Ok(mut s) = stream.inner.lock() else {
        return false;
    };
    let w = s.queue.width();
    // SAFETY: `data` is a DPI-provided buffer of at least `w` bytes.
    let slice = unsafe { std::slice::from_raw_parts(data, w) };
    let ok = s.queue.try_push(slice).is_ok();
    if ok {
        WRITE_OK.fetch_add(1, Ordering::Relaxed);
        if stream_debug_enabled() {
            eprintln!("frt-dpi: stream_try_write '{port}': wrote {w} bytes");
        }
    }
    ok
}

pub fn stream_ostream_step_impl(
    ctx: &DpiContext,
    port: &str,
    write: bool,
    data: *const u8,
) -> bool {
    let Some(stream) = ctx.streams.get(port) else {
        eprintln!("frt-dpi: stream_ostream_step: unknown port '{port}'");
        return false;
    };
    let Ok(mut s) = stream.inner.lock() else {
        return false;
    };

    if s.last_ostream_ready && write {
        let w = s.queue.width();
        // SAFETY: `data` is a DPI-provided buffer of at least `w` bytes.
        let slice = unsafe { std::slice::from_raw_parts(data, w) };
        if s.queue.try_push(slice).is_ok() {
            WRITE_OK.fetch_add(1, Ordering::Relaxed);
            if stream_debug_enabled() {
                eprintln!("frt-dpi: stream_ostream_step '{port}': wrote {w} bytes");
            }
        }
    }

    let can = !s.queue.is_full();
    s.last_ostream_ready = can;
    if !can {
        WRITE_FULL.fetch_add(1, Ordering::Relaxed);
        maybe_report_progress(port, &s.queue);
        maybe_yield();
    }
    can
}

pub fn stream_hls_ostream_step_impl(
    ctx: &DpiContext,
    port: &str,
    write: bool,
    data: *const u8,
) -> bool {
    let Some(stream) = ctx.streams.get(port) else {
        eprintln!("frt-dpi: stream_hls_ostream_step: unknown port '{port}'");
        return false;
    };
    let Ok(mut s) = stream.inner.lock() else {
        return false;
    };

    // HLS ap_fifo/full_n uses the queue state visible in the current cycle,
    // unlike AXIS tready which is sampled from the prior cycle in the Vitis path.
    if write {
        if s.queue.is_full() {
            let (h, t) = s.queue.head_tail();
            eprintln!(
                "frt-dpi: WARN '{port}': write=1 but queue full! h={h} t={t} depth={}",
                s.queue.depth()
            );
        } else {
            let w = s.queue.width();
            // SAFETY: `data` is a DPI-provided buffer of at least `w` bytes.
            let slice = unsafe { std::slice::from_raw_parts(data, w) };
            if s.queue.try_push(slice).is_ok() {
                WRITE_OK.fetch_add(1, Ordering::Relaxed);
                let (h, t) = s.queue.head_tail();
                if stream_debug_enabled() {
                    eprintln!(
                        "frt-dpi: stream_hls_ostream_step '{port}': wrote {w} bytes (h={h} t={t})"
                    );
                }
            }
        }
    }

    let can = !s.queue.is_full();
    if !can {
        WRITE_FULL.fetch_add(1, Ordering::Relaxed);
        maybe_report_progress(port, &s.queue);
        maybe_yield();
    }
    can
}

pub fn stream_can_write_impl(ctx: &DpiContext, port: &str) -> bool {
    let Some(stream) = ctx.streams.get(port) else {
        eprintln!("frt-dpi: stream_can_write: unknown port '{port}'");
        return false;
    };
    let Ok(s) = stream.inner.lock() else {
        return false;
    };
    let can = !s.queue.is_full();
    if !can {
        WRITE_FULL.fetch_add(1, Ordering::Relaxed);
        maybe_report_progress(port, &s.queue);
        maybe_yield();
    }
    can
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::{DpiContext, DpiStream, DpiStreamInner};
    use frt_shm::SharedMemoryQueue;
    use std::collections::HashMap;
    use std::sync::Mutex;

    fn make_ctx_with_stream(
        name: &str,
        depth: u32,
        width: u32,
        dpi_width_bytes: usize,
    ) -> DpiContext {
        let q = SharedMemoryQueue::create(name, depth, width).expect("create stream queue");
        DpiContext {
            buffers: HashMap::new(),
            streams: HashMap::from([(
                name.to_owned(),
                DpiStream {
                    inner: Mutex::new(DpiStreamInner {
                        queue: q,
                        last_istream_valid: false,
                        last_ostream_ready: false,
                    }),
                    dpi_width_bytes,
                },
            )]),
        }
    }

    #[test]
    fn can_write_false_when_full() {
        let ctx = make_ctx_with_stream("stream_can_write_full", 2, 4, 4);
        assert!(stream_can_write_impl(&ctx, "stream_can_write_full"));
        assert!(stream_try_write_impl(
            &ctx,
            "stream_can_write_full",
            1u32.to_le_bytes().as_ptr()
        ));
        assert!(stream_try_write_impl(
            &ctx,
            "stream_can_write_full",
            2u32.to_le_bytes().as_ptr()
        ));
        assert!(!stream_can_write_impl(&ctx, "stream_can_write_full"));
    }

    #[test]
    fn can_write_true_after_pop() {
        let ctx = make_ctx_with_stream("stream_can_write_pop", 1, 4, 4);
        assert!(stream_try_write_impl(
            &ctx,
            "stream_can_write_pop",
            7u32.to_le_bytes().as_ptr()
        ));
        assert!(!stream_can_write_impl(&ctx, "stream_can_write_pop"));
        let mut out = [0u8; 4];
        assert!(stream_try_read_impl(
            &ctx,
            "stream_can_write_pop",
            out.as_mut_ptr()
        ));
        assert_eq!(u32::from_le_bytes(out), 7);
        assert!(stream_can_write_impl(&ctx, "stream_can_write_pop"));
    }

    #[test]
    fn read_zero_fills_missing_eos_or_tlast_byte() {
        let ctx = make_ctx_with_stream("stream_try_read_zero_fills_tail", 1, 4, 5);
        let in_bytes = [1u8, 2, 3, 4];
        assert!(stream_try_write_impl(
            &ctx,
            "stream_try_read_zero_fills_tail",
            in_bytes.as_ptr()
        ));

        let mut out = [0xffu8; 5];
        assert!(stream_try_read_impl(
            &ctx,
            "stream_try_read_zero_fills_tail",
            out.as_mut_ptr()
        ));
        assert_eq!(&out[..4], &in_bytes);
        assert_eq!(out[4], 0);
    }

    #[test]
    fn read_preserves_explicit_eos_or_tlast_byte() {
        let ctx = make_ctx_with_stream("stream_try_read_preserves_tail", 1, 5, 5);
        let in_bytes = [1u8, 2, 3, 4, 1];
        assert!(stream_try_write_impl(
            &ctx,
            "stream_try_read_preserves_tail",
            in_bytes.as_ptr()
        ));

        let mut out = [0u8; 5];
        assert!(stream_try_read_impl(
            &ctx,
            "stream_try_read_preserves_tail",
            out.as_mut_ptr()
        ));
        assert_eq!(out, in_bytes);
    }

    #[test]
    fn istream_step_holds_front_until_consumed_and_refills_same_cycle() {
        let ctx = make_ctx_with_stream("stream_istream_step_holds_front", 4, 4, 4);
        let first = 1u32.to_le_bytes();
        let second = 2u32.to_le_bytes();
        assert!(stream_try_write_impl(
            &ctx,
            "stream_istream_step_holds_front",
            first.as_ptr()
        ));
        assert!(stream_try_write_impl(
            &ctx,
            "stream_istream_step_holds_front",
            second.as_ptr()
        ));

        let mut out = [0u8; 4];
        assert!(stream_istream_step_impl(
            &ctx,
            "stream_istream_step_holds_front",
            false,
            out.as_mut_ptr()
        ));
        assert_eq!(u32::from_le_bytes(out), 1);

        out.fill(0);
        assert!(stream_istream_step_impl(
            &ctx,
            "stream_istream_step_holds_front",
            false,
            out.as_mut_ptr()
        ));
        assert_eq!(u32::from_le_bytes(out), 1);

        out.fill(0);
        assert!(stream_istream_step_impl(
            &ctx,
            "stream_istream_step_holds_front",
            true,
            out.as_mut_ptr()
        ));
        assert_eq!(u32::from_le_bytes(out), 2);

        out.fill(0);
        assert!(!stream_istream_step_impl(
            &ctx,
            "stream_istream_step_holds_front",
            true,
            out.as_mut_ptr()
        ));
    }

    #[test]
    fn ostream_step_only_commits_data_after_prior_ready_cycle() {
        let ctx = make_ctx_with_stream("stream_ostream_step_prior_ready", 1, 4, 4);
        let first = 7u32.to_le_bytes();

        assert!(stream_ostream_step_impl(
            &ctx,
            "stream_ostream_step_prior_ready",
            false,
            first.as_ptr()
        ));
        let mut missing = [0u8; 4];
        assert!(!stream_try_read_impl(
            &ctx,
            "stream_ostream_step_prior_ready",
            missing.as_mut_ptr()
        ));

        assert!(!stream_ostream_step_impl(
            &ctx,
            "stream_ostream_step_prior_ready",
            true,
            first.as_ptr()
        ));
        let mut out = [0u8; 4];
        assert!(stream_try_read_impl(
            &ctx,
            "stream_ostream_step_prior_ready",
            out.as_mut_ptr()
        ));
        assert_eq!(u32::from_le_bytes(out), 7);
    }

    #[test]
    fn hls_ostream_step_commits_immediately_when_queue_is_ready() {
        let ctx = make_ctx_with_stream("stream_hls_ostream_step_immediate", 1, 4, 4);
        let first = 11u32.to_le_bytes();

        assert!(!stream_try_read_impl(
            &ctx,
            "stream_hls_ostream_step_immediate",
            [0u8; 4].as_ptr().cast_mut(),
        ));

        assert!(!stream_hls_ostream_step_impl(
            &ctx,
            "stream_hls_ostream_step_immediate",
            true,
            first.as_ptr()
        ));

        let mut out = [0u8; 4];
        assert!(stream_try_read_impl(
            &ctx,
            "stream_hls_ostream_step_immediate",
            out.as_mut_ptr()
        ));
        assert_eq!(u32::from_le_bytes(out), 11);
    }

    #[test]
    fn hls_ostream_step_does_not_drop_first_write_after_queue_recovers() {
        let ctx = make_ctx_with_stream("stream_hls_ostream_step_recovery", 1, 4, 4);
        let first = 21u32.to_le_bytes();
        let second = 22u32.to_le_bytes();

        assert!(!stream_hls_ostream_step_impl(
            &ctx,
            "stream_hls_ostream_step_recovery",
            true,
            first.as_ptr()
        ));

        let mut out = [0u8; 4];
        assert!(stream_try_read_impl(
            &ctx,
            "stream_hls_ostream_step_recovery",
            out.as_mut_ptr()
        ));
        assert_eq!(u32::from_le_bytes(out), 21);

        assert!(!stream_hls_ostream_step_impl(
            &ctx,
            "stream_hls_ostream_step_recovery",
            true,
            second.as_ptr()
        ));
        out.fill(0);
        assert!(stream_try_read_impl(
            &ctx,
            "stream_hls_ostream_step_recovery",
            out.as_mut_ptr()
        ));
        assert_eq!(u32::from_le_bytes(out), 22);
    }

    #[test]
    fn env_opt_out_defaults_to_enabled() {
        assert!(env_opt_out(None));
    }

    #[test]
    fn env_opt_out_can_be_disabled_explicitly() {
        for value in ["0", "false", "FALSE", "no", "NO"] {
            assert!(!env_opt_out(Some(value)), "expected {value} to disable");
        }
    }

    #[test]
    fn env_opt_out_stays_enabled_for_true_values() {
        for value in ["1", "true", "TRUE", "yes", "YES", "maybe"] {
            assert!(env_opt_out(Some(value)), "expected {value} to keep enabled");
        }
    }
}
