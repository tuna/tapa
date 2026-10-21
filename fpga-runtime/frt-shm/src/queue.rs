use crate::MmapSegment;
use std::ops::Range;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

#[repr(C)]
pub(crate) struct QueueHeader {
    magic: [u8; 4],
    version: i32,
    depth: u32,
    width: u32,
    tail: AtomicU64,
    head: AtomicU64,
}

const _: () = assert!(
    std::mem::size_of::<QueueHeader>() == 32,
    "QueueHeader must be exactly 32 bytes for shared-memory ABI"
);

pub struct SharedMemoryQueue {
    seg: MmapSegment,
    layout: QueueLayout,
}

struct QueueLayout {
    depth: u64,
    width: usize,
    byte_len: usize,
}

impl QueueLayout {
    fn new(depth: u32, width: u32) -> std::io::Result<Self> {
        if depth == 0 || width == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("queue needs depth >= 1 and width >= 1, got depth={depth} width={width}"),
            ));
        }
        let byte_len = (depth as usize)
            .checked_mul(width as usize)
            .and_then(|len| len.checked_add(std::mem::size_of::<QueueHeader>()))
            .filter(|len| isize::try_from(*len).is_ok())
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("queue shape is too large: depth={depth} width={width}"),
                )
            })?;
        Ok(Self {
            depth: u64::from(depth),
            width: width as usize,
            byte_len,
        })
    }

    fn slot(&self, index: u64) -> Range<usize> {
        let start = std::mem::size_of::<QueueHeader>() + (index % self.depth) as usize * self.width;
        start..start + self.width
    }
}

impl SharedMemoryQueue {
    pub fn create(name: &str, depth: u32, width: u32) -> std::io::Result<Self> {
        let layout = QueueLayout::new(depth, width)?;
        let mut seg = MmapSegment::create(name, layout.byte_len)?;
        // SAFETY: The segment is at least 32 bytes (the header size) and
        // freshly created with zeroed memory, so the cast is aligned and in-bounds.
        let hdr = unsafe { &mut *seg.as_mut_slice().as_mut_ptr().cast::<QueueHeader>() };
        hdr.magic = *b"tapa";
        hdr.version = 1;
        hdr.depth = depth;
        hdr.width = width;
        hdr.tail.store(0, Ordering::Relaxed);
        hdr.head.store(0, Ordering::Relaxed);
        Ok(Self { seg, layout })
    }

    pub fn open(path: &str) -> std::io::Result<Self> {
        let seg = MmapSegment::open(path)?;
        let layout = validate_header(seg.as_slice())?;
        Ok(Self { seg, layout })
    }

    fn hdr(&self) -> &QueueHeader {
        // SAFETY: The segment is always at least 32 bytes (header size) and
        // was either initialised by `create` or validated by `open`.
        unsafe { &*self.seg.as_slice().as_ptr().cast::<QueueHeader>() }
    }

    pub fn depth(&self) -> u64 {
        self.layout.depth
    }

    pub fn width(&self) -> usize {
        self.layout.width
    }

    pub fn head_tail(&self) -> (u64, u64) {
        let h = self.hdr();
        // Read head first: concurrent progress cannot produce a snapshot
        // whose head is newer than its tail when both counters only increase.
        (
            h.head.load(Ordering::Acquire),
            h.tail.load(Ordering::Acquire),
        )
    }

    pub fn is_empty(&self) -> bool {
        let h = self.hdr();
        h.tail.load(Ordering::Acquire) == h.head.load(Ordering::Acquire)
    }

    pub fn is_full(&self) -> bool {
        let (head, tail) = self.head_tail();
        tail - head >= self.layout.depth
    }

