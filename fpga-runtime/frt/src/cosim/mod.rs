use crate::device::{BufferAccess, Device, RuntimeArgCategory, RuntimeArgInfo};
use crate::error::{FrtError, Result};
use crate::instance::Simulator;
use frt_cosim::context::CosimContext;
use frt_cosim::metadata::KernelSpec;
use frt_cosim::runner::verilator::VerilatorRunner;
use frt_cosim::runner::xsim::XsimRunner;
use frt_cosim::runner::SimRunner;
use std::collections::HashMap;
use std::process::Command;
use std::path::{Path, PathBuf};
use std::process::Child;
use std::time::Instant;

enum TbDir {
    Temp(tempfile::TempDir),
    Fixed(PathBuf),
}

impl TbDir {
    fn path(&self) -> &Path {
        match self {
            TbDir::Temp(d) => d.path(),
            TbDir::Fixed(p) => p.as_path(),
        }
    }
}

struct RuntimeOptions {
    start_gui: bool,
    save_waveform: bool,
    setup_only: bool,
    resume_from_post_sim: bool,
    work_dir: Option<PathBuf>,
    work_dir_parallel: bool,
    part_num_override: Option<String>,
}

struct BufferBinding {
    ptr: *mut u8,
    bytes: usize,
    access: BufferAccess,
}

struct RunningSimulation {
    child: Child,
    started_at: Instant,
}

enum SimulationState {
    Idle,
    Running(RunningSimulation),
    Finished,
}

pub struct CosimDevice {
    spec: KernelSpec,
    ctx: CosimContext,
    runner: Box<dyn SimRunner>,
    tb_dir: TbDir,
    setup_only: bool,
    resume_from_post_sim: bool,
    scalars: HashMap<u32, Vec<u8>>,
    pending_buffers: HashMap<u32, BufferBinding>,
    simulation_state: SimulationState,
    readback_scheduled: bool,
    pending_sim_error: Option<FrtError>,
    load_ns: u64,
    compute_ns: u64,
    store_ns: u64,
}

unsafe impl Send for CosimDevice {}

impl CosimDevice {
    pub fn open(path: &Path, sim: &Simulator) -> Result<Self> {
        let spec = frt_cosim::metadata::load_spec(path)?;
        let ctx = CosimContext::new(&spec)?;
        let opts = runtime_options();
        let tb_dir = make_tb_dir(opts.work_dir.as_deref(), opts.work_dir_parallel)?;

        let runner: Box<dyn SimRunner> = match sim {
            Simulator::Verilator => {
                let dpi = dpi_lib_path("verilator")?;
                Box::new(VerilatorRunner::find(dpi)?)
            }
            Simulator::Xsim { legacy } => {
                let dpi = dpi_lib_path("xsim")?;
                Box::new(XsimRunner::find(
                    dpi,
                    *legacy || env_bool("FRT_XSIM_LEGACY"),
                    opts.save_waveform,
                    opts.start_gui,
                    opts.part_num_override.clone(),
                )?)
            }
        };

        Ok(Self {
            spec,
            ctx,
            runner,
            tb_dir,
            setup_only: opts.setup_only,
            resume_from_post_sim: opts.resume_from_post_sim,
            scalars: HashMap::new(),
            pending_buffers: HashMap::new(),
            simulation_state: SimulationState::Idle,
            readback_scheduled: false,
            pending_sim_error: None,
            load_ns: 0,
            compute_ns: 0,
            store_ns: 0,
        })
    }

    fn is_simulation_running(&self) -> bool {
        matches!(self.simulation_state, SimulationState::Running(_))
    }

