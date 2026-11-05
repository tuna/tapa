use crate::context::DpiContext;
use frt_shm::MmapSegment;

/// Resolve `(segment, byte offset)` for `addr` on `port`, reporting `op`'s
/// failure mode and returning None.
fn resolve_span<'c>(
    ctx: &'c DpiContext,
    port: &str,
    addr: u64,
    len: usize,
    op: &str,
) -> Option<(&'c MmapSegment, usize)> {
    let Some((seg, base)) = ctx.buffers.get(port) else {
        eprintln!("frt-dpi: {op}: unknown port '{port}'");
        return None;
    };
    let Some(offset) = addr
        .checked_sub(*base)
        .and_then(|v| usize::try_from(v).ok())
    else {
        eprintln!("frt-dpi: {op}: invalid addr on '{port}'");
        return None;
    };
    if offset.checked_add(len).is_none_or(|end| end > seg.len()) {
        eprintln!("frt-dpi: {op}: out of bounds on '{port}' offset={offset} len={len}");
        return None;
    }
    Some((seg, offset))
}

pub fn axi_read_impl(ctx: &DpiContext, port: &str, addr: u64, width: u32, out: *mut u8) {
    let len = width as usize;
    let Some((seg, offset)) = resolve_span(ctx, port, addr, len, "axi_read") else {
        return;
    };
    // SAFETY: `offset` is within the mmap segment (bounds checked above).
    let src = unsafe { seg.as_slice().as_ptr().add(offset) };
    // SAFETY: `src` points into the mmap segment with at least `len` bytes
    // remaining, and `out` is a valid DPI-provided buffer of at least `len` bytes.
    unsafe { std::ptr::copy_nonoverlapping(src, out, len) }
}

pub fn axi_write_impl(ctx: &DpiContext, port: &str, addr: u64, width: u32, data: *const u8) {
    let len = width as usize;
    let Some((seg, offset)) = resolve_span(ctx, port, addr, len, "axi_write") else {
        return;
    };
    // SAFETY: bounds checked above; MmapSegment::write_at writes through the
    // raw mmap pointer without creating a &mut reference from &self.
    unsafe { seg.write_at(offset, data, len) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn make_ctx_with_buf(name: &str, data: &[u8]) -> DpiContext {
        let mut seg = MmapSegment::create(name, data.len()).expect("create");
        seg.as_mut_slice().copy_from_slice(data);
        let base = 0x1000_0000u64;
        DpiContext {
            buffers: HashMap::from([(name.to_owned(), (seg, base))]),
            streams: HashMap::new(),
        }
    }

    #[test]
    fn read_at_base() {
        let ctx = make_ctx_with_buf("axi_test", b"hello");
        let mut out = [0u8; 5];
        axi_read_impl(&ctx, "axi_test", 0x1000_0000, 5, out.as_mut_ptr());
        assert_eq!(&out, b"hello");
    }

    #[test]
    fn write_then_read() {
        let ctx = make_ctx_with_buf("axi_wr", &[0u8; 4]);
        axi_write_impl(&ctx, "axi_wr", 0x1000_0000, 4, b"rust".as_ptr());
        let mut out = [0u8; 4];
        axi_read_impl(&ctx, "axi_wr", 0x1000_0000, 4, out.as_mut_ptr());
        assert_eq!(&out, b"rust");
    }

    #[test]
    fn out_of_bounds_transfers_preserve_host_and_device_buffers() {
        let port = "axi_bounds";
        let mut ctx = make_ctx_with_buf(port, b"host");
        ctx.buffers.get_mut(port).expect("buffer").1 = 0;

        for (addr, width) in [(3, 2), (4, 1), (u64::MAX, 2), (u64::MAX - 1, 4)] {
            let mut out = [0xee; 4];
            axi_read_impl(&ctx, port, addr, width, out.as_mut_ptr());
            assert_eq!(out, [0xee; 4], "invalid read must preserve its output");
            axi_write_impl(&ctx, port, addr, width, b"oops".as_ptr());
            assert_eq!(
                ctx.buffers[port].0.as_slice(),
                b"host",
                "invalid write must preserve mapped memory"
            );
        }

        axi_write_impl(&ctx, port, 2, 2, b"ok".as_ptr());
        let mut out = [0u8; 4];
        axi_read_impl(&ctx, port, 0, 4, out.as_mut_ptr());
        assert_eq!(&out, b"hook", "transfers ending at the boundary are valid");
    }

    #[test]
    fn unknown_port_is_noop() {
        let ctx = DpiContext {
            buffers: HashMap::new(),
            streams: HashMap::new(),
        };
        let mut out = [0xffu8; 4];
        axi_read_impl(&ctx, "missing", 0, 4, out.as_mut_ptr());
        assert_eq!(out, [0xff; 4]);
    }
}
