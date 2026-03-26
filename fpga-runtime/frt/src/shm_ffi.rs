use frt_shm::SharedMemoryQueue;
use std::ffi::{c_char, c_int, c_void};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

struct QueueHandle {
    queue: Mutex<SharedMemoryQueue>,
}

fn with_handle<R>(handle: *const c_void, f: impl FnOnce(&QueueHandle) -> R) -> Option<R> {
    if handle.is_null() {
        return None;
    }
    let h = unsafe { &*(handle as *const QueueHandle) };
    Some(f(h))
}

fn write_c_string(buf: *mut c_char, buf_len: usize, text: &str) -> bool {
    if buf.is_null() || buf_len == 0 {
        return false;
    }
    let bytes = text.as_bytes();
    if bytes.len() + 1 > buf_len {
        return false;
    }
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf as *mut u8, bytes.len());
        *buf.add(bytes.len()) = 0;
    }
    true
}

#[no_mangle]
pub extern "C" fn frt_shmq_create(
    depth: u32,
    width: u32,
    out_path: *mut c_char,
    out_path_len: usize,
) -> *mut c_void {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let depth = depth.max(1);
    let width = width.max(1);
    let name = format!(
        "frt_stream_{}_{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    );
    let queue = match SharedMemoryQueue::create(&name, depth, width) {
        Ok(q) => q,
        Err(_) => return std::ptr::null_mut(),
    };
    let path = queue.path().to_string_lossy().to_string();
    if !write_c_string(out_path, out_path_len, &path) {
        return std::ptr::null_mut();
    }
    let handle = Box::new(QueueHandle {
        queue: Mutex::new(queue),
    });
    Box::into_raw(handle) as *mut c_void
}

#[no_mangle]
pub extern "C" fn frt_shmq_destroy(handle: *mut c_void) {
    if handle.is_null() {
        return;
    }
    unsafe {
        drop(Box::from_raw(handle as *mut QueueHandle));
    }
}

#[no_mangle]
pub extern "C" fn frt_shmq_empty(handle: *const c_void) -> c_int {
    with_handle(handle, |h| {
        let Ok(q) = h.queue.lock() else {
            return -1;
        };
        if q.is_empty() { 1 } else { 0 }
    })
    .unwrap_or(-1)
}

#[no_mangle]
pub extern "C" fn frt_shmq_full(handle: *const c_void) -> c_int {
    with_handle(handle, |h| {
        let Ok(q) = h.queue.lock() else {
            return -1;
        };
        if q.is_full() { 1 } else { 0 }
    })
    .unwrap_or(-1)
}

#[no_mangle]
pub extern "C" fn frt_shmq_push(handle: *mut c_void, data: *const u8, len: usize) -> c_int {
    if data.is_null() {
        return -1;
    }
    with_handle(handle as *const c_void, |h| {
        let Ok(mut q) = h.queue.lock() else {
            return -1;
        };
        if q.width() != len {
            return -1;
        }
        let slice = unsafe { std::slice::from_raw_parts(data, len) };
        if q.try_push(slice).is_ok() { 0 } else { -1 }
    })
    .unwrap_or(-1)
}

#[no_mangle]
pub extern "C" fn frt_shmq_front(handle: *const c_void, out: *mut u8, len: usize) -> c_int {
    if out.is_null() {
        return -1;
    }
    with_handle(handle, |h| {
        let Ok(q) = h.queue.lock() else {
            return -1;
        };
        if q.width() != len {
            return -1;
        }
        let Some(data) = q.peek() else {
            return -1;
        };
        unsafe {
            std::ptr::copy_nonoverlapping(data.as_ptr(), out, len);
        }
        0
    })
    .unwrap_or(-1)
}

#[no_mangle]
pub extern "C" fn frt_shmq_pop(handle: *mut c_void, out: *mut u8, len: usize) -> c_int {
    if out.is_null() {
        return -1;
    }
    with_handle(handle as *const c_void, |h| {
        let Ok(mut q) = h.queue.lock() else {
            return -1;
        };
        if q.width() != len {
            return -1;
        }
        let Some(data) = q.pop() else {
            return -1;
        };
        unsafe {
            std::ptr::copy_nonoverlapping(data.as_ptr(), out, len);
        }
        0
    })
    .unwrap_or(-1)
}
