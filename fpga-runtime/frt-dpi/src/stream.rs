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
