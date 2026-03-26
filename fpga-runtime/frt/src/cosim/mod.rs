use crate::device::Device;
use crate::error::{FrtError, Result};
use crate::instance::Simulator;
use frt_cosim::context::CosimContext;
use frt_cosim::metadata::KernelSpec;
use frt_cosim::runner::verilator::VerilatorRunner;
use frt_cosim::runner::xsim::XsimRunner;
use frt_cosim::runner::SimRunner;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub struct CosimDevice {
    spec: KernelSpec,
    ctx: CosimContext,
    runner: Box<dyn SimRunner>,
    tb_dir: tempfile::TempDir,
    scalars: HashMap<u32, u64>,
    pending_reads: HashMap<u32, (*mut u8, usize)>,
    load_ns: u64,
    compute_ns: u64,
    store_ns: u64,
}

unsafe impl Send for CosimDevice {}

impl CosimDevice {
    pub fn open(path: &Path, sim: &Simulator) -> Result<Self> {
        let spec = frt_cosim::metadata::load_spec(path)?;
        let ctx = CosimContext::new(&spec)?;
        let tb_dir = tempfile::tempdir()?;

        let runner: Box<dyn SimRunner> = match sim {
            Simulator::Verilator => {
                let dpi = dpi_lib_path("verilator")?;
                Box::new(VerilatorRunner::find(dpi)?)
            }
            Simulator::Xsim { legacy } => {
                let dpi = dpi_lib_path("xsim")?;
                Box::new(XsimRunner::find(dpi, *legacy, false)?)
            }
        };

        runner.build(&spec, tb_dir.path())?;
        Ok(Self {
            spec,
            ctx,
            runner,
            tb_dir,
            scalars: HashMap::new(),
            pending_reads: HashMap::new(),
            load_ns: 0,
            compute_ns: 0,
            store_ns: 0,
        })
    }
}

fn dpi_lib_path(variant: &str) -> Result<PathBuf> {
    let exe = std::env::current_exe()?;
    let dir = exe.parent().unwrap_or(Path::new("."));
    for candidate in [
        format!("libfrt_dpi_{variant}.so"),
        format!("libfrt_dpi_{variant}.dylib"),
    ] {
        let p = dir.join(&candidate);
        if p.exists() {
            return Ok(p);
        }
    }
    Err(FrtError::MetadataParse(format!(
        "libfrt_dpi_{variant} shared library not found next to executable"
    )))
}

impl Device for CosimDevice {
    fn set_scalar_arg(&mut self, index: u32, value: u64) -> Result<()> {
        self.scalars.insert(index, value);
        Ok(())
    }

    fn set_buffer_arg(&mut self, index: u32, ptr: *mut u8, bytes: usize) -> Result<()> {
        let name = self
            .spec
            .args
            .iter()
            .find(|a| a.id == index)
            .map(|a| a.name.clone())
            .ok_or_else(|| FrtError::MetadataParse(format!("no arg at index {index}")))?;
        if let Some(seg) = self.ctx.buffers.get_mut(&name) {
            let len = bytes.min(seg.len());
            unsafe {
                seg.as_mut_slice()[..len].copy_from_slice(std::slice::from_raw_parts(ptr, len));
            }
        }
        self.pending_reads.insert(index, (ptr, bytes));
        Ok(())
    }

    fn set_stream_arg(&mut self, _index: u32, _shm_path: &str) -> Result<()> {
        Ok(())
    }

    fn write_to_device(&mut self) -> Result<()> {
        Ok(())
    }

    fn read_from_device(&mut self) -> Result<()> {
        for (index, (ptr, bytes)) in &self.pending_reads {
            let name = self
                .spec
                .args
                .iter()
                .find(|a| a.id == *index)
                .map(|a| a.name.clone())
                .ok_or_else(|| FrtError::MetadataParse(format!("no arg at index {index}")))?;
            if let Some(seg) = self.ctx.buffers.get(&name) {
                let len = (*bytes).min(seg.len());
                unsafe {
                    std::slice::from_raw_parts_mut(*ptr, len).copy_from_slice(&seg.as_slice()[..len]);
                }
            }
        }
        Ok(())
    }

    fn exec(&mut self) -> Result<()> {
        let result = self.runner.run(&self.ctx, self.tb_dir.path())?;
        self.compute_ns = result.wall_ns;
        Ok(())
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
