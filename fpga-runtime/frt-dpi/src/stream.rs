use crate::context::DpiContext;
use std::sync::atomic::{AtomicU64, Ordering};

static STREAM_DBG: std::sync::Once = std::sync::Once::new();
static mut STREAM_DBG_ENABLED: bool = false;

fn stream_debug_enabled() -> bool {
    STREAM_DBG.call_once(|| {
        unsafe { STREAM_DBG_ENABLED = std::env::var("FRT_STREAM_DEBUG").is_ok() };
    });
    unsafe { STREAM_DBG_ENABLED }
}

fn maybe_yield() {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *ENABLED.get_or_init(|| std::env::var("FRT_COSIM_YIELD").is_ok()) {
        std::thread::yield_now();
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
        READ_OK.fetch_add(1, Ordering::Relaxed);
        if stream_debug_enabled() {
            eprintln!(
                "frt-dpi: stream_try_read '{port}': got {} bytes",
                data.len()
            );
        }
        let copy_len = data.len().min(stream.dpi_width_bytes);
        unsafe {
            std::ptr::write_bytes(out, 0, stream.dpi_width_bytes);
            std::ptr::copy_nonoverlapping(data.as_ptr(), out, copy_len);
        }
        true
    } else {
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
        WRITE_OK.fetch_add(1, Ordering::Relaxed);
        if stream_debug_enabled() {
            eprintln!("frt-dpi: stream_try_write '{port}': wrote {w} bytes");
        }
    }
    ok
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
    if !can {
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
}
