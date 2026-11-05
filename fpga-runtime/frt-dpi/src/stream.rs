use crate::context::{DpiContext, DpiStream, DpiStreamInner};
use std::sync::atomic::{AtomicU64, Ordering};

impl DpiStreamInner {
    fn pop(&mut self) -> bool {
        self.queue.pop_into(&mut self.bytes)
    }

    fn peek(&mut self) -> bool {
        self.queue.peek_into(&mut self.bytes)
    }

    fn copy_to_dpi(&self, buf: &mut [u8]) {
        let n = buf.len().min(self.bytes.len());
        buf[..n].copy_from_slice(&self.bytes[..n]);
        // Queues without an explicit EOS/TLAST byte keep that DPI byte zero.
        buf[n..].fill(0);
    }

    fn push_from_dpi(
        &mut self,
        data: *const u8,
        dpi_width_bytes: usize,
    ) -> Result<bool, &'static str> {
        let n = self.bytes.len().min(dpi_width_bytes);
        // SAFETY: `data` is a DPI buffer of at least `dpi_width_bytes` bytes;
        // only its prefix fitting in the host token is accessed.
        let src = unsafe { std::slice::from_raw_parts(data, n) };
        self.bytes[..n].copy_from_slice(src);
        // C++ elem_t can have trailing object padding after its EOS/TLAST byte.
        self.bytes[n..].fill(0);
        self.queue.try_push_check_full(&self.bytes)
    }
}

fn stream_debug_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| frt_shm::env_bool(frt_shm::env::FRT_STREAM_DEBUG))
}

fn cosim_yield_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| frt_shm::env_bool_default_true(frt_shm::env::FRT_COSIM_YIELD))
}

