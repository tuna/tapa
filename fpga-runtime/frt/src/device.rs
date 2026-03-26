use crate::error::Result;

pub trait Device: Send {
    fn set_scalar_arg(&mut self, index: u32, value: u64) -> Result<()>;
    fn set_buffer_arg(&mut self, index: u32, ptr: *mut u8, bytes: usize) -> Result<()>;
    fn set_stream_arg(&mut self, index: u32, shm_path: &str) -> Result<()>;
    fn write_to_device(&mut self) -> Result<()>;
    fn read_from_device(&mut self) -> Result<()>;
    fn exec(&mut self) -> Result<()>;
    fn finish(&mut self) -> Result<()>;
    fn load_ns(&self) -> u64;
    fn compute_ns(&self) -> u64;
    fn store_ns(&self) -> u64;
}