    pub fn try_push(&mut self, data: &[u8]) -> Result<(), &'static str> {
        self.try_push_check_full(data).map(|_| ())
    }

    /// Push `data` and return whether the queue is full after the push.
    /// Returns `Err` if the queue was already full or `data` has wrong length.
    pub fn try_push_check_full(&mut self, data: &[u8]) -> Result<bool, &'static str> {
        let h = self.hdr();
        let tail = h.tail.load(Ordering::Acquire);
        let head = h.head.load(Ordering::Acquire);
        if tail - head >= self.layout.depth {
            return Err("the queue is full");
        }
        if data.len() != self.layout.width {
            return Err("the input size does not match the queue width");
        }
        self.seg.as_mut_slice()[self.layout.slot(tail)].copy_from_slice(data);
        let new_tail = tail + 1;
        self.hdr().tail.store(new_tail, Ordering::Release);
        Ok(new_tail - head >= self.layout.depth)
    }

    /// Pop the front element directly into `buf`, returning `true` on success.
    /// Returns `false` without consuming if empty or `buf` is shorter than
    /// `self.width()` bytes. Only `width` bytes are written.
    pub fn pop_into(&mut self, buf: &mut [u8]) -> bool {
        let Some(head) = self.read_front(buf) else {
            return false;
        };
        self.hdr().head.store(head + 1, Ordering::Release);
        true
    }

    /// Peek the front element directly into `buf`, returning `true` on success.
    /// Returns `false` if empty or `buf` is shorter than `self.width()` bytes.
    /// Only `width` bytes are written.
    pub fn peek_into(&self, buf: &mut [u8]) -> bool {
        self.read_front(buf).is_some()
    }

    fn read_front(&self, buf: &mut [u8]) -> Option<u64> {
        let h = self.hdr();
        let head = h.head.load(Ordering::Acquire);
        if h.tail.load(Ordering::Acquire) == head {
            return None;
        }
        let dst = buf.get_mut(..self.layout.width)?;
        dst.copy_from_slice(&self.seg.as_slice()[self.layout.slot(head)]);
        Some(head)
    }

    pub fn path(&self) -> &Path {
        self.seg.path()
    }
}

