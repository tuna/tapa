//! The testbench's side of the host's control-register requests (see
//! `frt_shm::control`).

use frt_shm::control::{REPLY_BYTES, REQUEST_BYTES};

use crate::context::DpiContext;

/// Pops the next request into `out` (`REQUEST_BYTES`); false when none is
/// waiting, or the kernel takes no requests.
pub fn ctrl_request_impl(ctx: &DpiContext, out: *mut u8) -> bool {
    let Some(control) = &ctx.control else {
        return false;
    };
    let mut queues = control.lock().expect("control queues");
    let mut request = [0u8; REQUEST_BYTES];
    if !queues.0.pop_into(&mut request) {
        return false;
    }
    // SAFETY: the testbench passes a buffer of `REQUEST_BYTES` bytes.
    unsafe { std::ptr::copy_nonoverlapping(request.as_ptr(), out, REQUEST_BYTES) };
    true
}

/// Pushes the reply at `data` (`REPLY_BYTES`), waiting for room.
pub fn ctrl_reply_impl(ctx: &DpiContext, data: *const u8) {
    let Some(control) = &ctx.control else {
        return;
    };
    let mut reply = [0u8; REPLY_BYTES];
    // SAFETY: the testbench passes a buffer of `REPLY_BYTES` bytes.
    unsafe { std::ptr::copy_nonoverlapping(data, reply.as_mut_ptr(), REPLY_BYTES) };
    let mut queues = control.lock().expect("control queues");
    while queues.1.try_push(&reply).is_err() {
        std::thread::yield_now();
    }
}

/// The memory of the mmap argument `port`, mapped from the host's segment,
/// and its size in `size`; null when there is none.
pub fn axi_memory_impl(ctx: &DpiContext, port: &str, size: &mut u64) -> *mut u8 {
    let Some((segment, _)) = ctx.buffers.get(port) else {
        *size = 0;
        return std::ptr::null_mut();
    };
    *size = segment.as_slice().len() as u64;
    segment.shared_ptr()
}
