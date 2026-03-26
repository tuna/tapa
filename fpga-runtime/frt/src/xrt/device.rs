use super::metadata::{extract_embedded_xml, parse_embedded_xml, XrtMetadata};
use crate::device::Device;
use crate::error::{FrtError, Result};
use std::collections::HashMap;
use std::path::Path;

pub struct XrtDevice {
    _meta: XrtMetadata,
    scalars: HashMap<u32, u64>,
    buffers: HashMap<u32, (*mut u8, usize)>,
    load_ns: u64,
    compute_ns: u64,
    store_ns: u64,
}

unsafe impl Send for XrtDevice {}

impl XrtDevice {
    pub fn open(xclbin_path: &Path) -> Result<Self> {
        let bytes = std::fs::read(xclbin_path)?;
        let xml = extract_embedded_xml(&bytes)?;
        let meta = parse_embedded_xml(&xml)?;
        Ok(Self {
            _meta: meta,
            scalars: HashMap::new(),
            buffers: HashMap::new(),
            load_ns: 0,
            compute_ns: 0,
            store_ns: 0,
        })
    }
}

impl Device for XrtDevice {
    fn set_scalar_arg(&mut self, index: u32, value: u64) -> Result<()> {
        self.scalars.insert(index, value);
        Ok(())
    }

    fn set_buffer_arg(&mut self, index: u32, ptr: *mut u8, bytes: usize) -> Result<()> {
        self.buffers.insert(index, (ptr, bytes));
        Ok(())
    }

    fn set_stream_arg(&mut self, _index: u32, _shm_path: &str) -> Result<()> {
        Ok(())
    }

    fn write_to_device(&mut self) -> Result<()> {
        Ok(())
    }

    fn read_from_device(&mut self) -> Result<()> {
        Ok(())
    }

    fn exec(&mut self) -> Result<()> {
        Err(FrtError::MetadataParse(
            "XrtDevice execution path is not wired in this migration step".into(),
        ))
    }

    fn finish(&mut self) -> Result<()> {
        Ok(())
    }

    fn load_ns(&self) -> u64 {
        self.load_ns
    }

    fn compute_ns(&self) -> u64 {
        self.compute_ns
    }

    fn store_ns(&self) -> u64 {
        self.store_ns
    }
}
