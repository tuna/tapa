use crate::context::DpiContext;

pub fn axi_read_impl(ctx: &DpiContext, port: &str, addr: u64, width: u32, out: *mut u8) {
    let Some((seg, base)) = ctx.buffers.get(port) else {
        eprintln!("frt-dpi: axi_read: unknown port '{port}'");
        return;
    };

    let Some(offset) = addr.checked_sub(*base).map(|v| v as usize) else {
        eprintln!("frt-dpi: axi_read: addr below base on '{port}'");
        return;
    };
    let len = width as usize;
    if offset + len > seg.len() {
        eprintln!("frt-dpi: axi_read: out of bounds on '{port}' offset={offset} len={len}");
        return;
    }
    unsafe { std::ptr::copy_nonoverlapping(seg.as_slice().as_ptr().add(offset), out, len) }
}

pub fn axi_write_impl(ctx: &DpiContext, port: &str, addr: u64, width: u32, data: *const u8) {
    let Some((seg, base)) = ctx.buffers.get(port) else {
        eprintln!("frt-dpi: axi_write: unknown port '{port}'");
        return;
    };

    let Some(offset) = addr.checked_sub(*base).map(|v| v as usize) else {
        eprintln!("frt-dpi: axi_write: addr below base on '{port}'");
        return;
    };
    let len = width as usize;
    if offset + len > seg.len() {
        eprintln!("frt-dpi: axi_write: out of bounds on '{port}'");
        return;
    }
    let slice = unsafe { std::slice::from_raw_parts_mut(seg.as_slice().as_ptr() as *mut u8, seg.len()) };
    unsafe { std::ptr::copy_nonoverlapping(data, slice.as_mut_ptr().add(offset), len) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::DpiContext;
    use frt_shm::MmapSegment;
    use std::collections::HashMap;

    fn make_ctx_with_buf(name: &str, data: &[u8]) -> DpiContext {
        let mut seg = MmapSegment::create(name, data.len()).expect("create");
        seg.as_mut_slice().copy_from_slice(data);
        let base = 0x1000_0000u64;
        DpiContext {
            buffers: HashMap::from([(name.to_string(), (seg, base))]),
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
