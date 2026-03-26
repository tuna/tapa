use crate::context::DpiContext;

pub fn stream_try_read_impl(ctx: &DpiContext, port: &str, out: *mut u8) -> bool {
    let Some(q) = ctx.streams.get(port) else {
        eprintln!("frt-dpi: stream_try_read: unknown port '{port}'");
        return false;
    };
    let mut q = match q.lock() {
        Ok(guard) => guard,
        Err(_) => return false,
    };
    if let Some(data) = q.pop() {
        unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), out, data.len()) }
        true
    } else {
        false
    }
}

pub fn stream_try_write_impl(ctx: &DpiContext, port: &str, data: *const u8) -> bool {
    let Some(q) = ctx.streams.get(port) else {
        eprintln!("frt-dpi: stream_try_write: unknown port '{port}'");
        return false;
    };
    let mut q = match q.lock() {
        Ok(guard) => guard,
        Err(_) => return false,
    };
    let w = q.width();
    let slice = unsafe { std::slice::from_raw_parts(data, w) };
    q.try_push(slice).is_ok()
}

pub fn stream_can_write_impl(ctx: &DpiContext, port: &str) -> bool {
    let Some(q) = ctx.streams.get(port) else {
        eprintln!("frt-dpi: stream_can_write: unknown port '{port}'");
        return false;
    };
    let q = match q.lock() {
        Ok(guard) => guard,
        Err(_) => return false,
    };
    !q.is_full()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::DpiContext;
    use frt_shm::SharedMemoryQueue;
    use std::collections::HashMap;
    use std::sync::Mutex;

    fn make_ctx_with_stream(name: &str, depth: u32, width: u32) -> DpiContext {
        let q = SharedMemoryQueue::create(name, depth, width).expect("create stream queue");
        DpiContext {
            buffers: HashMap::new(),
            streams: HashMap::from([(name.to_owned(), Mutex::new(q))]),
        }
    }

    #[test]
    fn can_write_false_when_full() {
        let ctx = make_ctx_with_stream("stream_can_write_full", 2, 4);
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
        let ctx = make_ctx_with_stream("stream_can_write_pop", 1, 4);
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
}
