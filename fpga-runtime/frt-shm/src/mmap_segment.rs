use memmap2::MmapMut;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub struct MmapSegment {
    path: PathBuf,
    mmap: MmapMut,
    owner: bool,
}

impl MmapSegment {
    pub fn create(name: &str, size_bytes: usize) -> std::io::Result<Self> {
        let path = shm_path(name);
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)?;
        file.set_len(size_bytes as u64)?;
        // SAFETY: The file was just created, truncated, and sized; we hold the only handle.
        let mmap = unsafe { MmapMut::map_mut(&file)? };
        Ok(Self {
            path,
            mmap,
            owner: true,
        })
    }

    pub fn open(path: &str, _size_bytes: usize) -> std::io::Result<Self> {
        let path = PathBuf::from(path);
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)?;
        // SAFETY: The file exists and is opened read-write; the caller ensures correct size.
        let mmap = unsafe { MmapMut::map_mut(&file)? };
        Ok(Self {
            path,
            mmap,
            owner: false,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.mmap
    }

    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        &mut self.mmap
    }

    /// Write `len` bytes from `src` into the mmap at `offset` without
    /// requiring `&mut self`.  This is correct for `MAP_SHARED` memory
    /// that is concurrently accessed by another process (the simulator).
    ///
    /// # Safety
    /// `src` must be valid for `len` bytes and `offset + len` must not
    /// exceed the mapping length.
    pub unsafe fn write_at(&self, offset: usize, src: *const u8, len: usize) {
        debug_assert!(offset + len <= self.mmap.len(), "write_at out of bounds");
        // SAFETY: Caller guarantees `offset + len <= mmap.len()`, so the
        // pointer arithmetic stays within the allocation.  We derive from
        // `MmapMut::as_ptr` (not `&[u8]`) to avoid aliasing issues on
        // MAP_SHARED memory.
        let dst = unsafe { self.mmap.as_ptr().add(offset) };
        // SAFETY: Caller guarantees `src` is valid for `len` bytes and the
        // destination range is in-bounds (checked above via debug_assert).
        unsafe { std::ptr::copy_nonoverlapping(src, dst.cast_mut(), len) };
    }

    pub fn len(&self) -> usize {
        self.mmap.len()
    }

    pub fn is_empty(&self) -> bool {
        self.mmap.is_empty()
    }
}

impl Drop for MmapSegment {
    fn drop(&mut self) {
        if self.owner {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

static SHM_COUNTER: AtomicU64 = AtomicU64::new(0);

fn shm_path(name: &str) -> PathBuf {
    let pid = std::process::id();
    let nonce = SHM_COUNTER.fetch_add(1, Ordering::Relaxed);
    let safe_name: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    #[cfg(target_os = "linux")]
    {
        PathBuf::from(format!("/dev/shm/tapa_{}_{}_{}", safe_name, pid, nonce))
    }
    #[cfg(not(target_os = "linux"))]
    {
        std::env::temp_dir().join(format!("tapa_{safe_name}_{pid}_{nonce}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_write_read() {
        let mut seg = MmapSegment::create("test_rw", 64).expect("create");
        seg.as_mut_slice()[..4].copy_from_slice(b"tapa");
        assert_eq!(&seg.as_slice()[..4], b"tapa");
    }

    #[test]
    fn test_drop_unlinks() {
        let path = {
            let seg = MmapSegment::create("test_drop", 16).expect("create");
            seg.path().to_owned()
        };
        assert!(!path.exists(), "shm file should be unlinked on drop");
    }

    #[test]
    fn test_open_existing() {
        let seg = MmapSegment::create("test_open", 32).expect("create");
        let path = seg.path().to_str().expect("utf8").to_owned();
        let mapped = MmapSegment::open(&path, 32).expect("open");
        assert_eq!(mapped.as_slice().len(), 32);
    }
}