    fn spawn_noop_process() -> Result<Child> {
        let mut cmd = Command::new("/bin/sh");
        cmd.args(["-c", ":"]);
        #[cfg(unix)]
        unsafe {
            use std::os::unix::process::CommandExt;
            cmd.pre_exec(|| {
                if libc::setpgid(0, 0) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        Ok(cmd.spawn()?)
    }

    fn copy_back_to_host(&mut self) -> Result<()> {
        let started = Instant::now();
        for (index, binding) in &self.pending_buffers {
            if !binding.access.stores_to_host() {
                continue;
            }
            if binding.ptr.is_null() && binding.bytes != 0 {
                return Err(FrtError::MetadataParse(format!(
                    "null pointer for buffer arg {index}"
                )));
            }
            let name = self
                .spec
                .args
                .iter()
                .find(|a| a.id == *index)
                .map(|a| a.name.clone())
                .ok_or_else(|| FrtError::MetadataParse(format!("no arg at index {index}")))?;
            if let Some(seg) = self.ctx.buffers.get(&name) {
                let len = binding.bytes.min(seg.len());
                if len > 0 {
                    unsafe {
                        std::slice::from_raw_parts_mut(binding.ptr, len)
                            .copy_from_slice(&seg.as_slice()[..len]);
                    }
                }
            }
        }
        self.store_ns = started.elapsed().as_nanos() as u64;
        if self.store_ns == 0 {
            self.store_ns = 1;
        }
        Ok(())
    }

    fn poll_simulation(&mut self) -> Result<bool> {
        match &mut self.simulation_state {
            SimulationState::Idle => Ok(false),
            SimulationState::Finished => Ok(true),
            SimulationState::Running(run) => {
                let maybe_status = run.child.try_wait()?;
                if let Some(status) = maybe_status {
                    self.compute_ns = run.started_at.elapsed().as_nanos() as u64;
                    if !status.success() {
                        self.pending_sim_error = Some(FrtError::SimFailed(status));
                    }
                    self.simulation_state = SimulationState::Finished;
                    Ok(true)
                } else {
                    Ok(false)
                }
            }
        }
    }

    fn wait_simulation(&mut self) -> Result<()> {
        if let SimulationState::Running(run) = &mut self.simulation_state {
            let status = run.child.wait()?;
            self.compute_ns = run.started_at.elapsed().as_nanos() as u64;
            if !status.success() {
                self.pending_sim_error = Some(FrtError::SimFailed(status));
            }
            self.simulation_state = SimulationState::Finished;
        }
        Ok(())
    }

}

fn env_bool(name: &str) -> bool {
    match std::env::var(name) {
        Ok(v) => matches!(v.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"),
        Err(_) => false,
    }
}

fn env_non_empty(name: &str) -> Option<String> {
    std::env::var(name).ok().and_then(|v| {
        let trimmed = v.trim().to_owned();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    })
}

fn runtime_options() -> RuntimeOptions {
    RuntimeOptions {
        start_gui: env_bool("FRT_XOSIM_START_GUI"),
        save_waveform: env_bool("FRT_XOSIM_SAVE_WAVEFORM"),
        setup_only: env_bool("FRT_XOSIM_SETUP_ONLY"),
        resume_from_post_sim: env_bool("FRT_XOSIM_RESUME_FROM_POST_SIM"),
        work_dir: env_non_empty("FRT_XOSIM_WORK_DIR").map(PathBuf::from),
        work_dir_parallel: env_bool("FRT_XOSIM_WORK_DIR_PARALLEL"),
        part_num_override: env_non_empty("FRT_XOSIM_PART_NUM"),
    }
}

fn make_tb_dir(work_dir: Option<&Path>, parallel: bool) -> Result<TbDir> {
    if let Some(base) = work_dir {
        std::fs::create_dir_all(base)?;
        if parallel {
            let suffix = format!(
                "{}_{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            );
            let dir = base.join(suffix);
            std::fs::create_dir_all(&dir)?;
            return Ok(TbDir::Fixed(dir));
        }
        return Ok(TbDir::Fixed(base.to_path_buf()));
    }
    Ok(TbDir::Temp(tempfile::tempdir()?))
}

fn dpi_lib_path(variant: &str) -> Result<PathBuf> {
    let exe = std::env::current_exe()?;
    let dir = exe.parent().unwrap_or(Path::new("."));
    let mut search_dirs = vec![dir.to_path_buf()];
    if let Some(p) = dir.parent() {
        search_dirs.push(p.to_path_buf());
    }
    if let Some(p) = dir.parent().and_then(|x| x.parent()) {
        search_dirs.push(p.to_path_buf());
    }
    for candidate in [
        format!("libfrt_dpi_{variant}.so"),
        format!("libfrt_dpi_{variant}.dylib"),
    ] {
        for base in &search_dirs {
            let p = base.join(&candidate);
            if p.exists() {
                return Ok(p);
            }
        }
    }
    Err(FrtError::MetadataParse(format!(
        "libfrt_dpi_{variant} shared library not found next to executable"
    )))
}

impl Device for CosimDevice {
    fn set_scalar_arg(&mut self, index: u32, value: &[u8]) -> Result<()> {
        self.scalars.insert(index, value.to_vec());
        Ok(())
    }

    fn set_buffer_arg(
        &mut self,
        index: u32,
        ptr: *mut u8,
        bytes: usize,
        access: BufferAccess,
    ) -> Result<()> {
        let name = self
            .spec
            .args
            .iter()
            .find(|a| a.id == index)
            .map(|a| a.name.clone())
            .ok_or_else(|| FrtError::MetadataParse(format!("no arg at index {index}")))?;
        if !self.ctx.buffers.contains_key(&name) {
            return Err(FrtError::MetadataParse(format!(
                "arg '{name}' is not an mmap buffer"
            )));
        }
        self.pending_buffers
            .insert(index, BufferBinding { ptr, bytes, access });
        Ok(())
    }

    fn set_stream_arg(&mut self, _index: u32, _shm_path: &str) -> Result<()> {
        let index = _index;
        let shm_path = _shm_path;
        if shm_path.is_empty() {
            return Ok(());
        }
        let name = self
            .spec
            .args
            .iter()
            .find(|a| a.id == index)
            .and_then(|a| match a.kind {
                frt_cosim::metadata::ArgKind::Stream { .. } => Some(a.name.clone()),
                _ => None,
            })
            .ok_or_else(|| FrtError::MetadataParse(format!("no stream arg at index {index}")))?;
        self.ctx.bind_stream_path(&name, shm_path)?;
        Ok(())
    }

    fn suspend_buffer(&mut self, index: u32) -> usize {
        if self.pending_buffers.remove(&index).is_some() {
            1
        } else {
            0
        }
    }

    fn write_to_device(&mut self) -> Result<()> {
        let started = Instant::now();
        for (index, binding) in &self.pending_buffers {
            if !binding.access.loads_from_host() {
                continue;
            }
            if binding.ptr.is_null() && binding.bytes != 0 {
                return Err(FrtError::MetadataParse(format!(
                    "null pointer for buffer arg {index}"
                )));
            }
            let name = self
                .spec
                .args
                .iter()
                .find(|a| a.id == *index)
                .map(|a| a.name.clone())
                .ok_or_else(|| FrtError::MetadataParse(format!("no arg at index {index}")))?;
            if let Some(seg) = self.ctx.buffers.get_mut(&name) {
                let len = binding.bytes.min(seg.len());
                if len > 0 {
                    unsafe {
                        seg.as_mut_slice()[..len]
                            .copy_from_slice(std::slice::from_raw_parts(binding.ptr, len));
                    }
                }
            }
        }
        self.load_ns = started.elapsed().as_nanos() as u64;
        Ok(())
    }

    fn read_from_device(&mut self) -> Result<()> {
        self.readback_scheduled = true;
        if self.is_simulation_running() {
            return Ok(());
        }
        self.copy_back_to_host()
    }

    fn exec(&mut self) -> Result<()> {
        self.runner
            .build(&self.spec, &self.ctx, &self.scalars, self.tb_dir.path())?;
        if self.resume_from_post_sim {
            let child = Self::spawn_noop_process()?;
            self.simulation_state = SimulationState::Running(RunningSimulation {
                child,
                started_at: Instant::now(),
            });
            self.compute_ns = 0;
            return Ok(());
        }
        if self.setup_only {
            self.compute_ns = 0;
            self.simulation_state = SimulationState::Finished;
            return Ok(());
        }
        let child = self.runner.spawn(&self.ctx, self.tb_dir.path())?;
        self.simulation_state = SimulationState::Running(RunningSimulation {
            child,
            started_at: Instant::now(),
        });
        Ok(())
    }

    fn finish(&mut self) -> Result<()> {
        self.wait_simulation()?;
        if matches!(self.simulation_state, SimulationState::Idle) {
            self.simulation_state = SimulationState::Finished;
        }
        if let Some(err) = self.pending_sim_error.take() {
            return Err(err);
        }
        if self.readback_scheduled {
            self.copy_back_to_host()?;
            self.readback_scheduled = false;
        }
        Ok(())
    }

    fn kill(&mut self) -> Result<()> {
        match &mut self.simulation_state {
            SimulationState::Running(run) => {
                #[cfg(unix)]
                unsafe {
                    let pgid = run.child.id() as i32;
                    if libc::killpg(pgid, libc::SIGINT) != 0 {
                        let err = std::io::Error::last_os_error();
                        if err.raw_os_error() != Some(libc::ESRCH) {
                            tracing::warn!(
                                "failed to send SIGINT to simulator process group: {err}"
                            );
                        }
                    }
                }
                match run.child.kill() {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::InvalidInput => {}
                    Err(e) => return Err(e.into()),
                }
                let _ = run.child.wait();
                self.compute_ns = run.started_at.elapsed().as_nanos() as u64;
                self.simulation_state = SimulationState::Finished;
            }
            SimulationState::Idle => {
                self.simulation_state = SimulationState::Finished;
            }
            SimulationState::Finished => {}
        }
        Ok(())
    }

    fn is_finished(&mut self) -> Result<bool> {
        self.poll_simulation()
    }

    fn args_info(&self) -> Vec<RuntimeArgInfo> {
        let mut args = Vec::with_capacity(self.spec.args.len());
        for arg in &self.spec.args {
            let (type_name, category) = match &arg.kind {
                frt_cosim::metadata::ArgKind::Scalar { .. } => {
                    ("scalar".to_owned(), RuntimeArgCategory::Scalar)
                }
                frt_cosim::metadata::ArgKind::Mmap { .. } => {
                    ("mmap".to_owned(), RuntimeArgCategory::Mmap)
                }
                frt_cosim::metadata::ArgKind::Stream { protocol, .. } => (
                    match protocol {
                        frt_cosim::metadata::StreamProtocol::Axis => "axis",
                        frt_cosim::metadata::StreamProtocol::ApFifo => "ap_fifo",
                    }
                    .to_owned(),
                    RuntimeArgCategory::Stream,
                ),
            };
            args.push(RuntimeArgInfo {
                index: arg.id,
                name: arg.name.clone(),
                type_name,
                category,
            });
        }
        args.sort_by_key(|a| a.index);
        args
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

#[cfg(test)]
mod tests {
    use super::*;
    use frt_cosim::metadata::{ArgKind, ArgSpec, Mode};
    use std::collections::HashMap;
    use std::process::{Child, Command};
    use std::time::Duration;

    struct SleepRunner {
        sleep_seconds: f32,
    }

    impl SimRunner for SleepRunner {
        fn build(
            &self,
            _spec: &KernelSpec,
            _ctx: &CosimContext,
            _scalar_values: &HashMap<u32, Vec<u8>>,
            _tb_dir: &Path,
        ) -> frt_cosim::error::Result<()> {
            Ok(())
        }

        fn spawn(&self, _ctx: &CosimContext, _tb_dir: &Path) -> frt_cosim::error::Result<Child> {
            let child = Command::new("/bin/sh")
                .args(["-c", &format!("sleep {}", self.sleep_seconds)])
                .spawn()?;
            Ok(child)
        }
    }

    fn make_test_device(sleep_seconds: f32) -> CosimDevice {
        let spec = KernelSpec {
            top_name: "top".to_owned(),
            mode: Mode::Hls,
            args: vec![],
            part_num: None,
            verilog_files: vec![],
            tcl_files: vec![],
            xci_files: vec![],
            scalar_register_map: HashMap::new(),
        };
        let ctx = CosimContext::new(&spec).expect("create cosim context");
        CosimDevice {
            spec,
            ctx,
            runner: Box::new(SleepRunner { sleep_seconds }),
            tb_dir: TbDir::Temp(tempfile::tempdir().expect("create temp dir")),
            setup_only: false,
            resume_from_post_sim: false,
            scalars: HashMap::new(),
            pending_buffers: HashMap::new(),
            simulation_state: SimulationState::Idle,
            readback_scheduled: false,
            pending_sim_error: None,
            load_ns: 0,
            compute_ns: 0,
            store_ns: 0,
        }
    }

    fn make_test_device_with_mmap(resume_from_post_sim: bool) -> CosimDevice {
        let mut dev = make_test_device(0.01);
        dev.spec.args = vec![ArgSpec {
            name: "buf0".to_owned(),
            id: 0,
            kind: ArgKind::Mmap {
                data_width: 32,
                addr_width: 64,
            },
        }];
        dev.ctx = CosimContext::new(&dev.spec).expect("create cosim context");
        dev.resume_from_post_sim = resume_from_post_sim;
        dev
    }

    #[test]
    fn is_finished_is_false_before_exec() {
        let mut dev = make_test_device(0.02);
        assert!(!dev.is_finished().expect("poll simulation state"));
    }

    #[test]
    fn is_finished_transitions_after_exec() {
        let mut dev = make_test_device(0.05);
        dev.exec().expect("spawn simulation");
        assert!(!dev.is_finished().expect("simulation should be running"));

        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        let mut done = false;
        while std::time::Instant::now() < deadline {
            if dev.is_finished().expect("poll simulation") {
                done = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(done, "simulation did not finish before timeout");
        dev.finish().expect("finish simulation");
    }

    #[test]
    fn kill_transitions_to_finished() {
        let mut dev = make_test_device(5.0);
        dev.exec().expect("spawn simulation");
        assert!(!dev.is_finished().expect("simulation should be running"));
        dev.kill().expect("kill simulation");
        assert!(dev
            .is_finished()
            .expect("killed simulation should be finished"));
    }

    #[test]
    fn kill_before_exec_marks_finished() {
        let mut dev = make_test_device(0.01);
        assert!(!dev.is_finished().expect("idle simulation should not be finished"));
        dev.kill().expect("kill idle simulation");
        assert!(dev.is_finished().expect("idle kill should mark finished"));
    }

    #[test]
    fn finish_before_exec_marks_finished() {
        let mut dev = make_test_device(0.01);
        assert!(!dev.is_finished().expect("idle simulation should not be finished"));
        dev.finish().expect("finish idle simulation");
        assert!(dev.is_finished().expect("idle finish should mark finished"));
    }

    #[test]
    fn resume_from_post_sim_defers_copyback_until_finish() {
        let mut dev = make_test_device_with_mmap(true);
        let mut host_word = 10u32;
        dev.set_buffer_arg(
            0,
            (&mut host_word as *mut u32).cast::<u8>(),
            std::mem::size_of_val(&host_word),
            BufferAccess::ReadWrite,
        )
        .expect("set buffer");
        dev.ctx
            .buffers
            .get_mut("buf0")
            .expect("mmap buffer")
            .as_mut_slice()[..4]
            .copy_from_slice(&42u32.to_le_bytes());
        dev.exec().expect("resume-from-post-sim exec");
        assert_eq!(dev.compute_ns(), 0);
        dev.read_from_device().expect("schedule readback");
        assert_eq!(host_word, 10);
        assert_eq!(dev.store_ns(), 0);
        dev.finish().expect("finish resume-from-post-sim");
        assert_eq!(host_word, 42);
        assert!(dev.store_ns() > 0);
    }

    #[test]
    fn suspend_buffer_removes_binding() {
        let mut dev = make_test_device_with_mmap(false);
        let mut data = [1u8; 16];
        dev.set_buffer_arg(0, data.as_mut_ptr(), data.len(), BufferAccess::ReadWrite)
            .expect("set buffer");
        assert_eq!(dev.pending_buffers.len(), 1);
        assert_eq!(dev.suspend_buffer(0), 1);
        assert!(dev.pending_buffers.is_empty());
        assert_eq!(dev.suspend_buffer(0), 0);
    }
}