fn maybe_yield() {
    if cosim_yield_enabled() {
        // Sleep briefly to yield CPU to other cosim processes so they can
        // drain/fill shared queues.  Without this, 7+ xsim processes all
        // spinning on empty/full queues starve the productive processes.
        std::thread::sleep(std::time::Duration::from_micros(1));
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

/// The stream for `port` with its state locked, or None after reporting.
fn lock_stream<'c>(
    ctx: &'c DpiContext,
    port: &str,
    op: &str,
) -> Option<(&'c DpiStream, std::sync::MutexGuard<'c, DpiStreamInner>)> {
    let Some(stream) = ctx.streams.get(port) else {
        eprintln!("frt-dpi: {op}: unknown port '{port}'");
        return None;
    };
    let Ok(s) = stream.inner.lock() else {
        return None;
    };
    Some((stream, s))
}

/// The can-write verdict after accounting for a full queue.
fn write_ready(s: &DpiStreamInner, port: &str, known_full: bool) -> bool {
    let can = !known_full && !s.queue.is_full();
    if !can {
        WRITE_FULL.fetch_add(1, Ordering::Relaxed);
        maybe_report_progress(port, &s.queue);
        maybe_yield();
    }
    can
}

pub fn stream_try_read_impl(ctx: &DpiContext, port: &str, out: *mut u8) -> bool {
    let Some((stream, mut s)) = lock_stream(ctx, port, "stream_try_read") else {
        return false;
    };
    let dpi = stream.dpi_width_bytes;
    // SAFETY: `out` is a DPI-provided buffer of at least `dpi` bytes.
    let buf = unsafe { std::slice::from_raw_parts_mut(out, dpi) };
    // pop_into already checks emptiness internally, so calling it directly
    // avoids a redundant pair of atomic loads vs a separate is_empty() check.
    if !s.pop() {
        READ_MISS.fetch_add(1, Ordering::Relaxed);
        maybe_report_progress(port, &s.queue);
        maybe_yield();
        return false;
    }
    s.copy_to_dpi(buf);
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
    let Some((stream, mut s)) = lock_stream(ctx, port, "stream_istream_step") else {
        return false;
    };

    if s.last_istream_valid && consume {
        // Consume the previously-peeked front element; the payload is discarded.
        if s.pop() {
            READ_OK.fetch_add(1, Ordering::Relaxed);
        }
    }

    let dpi = stream.dpi_width_bytes;
    // SAFETY: `out` is a DPI-provided buffer of at least `dpi` bytes.
    let buf = unsafe { std::slice::from_raw_parts_mut(out, dpi) };
    // peek_into already checks emptiness internally, avoiding redundant atomic loads.
    if !s.peek() {
        s.last_istream_valid = false;
        READ_MISS.fetch_add(1, Ordering::Relaxed);
        maybe_report_progress(port, &s.queue);
        maybe_yield();
        return false;
    }
    s.copy_to_dpi(buf);
    s.last_istream_valid = true;
    true
}

pub fn stream_try_write_impl(ctx: &DpiContext, port: &str, data: *const u8) -> bool {
    let Some((stream, mut s)) = lock_stream(ctx, port, "stream_try_write") else {
        return false;
    };
    let ok = s.push_from_dpi(data, stream.dpi_width_bytes).is_ok();
    if ok {
        WRITE_OK.fetch_add(1, Ordering::Relaxed);
        if stream_debug_enabled() {
            eprintln!(
                "frt-dpi: stream_try_write '{port}': wrote {} bytes",
                s.queue.width()
            );
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
    let Some((stream, mut s)) = lock_stream(ctx, port, "stream_ostream_step") else {
        return false;
    };

    let mut known_full = false;
    if s.last_ostream_ready && write {
        match s.push_from_dpi(data, stream.dpi_width_bytes) {
            Ok(full_after) => {
                WRITE_OK.fetch_add(1, Ordering::Relaxed);
                known_full = full_after;
                if stream_debug_enabled() {
                    eprintln!(
                        "frt-dpi: stream_ostream_step '{port}': wrote {} bytes",
                        s.queue.width()
                    );
                }
            }
            Err(_) => {
                known_full = true;
            }
        }
    }

    let can = write_ready(&s, port, known_full);
    s.last_ostream_ready = can;
    can
}

pub fn stream_hls_ostream_step_impl(
    ctx: &DpiContext,
    port: &str,
    write: bool,
    data: *const u8,
) -> bool {
    let Some((stream, mut s)) = lock_stream(ctx, port, "stream_hls_ostream_step") else {
        return false;
    };

    // HLS ap_fifo/full_n uses the queue state visible in the current cycle,
    // unlike AXIS tready which is sampled from the prior cycle in the Vitis path.
    let mut known_full = false;
    if write {
        if let Ok(full_after) = s.push_from_dpi(data, stream.dpi_width_bytes) {
            WRITE_OK.fetch_add(1, Ordering::Relaxed);
            known_full = full_after;
            if stream_debug_enabled() {
                let (h, t) = s.queue.head_tail();
                eprintln!(
                    "frt-dpi: stream_hls_ostream_step '{port}': wrote {} bytes (h={h} t={t})",
                    s.queue.width()
                );
            }
        } else {
            known_full = true;
            let (h, t) = s.queue.head_tail();
            eprintln!(
                "frt-dpi: WARN '{port}': write=1 but queue full! h={h} t={t} depth={}",
                s.queue.depth()
            );
        }
    }

    write_ready(&s, port, known_full)
}

pub fn stream_can_write_impl(ctx: &DpiContext, port: &str) -> bool {
    let Some((_, s)) = lock_stream(ctx, port, "stream_can_write") else {
        return false;
    };
    write_ready(&s, port, false)
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
                        bytes: vec![0; q.width()],
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
        let port = "stream_try_read_preserves_tail";
        for width in [5, 8] {
            let ctx = make_ctx_with_stream(port, 1, width, 5);
            let in_bytes = [1u8, 2, 3, 4, 1];
            let mut host_bytes = vec![0xee; width as usize];
            host_bytes[..5].copy_from_slice(&in_bytes);
            ctx.streams[port]
                .inner
                .lock()
                .expect("stream lock")
                .queue
                .try_push(&host_bytes)
                .expect("host push");

            let mut out = [0xffu8; 7];
            assert!(stream_try_read_impl(&ctx, port, out[1..6].as_mut_ptr()));
            assert_eq!(&out[1..6], &in_bytes);
            assert_eq!(out[0], 0xff);
            assert_eq!(out[6], 0xff);

            assert!(stream_try_write_impl(&ctx, port, in_bytes.as_ptr()));
            assert!(ctx.streams[port]
                .inner
                .lock()
                .expect("stream lock")
                .queue
                .pop_into(&mut host_bytes));
            assert_eq!(&host_bytes[..5], &in_bytes);
            assert!(host_bytes[5..].iter().all(|&byte| byte == 0));
        }
    }

    #[test]
    fn istream_step_holds_front_until_consumed_and_refills_same_cycle() {
        let port = "stream_istream_step_holds_front";
        for width in [5, 8] {
            let ctx = make_ctx_with_stream(port, 4, width, 5);
            let first = [1, 0, 0, 0, 0];
            let second = [2, 0, 0, 0, 1];
            assert!(stream_try_write_impl(&ctx, port, first.as_ptr()));
            assert!(stream_try_write_impl(&ctx, port, second.as_ptr()));

            let mut out = [0u8; 5];
            assert!(stream_istream_step_impl(
                &ctx,
                port,
                false,
                out.as_mut_ptr()
            ));
            assert_eq!(out, first);

            out.fill(0);
            assert!(stream_istream_step_impl(
                &ctx,
                port,
                false,
                out.as_mut_ptr()
            ));
            assert_eq!(out, first);

            out.fill(0);
            assert!(stream_istream_step_impl(&ctx, port, true, out.as_mut_ptr()));
            assert_eq!(out, second);

            out.fill(0);
            assert!(!stream_istream_step_impl(
                &ctx,
                port,
                true,
                out.as_mut_ptr()
            ));
        }
    }

    #[test]
    fn ostream_step_only_commits_data_after_prior_ready_cycle() {
        let port = "stream_ostream_step_prior_ready";
        for width in [5, 8] {
            let ctx = make_ctx_with_stream(port, 1, width, 5);
            let first = [7, 0, 0, 0, 1];

            assert!(stream_ostream_step_impl(&ctx, port, false, first.as_ptr()));
            let mut out = [0u8; 5];
            assert!(!stream_try_read_impl(&ctx, port, out.as_mut_ptr()));

            assert!(!stream_ostream_step_impl(&ctx, port, true, first.as_ptr()));
            assert!(stream_try_read_impl(&ctx, port, out.as_mut_ptr()));
            assert_eq!(out, first);
        }
    }

    #[test]
    fn hls_ostream_step_commits_immediately_when_queue_is_ready() {
        let port = "stream_hls_ostream_step_immediate";
        for width in [5, 8] {
            let ctx = make_ctx_with_stream(port, 1, width, 5);
            let first = [11, 0, 0, 0, 1];
            let mut out = [0u8; 5];
            assert!(!stream_try_read_impl(&ctx, port, out.as_mut_ptr()));

            assert!(!stream_hls_ostream_step_impl(
                &ctx,
                port,
                true,
                first.as_ptr()
            ));

            assert!(stream_try_read_impl(&ctx, port, out.as_mut_ptr()));
            assert_eq!(out, first);
        }
    }

    #[test]
    fn hls_ostream_step_does_not_drop_first_write_after_queue_recovers() {
        let port = "stream_hls_ostream_step_recovery";
        for width in [5, 8] {
            let ctx = make_ctx_with_stream(port, 1, width, 5);
            let first = [21, 0, 0, 0, 0];
            let second = [22, 0, 0, 0, 1];

            assert!(!stream_hls_ostream_step_impl(
                &ctx,
                port,
                true,
                first.as_ptr()
            ));

            let mut out = [0u8; 5];
            assert!(stream_try_read_impl(&ctx, port, out.as_mut_ptr()));
            assert_eq!(out, first);

            assert!(!stream_hls_ostream_step_impl(
                &ctx,
                port,
                true,
                second.as_ptr()
            ));
            out.fill(0);
            assert!(stream_try_read_impl(&ctx, port, out.as_mut_ptr()));
            assert_eq!(out, second);
        }
    }
}
