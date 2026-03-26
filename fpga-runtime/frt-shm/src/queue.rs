use crate::MmapSegment;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

#[repr(C)]
pub struct QueueHeader {
    magic: [u8; 4],
    version: i32,
    depth: u32,
    width: u32,
    tail: AtomicU64,
    head: AtomicU64,
}

const _: () = assert!(std::mem::size_of::<QueueHeader>() == 32);

pub struct SharedMemoryQueue {
    seg: MmapSegment,
}

impl SharedMemoryQueue {
    pub fn create(name: &str, depth: u32, width: u32) -> std::io::Result<Self> {
        let size = 32 + (depth as usize) * (width as usize);
        let mut seg = MmapSegment::create(name, size)?;
        let hdr = unsafe { &mut *(seg.as_mut_slice().as_mut_ptr() as *mut QueueHeader) };
        hdr.magic = *b"tapa";
        hdr.version = 0;
        hdr.depth = depth;
        hdr.width = width;
        hdr.tail.store(0, Ordering::Relaxed);
        hdr.head.store(0, Ordering::Relaxed);
        Ok(Self { seg })
    }

    pub fn open(path: &str) -> std::io::Result<Self> {
        let seg = MmapSegment::open(path, 0)?;
        Ok(Self { seg })
    }

    fn hdr(&self) -> &QueueHeader {
        unsafe { &*(self.seg.as_slice().as_ptr() as *const QueueHeader) }
    }

    fn data_ptr(&self) -> *const u8 {
        unsafe { self.seg.as_slice().as_ptr().add(32) }
    }

    fn data_mut_ptr(&mut self) -> *mut u8 {
        unsafe { self.seg.as_mut_slice().as_mut_ptr().add(32) }
    }

    pub fn depth(&self) -> u64 {
        self.hdr().depth as u64
    }

    pub fn width(&self) -> usize {
        self.hdr().width as usize
    }

    pub fn is_empty(&self) -> bool {
        let h = self.hdr();
        h.tail.load(Ordering::Acquire) == h.head.load(Ordering::Acquire)
    }

    pub fn is_full(&self) -> bool {
        let h = self.hdr();
        h.tail.load(Ordering::Acquire) - h.head.load(Ordering::Acquire) >= self.depth()
    }

    pub fn try_push(&mut self, data: &[u8]) -> Result<(), &'static str> {
        if self.is_full() {
            return Err("queue full");
        }
        let tail = self.hdr().tail.load(Ordering::Relaxed);
        let slot = (tail % self.depth()) as usize * self.width();
        let w = self.width();
        let len = data.len().min(w);
        let dst = unsafe { self.data_mut_ptr().add(slot) };
        unsafe {
            std::ptr::copy_nonoverlapping(data.as_ptr(), dst, len);
            if len < w {
                std::ptr::write_bytes(dst.add(len), 0, w - len);
            }
        }
        self.hdr().tail.store(tail + 1, Ordering::Release);
        Ok(())
    }

    pub fn push(&mut self, data: &[u8]) -> Result<(), &'static str> {
        self.try_push(data)
    }

    pub fn pop(&mut self) -> Option<Vec<u8>> {
        if self.is_empty() {
            return None;
        }
        let head = self.hdr().head.load(Ordering::Relaxed);
        let slot = (head % self.depth()) as usize * self.width();
        let w = self.width();
        let mut out = vec![0u8; w];
        unsafe {
            std::ptr::copy_nonoverlapping(self.data_ptr().add(slot), out.as_mut_ptr(), w);
        }
        self.hdr().head.store(head + 1, Ordering::Release);
        Some(out)
    }

    pub fn path(&self) -> &Path {
        self.seg.path()
    }
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
        q.push(b"abcd").expect("push");
        assert!(!q.is_empty());
        let got = q.pop().expect("pop");
        assert_eq!(got, b"abcd");
        assert!(q.is_empty());
    }

    #[test]
    fn full_blocks_push() {
        let mut q = SharedMemoryQueue::create("test_q_full", 2, 4).expect("create");
        q.push(b"aaaa").expect("push");
        q.push(b"bbbb").expect("push");
        assert!(q.is_full());
        assert!(q.try_push(b"cccc").is_err());
    }

    #[test]
    fn wraparound() {
        let mut q = SharedMemoryQueue::create("test_q_wrap", 4, 2).expect("create");
        for i in 0u8..4 {
            q.push(&[i, i]).expect("push");
        }
        for i in 0u8..4 {
            assert_eq!(q.pop().expect("pop"), vec![i, i]);
        }
        q.push(b"xy").expect("push");
        assert_eq!(q.pop().expect("pop"), b"xy");
    }
}
