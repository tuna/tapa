use crate::context::DpiContext;
use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

static STREAM_DBG: std::sync::Once = std::sync::Once::new();
static mut STREAM_DBG_ENABLED: bool = false;
thread_local! {
    static BLOCKED_STREAM_STREAK: Cell<u32> = const { Cell::new(0) };
}

fn stream_debug_enabled() -> bool {
    STREAM_DBG.call_once(|| {
        unsafe { STREAM_DBG_ENABLED = std::env::var("FRT_STREAM_DEBUG").is_ok() };
    });
    unsafe { STREAM_DBG_ENABLED }
}

fn blocked_stream_backoff_enabled_from_env(value: Option<&str>) -> bool {
    match value {
        Some(value) => !matches!(value, "0" | "false" | "FALSE" | "no" | "NO"),
        None => true,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlockedStreamBackoff {
    Yield,
    Sleep(Duration),
}

fn blocked_stream_backoff_for_streak(streak: u32) -> BlockedStreamBackoff {
    if streak < 64 {
        BlockedStreamBackoff::Yield
    } else if streak < 512 {
        BlockedStreamBackoff::Sleep(Duration::from_micros(50))
    } else {
        BlockedStreamBackoff::Sleep(Duration::from_micros(200))
    }
}

fn reset_blocked_stream_backoff() {
    BLOCKED_STREAM_STREAK.with(|streak| streak.set(0));
}

fn maybe_yield() {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *ENABLED.get_or_init(|| {
        blocked_stream_backoff_enabled_from_env(std::env::var("FRT_COSIM_YIELD").ok().as_deref())
    }) {
        BLOCKED_STREAM_STREAK.with(|streak| {
            let next = streak.get().saturating_add(1);
            streak.set(next);
            match blocked_stream_backoff_for_streak(next) {
                BlockedStreamBackoff::Yield => std::thread::yield_now(),
                BlockedStreamBackoff::Sleep(duration) => std::thread::sleep(duration),
            }
        });
    }
}

static READ_OK: AtomicU64 = AtomicU64::new(0);
static READ_MISS: AtomicU64 = AtomicU64::new(0);
static WRITE_OK: AtomicU64 = AtomicU64::new(0);
static WRITE_FULL: AtomicU64 = AtomicU64::new(0);
static LAST_REPORT: AtomicU64 = AtomicU64::new(0);

fn maybe_report_progress() {
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
        if rok + rmiss + wok + wfull > 0 {
            eprintln!("frt-dpi: progress: read_ok={rok} read_empty={rmiss} write_ok={wok} write_full={wfull}");
        }
    }
}

fn copy_stream_bytes(out: *mut u8, data: &[u8], dpi_width_bytes: usize) {
    let copy_len = data.len().min(dpi_width_bytes);
    unsafe {
        std::ptr::write_bytes(out, 0, dpi_width_bytes);
        std::ptr::copy_nonoverlapping(data.as_ptr(), out, copy_len);
    }
}

pub fn stream_try_read_impl(ctx: &DpiContext, port: &str, out: *mut u8) -> bool {
    let Some(stream) = ctx.streams.get(port) else {
        eprintln!("frt-dpi: stream_try_read: unknown port '{port}'");
        return false;
    };
    let mut q = match stream.queue.lock() {
        Ok(guard) => guard,
        Err(_) => return false,
    };
    if let Some(data) = q.pop() {
        reset_blocked_stream_backoff();
        READ_OK.fetch_add(1, Ordering::Relaxed);
        if stream_debug_enabled() {
            eprintln!(
                "frt-dpi: stream_try_read '{port}': got {} bytes",
                data.len()
            );
        }
        copy_stream_bytes(out, &data, stream.dpi_width_bytes);
        true
    } else {
        READ_MISS.fetch_add(1, Ordering::Relaxed);
        maybe_report_progress();
        maybe_yield();
        false
    }
}

pub fn stream_istream_step_impl(ctx: &DpiContext, port: &str, consume: bool, out: *mut u8) -> bool {
    let Some(stream) = ctx.streams.get(port) else {
        eprintln!("frt-dpi: stream_istream_step: unknown port '{port}'");
        return false;
    };
    let mut q = match stream.queue.lock() {
        Ok(guard) => guard,
        Err(_) => return false,
    };
    let mut state = match stream.state.lock() {
        Ok(guard) => guard,
        Err(_) => return false,
    };

    if state.last_istream_valid && consume {
        if q.pop().is_some() {
            reset_blocked_stream_backoff();
            READ_OK.fetch_add(1, Ordering::Relaxed);
        }
    }

    if let Some(data) = q.peek() {
        copy_stream_bytes(out, &data, stream.dpi_width_bytes);
        state.last_istream_valid = true;
        true
    } else {
        state.last_istream_valid = false;
        READ_MISS.fetch_add(1, Ordering::Relaxed);
        maybe_report_progress();
        maybe_yield();
        false
    }
}

pub fn stream_try_write_impl(ctx: &DpiContext, port: &str, data: *const u8) -> bool {
    let Some(stream) = ctx.streams.get(port) else {
        eprintln!("frt-dpi: stream_try_write: unknown port '{port}'");
        return false;
    };
    let mut q = match stream.queue.lock() {
        Ok(guard) => guard,
        Err(_) => return false,
    };
    let w = q.width();
    let slice = unsafe { std::slice::from_raw_parts(data, w) };
    let ok = q.try_push(slice).is_ok();
    if ok {
        reset_blocked_stream_backoff();
        WRITE_OK.fetch_add(1, Ordering::Relaxed);
        if stream_debug_enabled() {
            eprintln!("frt-dpi: stream_try_write '{port}': wrote {w} bytes");
        }
    }
    ok
}

pub fn stream_ostream_step_impl(ctx: &DpiContext, port: &str, write: bool, data: *const u8) -> bool {
    let Some(stream) = ctx.streams.get(port) else {
        eprintln!("frt-dpi: stream_ostream_step: unknown port '{port}'");
        return false;
    };
    let mut q = match stream.queue.lock() {
        Ok(guard) => guard,
        Err(_) => return false,
    };
    let mut state = match stream.state.lock() {
        Ok(guard) => guard,
        Err(_) => return false,
    };

    if state.last_ostream_ready && write {
        let w = q.width();
        let slice = unsafe { std::slice::from_raw_parts(data, w) };
        if q.try_push(slice).is_ok() {
            reset_blocked_stream_backoff();
            WRITE_OK.fetch_add(1, Ordering::Relaxed);
            if stream_debug_enabled() {
                eprintln!("frt-dpi: stream_ostream_step '{port}': wrote {w} bytes");
            }
        }
    }

    let can = !q.is_full();
    state.last_ostream_ready = can;
    if can {
        reset_blocked_stream_backoff();
    } else {
        WRITE_FULL.fetch_add(1, Ordering::Relaxed);
        maybe_report_progress();
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
    let mut q = match stream.queue.lock() {
        Ok(guard) => guard,
        Err(_) => return false,
    };

    // HLS ap_fifo/full_n uses the queue state visible in the current cycle,
    // unlike AXIS tready which is sampled from the prior cycle in the Vitis path.
    if write && !q.is_full() {
        let w = q.width();
        let slice = unsafe { std::slice::from_raw_parts(data, w) };
        if q.try_push(slice).is_ok() {
            reset_blocked_stream_backoff();
            WRITE_OK.fetch_add(1, Ordering::Relaxed);
            if stream_debug_enabled() {
                eprintln!("frt-dpi: stream_hls_ostream_step '{port}': wrote {w} bytes");
            }
        }
    }

    let can = !q.is_full();
    if can {
        reset_blocked_stream_backoff();
    } else {
        WRITE_FULL.fetch_add(1, Ordering::Relaxed);
        maybe_report_progress();
        maybe_yield();
    }
    can
}

pub fn stream_can_write_impl(ctx: &DpiContext, port: &str) -> bool {
    let Some(stream) = ctx.streams.get(port) else {
        eprintln!("frt-dpi: stream_can_write: unknown port '{port}'");
        return false;
    };
    let q = match stream.queue.lock() {
        Ok(guard) => guard,
        Err(_) => return false,
    };
    let can = !q.is_full();
    if can {
        reset_blocked_stream_backoff();
    } else {
        WRITE_FULL.fetch_add(1, Ordering::Relaxed);
        maybe_report_progress();
        maybe_yield();
    }
    can
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::{DpiContext, DpiStream};
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
                    queue: Mutex::new(q),
                    dpi_width_bytes,
                    state: Mutex::new(Default::default()),
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
        assert!(stream_try_write_impl(&ctx, "stream_istream_step_holds_front", first.as_ptr()));
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
            [0u8; 4].as_ptr() as *mut u8,
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
    fn blocked_stream_backoff_defaults_to_enabled() {
        assert!(blocked_stream_backoff_enabled_from_env(None));
    }

    #[test]
    fn blocked_stream_backoff_can_be_disabled_explicitly() {
        for value in ["0", "false", "FALSE", "no", "NO"] {
            assert!(
                !blocked_stream_backoff_enabled_from_env(Some(value)),
                "expected {value} to disable blocked-stream backoff"
            );
        }
    }

    #[test]
    fn blocked_stream_backoff_stays_enabled_for_true_values() {
        for value in ["1", "true", "TRUE", "yes", "YES", "maybe"] {
            assert!(
                blocked_stream_backoff_enabled_from_env(Some(value)),
                "expected {value} to keep blocked-stream backoff enabled"
            );
        }
    }

    #[test]
    fn blocked_stream_backoff_starts_with_yield() {
        assert_eq!(
            blocked_stream_backoff_for_streak(1),
            BlockedStreamBackoff::Yield
        );
        assert_eq!(
            blocked_stream_backoff_for_streak(63),
            BlockedStreamBackoff::Yield
        );
    }

    #[test]
    fn blocked_stream_backoff_escalates_to_short_sleep() {
        assert_eq!(
            blocked_stream_backoff_for_streak(64),
            BlockedStreamBackoff::Sleep(Duration::from_micros(50))
        );
        assert_eq!(
            blocked_stream_backoff_for_streak(511),
            BlockedStreamBackoff::Sleep(Duration::from_micros(50))
        );
    }

    #[test]
    fn blocked_stream_backoff_escalates_to_longer_sleep() {
        assert_eq!(
            blocked_stream_backoff_for_streak(512),
            BlockedStreamBackoff::Sleep(Duration::from_micros(200))
        );
    }

}