fn validate_header(bytes: &[u8]) -> std::io::Result<QueueLayout> {
    if bytes.len() < std::mem::size_of::<QueueHeader>() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("shared-memory queue file is too small: {}", bytes.len()),
        ));
    }
    // SAFETY: `bytes` starts at the page-aligned mapping opened above, and
    // contains at least `size_of::<QueueHeader>()` bytes.
    let hdr = unsafe { &*bytes.as_ptr().cast::<QueueHeader>() };
    if hdr.magic != *b"tapa" {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "unexpected magic `{:?}`; expected `tapa`",
                std::str::from_utf8(&hdr.magic).unwrap_or("????")
            ),
        ));
    }
    if hdr.version != 1 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("unsupported queue version {}; expected 1", hdr.version),
        ));
    }
    let layout = QueueLayout::new(hdr.depth, hdr.width)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    if bytes.len() < layout.byte_len {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "shared-memory queue payload is too small: {} bytes; need {}",
                bytes.len(),
                layout.byte_len
            ),
        ));
    }
    Ok(layout)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::size_of;

    #[test]
    fn header_size_is_32() {
        assert_eq!(size_of::<QueueHeader>(), 32);
    }

    #[test]
    fn push_pop_roundtrip() {
        let mut q = SharedMemoryQueue::create("test_q_pp", 8, 4).expect("create");
        assert!(q.is_empty());
        q.try_push(b"abcd").expect("push");
        assert!(!q.is_empty());
        let mut got = [0u8; 4];
        assert!(q.pop_into(&mut got));
        assert_eq!(&got, b"abcd");
        assert!(q.is_empty());
    }

    #[test]
    fn full_blocks_push() {
        let mut q = SharedMemoryQueue::create("test_q_full", 2, 4).expect("create");
        q.try_push(b"aaaa").expect("push");
        q.try_push(b"bbbb").expect("push");
        assert!(q.is_full());
        assert!(q.try_push(b"cccc").is_err());
    }

    #[test]
    fn wraparound() {
        let mut q = SharedMemoryQueue::create("test_q_wrap", 4, 2).expect("create");
        for i in 0u8..4 {
            q.try_push(&[i, i]).expect("push");
        }
        for i in 0u8..4 {
            let mut got = [0u8; 2];
            assert!(q.pop_into(&mut got));
            assert_eq!(got, [i, i]);
        }
        q.try_push(b"xy").expect("push");
        let mut got = [0u8; 2];
        assert!(q.pop_into(&mut got));
        assert_eq!(&got, b"xy");
    }

    #[test]
    fn peek_does_not_consume() {
        let mut q = SharedMemoryQueue::create("test_q_peek", 4, 2).expect("create");
        q.try_push(b"ab").expect("push");
        let mut buf = [0u8; 2];
        assert!(q.peek_into(&mut buf));
        assert_eq!(&buf, b"ab");
        assert!(q.pop_into(&mut buf));
        assert_eq!(&buf, b"ab");
    }

    #[test]
    fn short_reads_preserve_the_token_and_long_reads_preserve_the_suffix() {
        let mut q = SharedMemoryQueue::create("test_q_read_width", 2, 4).expect("create");
        q.try_push(b"abcd").expect("push");

        let mut short = [0xee; 3];
        assert!(!q.peek_into(&mut short));
        assert!(!q.pop_into(&mut short));
        assert_eq!(short, [0xee; 3]);
        assert!(!q.is_empty());

        let mut long = [0xee; 6];
        assert!(q.peek_into(&mut long));
        assert_eq!(&long, b"abcd\xee\xee");
        assert!(!q.is_empty());
        long.fill(0xee);
        assert!(q.pop_into(&mut long));
        assert_eq!(&long, b"abcd\xee\xee");
        assert!(q.is_empty());
    }

    #[test]
    fn open_rejects_invalid_header() {
        let seg = MmapSegment::create("test_q_invalid", 32).expect("create");
        let path = seg.path().to_owned();
        std::fs::write(&path, [0u8; 32]).expect("write invalid queue");
        let path_str = path.to_str().expect("utf8").to_owned();
        match SharedMemoryQueue::open(&path_str) {
            Ok(_) => panic!("open should fail"),
            Err(err) => assert_eq!(err.kind(), std::io::ErrorKind::InvalidData),
        }
    }

    #[test]
    fn open_rejects_a_valid_header_with_truncated_payload() {
        let q = SharedMemoryQueue::create("test_q_payload_source", 2, 4).expect("create");
        let mut seg = MmapSegment::create("test_q_truncated_payload", q.seg.len() - 1)
            .expect("create truncated segment");
        let len = seg.len();
        seg.as_mut_slice().copy_from_slice(&q.seg.as_slice()[..len]);

        match SharedMemoryQueue::open(seg.path().to_str().expect("utf8")) {
            Ok(_) => panic!("a queue must contain every slot its header declares"),
            Err(err) => assert_eq!(err.kind(), std::io::ErrorKind::InvalidData),
        }
    }

    /// Reject shapes that cannot address valid slots before creating a mapping.
    #[test]
    fn create_rejects_invalid_shapes() {
        for (depth, width) in [(0, 4), (2, 0), (u32::MAX, u32::MAX)] {
            match SharedMemoryQueue::create("test_q_invalid_shape", depth, width) {
                Ok(_) => panic!("depth={depth} width={width} must be rejected"),
                Err(err) => assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput),
            }
        }
    }

    #[test]
    fn create_uses_version_one() {
        let q = SharedMemoryQueue::create("test_q_version", 2, 4).expect("create");
        assert_eq!(q.hdr().version, 1);
        let reopened = SharedMemoryQueue::open(q.path().to_str().expect("utf8")).expect("open");
        assert_eq!(reopened.hdr().version, 1);
    }

    #[test]
    fn concurrent_full_probe_tolerates_queue_progress() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::{Arc, Barrier};

        let q = SharedMemoryQueue::create("test_q_concurrent_full", 1, 1).expect("create");
        let path = q.path().to_str().expect("utf8").to_owned();
        let mut producer = SharedMemoryQueue::open(&path).expect("open producer");
        let mut consumer = SharedMemoryQueue::open(&path).expect("open consumer");
        let observer = SharedMemoryQueue::open(&path).expect("open observer");
        let barrier = Arc::new(Barrier::new(4));
        let done = Arc::new(AtomicBool::new(false));

        std::thread::scope(|scope| {
            let producer_barrier = Arc::clone(&barrier);
            let producer_done = Arc::clone(&done);
            scope.spawn(move || {
                producer_barrier.wait();
                for _ in 0..20_000 {
                    let pending = [1];
                    loop {
                        match producer.try_push(&pending) {
                            Ok(()) => break,
                            Err("queue full") => std::thread::yield_now(),
                            Err(error) => panic!("push failed: {error}"),
                        }
                    }
                }
                producer_done.store(true, Ordering::Release);
            });

            let consumer_barrier = Arc::clone(&barrier);
            scope.spawn(move || {
                consumer_barrier.wait();
                let mut got = [0u8; 1];
                for _ in 0..20_000 {
                    while !consumer.pop_into(&mut got) {
                        std::thread::yield_now();
                    }
                }
            });

            let observer_barrier = Arc::clone(&barrier);
            scope.spawn(move || {
                observer_barrier.wait();
                while !done.load(Ordering::Acquire) {
                    let _ = observer.is_full();
                }
            });
            barrier.wait();
        });
    }

    #[test]
    fn push_rejects_short_or_long_payloads() {
        let mut q = SharedMemoryQueue::create("test_q_exact", 2, 4).expect("create");
        assert!(q.try_push(b"abc").is_err());
        assert!(q.try_push(b"abcde").is_err());
        q.try_push(b"abcd")
            .expect("exact-width push should succeed");
    }
}
