use super::metadata::{extract_embedded_xml, parse_embedded_xml, XrtArgKind, XrtMetadata};
use crate::device::Device;
use crate::error::{FrtError, Result};
use opencl3::command_queue::{CommandQueue, CL_QUEUE_OUT_OF_ORDER_EXEC_MODE_ENABLE, CL_QUEUE_PROFILING_ENABLE};
use opencl3::context::Context;
use opencl3::device::{Device as OclDevice, CL_DEVICE_TYPE_ACCELERATOR, CL_DEVICE_TYPE_ALL};
use opencl3::event::Event;
use opencl3::kernel::{ExecuteKernel, Kernel};
use opencl3::memory::{Buffer, CL_MEM_READ_WRITE, CL_MEM_USE_HOST_PTR};
use opencl3::platform::get_platforms;
use opencl3::program::Program;
use opencl3::types::{cl_event, CL_BLOCKING};
use std::collections::HashMap;
use std::ffi::c_void;
use std::path::Path;

struct BufferBinding {
    ptr: *mut u8,
    bytes: usize,
    buffer: Buffer<u8>,
}

pub struct XrtDevice {
    _meta: XrtMetadata,
    context: Context,
    queue: CommandQueue,
    kernel: Kernel,
    scalars: HashMap<u32, u64>,
    buffers: HashMap<u32, BufferBinding>,
    load_events: Vec<Event>,
    compute_events: Vec<Event>,
    store_events: Vec<Event>,
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

        let device_id = select_device()?;
        let ocl_device = OclDevice::new(device_id);
        let context = ocl_result(Context::from_device(&ocl_device), "create OpenCL context")?;
        let queue = ocl_result(
            CommandQueue::create_default_with_properties(
                &context,
                CL_QUEUE_OUT_OF_ORDER_EXEC_MODE_ENABLE | CL_QUEUE_PROFILING_ENABLE,
                0,
            ),
            "create OpenCL command queue",
        )?;

        let mut program = unsafe {
            ocl_result(
                Program::create_from_binary(&context, &[device_id], &[&bytes]),
                "create OpenCL program from xclbin",
            )?
        };
        ocl_result(program.build(&[device_id], ""), "build OpenCL program")?;

        let kernel = ocl_result(
            Kernel::create(&program, &meta.top_name),
            "create OpenCL kernel",
        )?;

        Ok(Self {
            _meta: meta,
            context,
            queue,
            kernel,
            scalars: HashMap::new(),
            buffers: HashMap::new(),
            load_events: Vec::new(),
            compute_events: Vec::new(),
            store_events: Vec::new(),
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
        if ptr.is_null() && bytes != 0 {
            return Err(FrtError::MetadataParse(format!(
                "null host pointer for non-empty buffer arg {index}"
            )));
        }
        let buffer = unsafe {
            ocl_result(
                Buffer::<u8>::create(
                    &self.context,
                    CL_MEM_READ_WRITE | CL_MEM_USE_HOST_PTR,
                    bytes,
                    ptr as *mut c_void,
                ),
                "create OpenCL buffer",
            )?
        };
        self.buffers.insert(index, BufferBinding { ptr, bytes, buffer });
        Ok(())
    }

    fn set_stream_arg(&mut self, index: u32, _shm_path: &str) -> Result<()> {
        if self
            ._meta
            .args
            .iter()
            .any(|a| a.id == index && matches!(a.kind, XrtArgKind::Stream { .. }))
        {
            return Err(FrtError::MetadataParse(
                "XRT/OpenCL stream arguments are not supported in this runtime path".into(),
            ));
        }
        Err(FrtError::MetadataParse(format!(
            "no stream arg at index {index}"
        )))
    }

    fn write_to_device(&mut self) -> Result<()> {
        self.load_events.clear();
        for binding in self.buffers.values_mut() {
            let host_slice = unsafe { std::slice::from_raw_parts(binding.ptr, binding.bytes) };
            let event = unsafe {
                ocl_result(
                    self.queue
                        .enqueue_write_buffer(&mut binding.buffer, CL_BLOCKING, 0, host_slice, &[]),
                    "enqueue OpenCL write buffer",
                )?
            };
            self.load_events.push(event);
        }
        self.load_ns = elapsed_ns(&self.load_events);
        Ok(())
    }

    fn read_from_device(&mut self) -> Result<()> {
        self.store_events.clear();
        let waits: Vec<cl_event> = self.compute_events.iter().map(Event::get).collect();
        for binding in self.buffers.values() {
            let host_slice = unsafe { std::slice::from_raw_parts_mut(binding.ptr, binding.bytes) };
            let event = unsafe {
                ocl_result(
                    self.queue
                        .enqueue_read_buffer(&binding.buffer, CL_BLOCKING, 0, host_slice, &waits),
                    "enqueue OpenCL read buffer",
                )?
            };
            self.store_events.push(event);
        }
        self.store_ns = elapsed_ns(&self.store_events);
        Ok(())
    }

    fn exec(&mut self) -> Result<()> {
        self.compute_events.clear();

        let mut args = self._meta.args.clone();
        args.sort_by_key(|a| a.id);

        let mut exec = ExecuteKernel::new(&self.kernel);
        for arg in &args {
            match arg.kind {
                XrtArgKind::Scalar { width } => {
                    let v = self.scalars.get(&arg.id).copied().unwrap_or(0);
                    unsafe {
                        if width <= 32 {
                            exec.set_arg(&(v as u32));
                        } else {
                            exec.set_arg(&v);
                        }
                    };
                }
                XrtArgKind::Mmap { .. } => {
                    let binding = self.buffers.get(&arg.id).ok_or_else(|| {
                        FrtError::MetadataParse(format!("missing mmap arg binding for id {}", arg.id))
                    })?;
                    unsafe {
                        exec.set_arg(&binding.buffer);
                    };
                }
                XrtArgKind::Stream { .. } => {
                    return Err(FrtError::MetadataParse(
                        "XRT/OpenCL stream arguments are not supported in this runtime path".into(),
                    ));
                }
            }
        }

        for evt in &self.load_events {
            exec.set_wait_event(evt);
        }
        let evt = unsafe {
            ocl_result(
                exec.set_global_work_size(1).enqueue_nd_range(&self.queue),
                "enqueue OpenCL kernel",
            )?
        };
        self.compute_events.push(evt);
        self.compute_ns = elapsed_ns(&self.compute_events);
        Ok(())
    }

    fn finish(&mut self) -> Result<()> {
        ocl_result(self.queue.finish(), "finish OpenCL queue")?;
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

fn select_device() -> Result<opencl3::types::cl_device_id> {
    let platforms = ocl_result(get_platforms(), "enumerate OpenCL platforms")?;
    for p in &platforms {
        let vendor = ocl_result(p.vendor(), "query OpenCL platform vendor")?;
        if !vendor.to_ascii_lowercase().contains("xilinx") {
            continue;
        }
        let devices = ocl_result(
            p.get_devices(CL_DEVICE_TYPE_ACCELERATOR),
            "enumerate Xilinx OpenCL accelerator devices",
        )?;
        if let Some(id) = devices.first().copied() {
            return Ok(id);
        }
    }
    for p in &platforms {
        let devices = ocl_result(p.get_devices(CL_DEVICE_TYPE_ALL), "enumerate OpenCL devices")?;
        if let Some(id) = devices.first().copied() {
            return Ok(id);
        }
    }
    Err(FrtError::MetadataParse(
        "no OpenCL device available for XRT runtime".into(),
    ))
}

fn ocl_result<T>(res: opencl3::Result<T>, ctx: &str) -> Result<T> {
    res.map_err(|e| FrtError::OpenCl {
        code: e.0,
        msg: format!("{ctx}: {e}"),
    })
}

fn elapsed_ns(events: &[Event]) -> u64 {
    if events.is_empty() {
        return 0;
    }
    let mut start = u64::MAX;
    let mut end = 0u64;
    for e in events {
        if let (Ok(s), Ok(t)) = (e.profiling_command_start(), e.profiling_command_end()) {
            start = start.min(s as u64);
            end = end.max(t as u64);
        }
    }
    if start == u64::MAX || end < start {
        0
    } else {
        end - start
    }
}
