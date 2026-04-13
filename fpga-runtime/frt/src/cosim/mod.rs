use crate::device::{BufferAccess, Device, RuntimeArgCategory, RuntimeArgInfo};
use crate::error::{FrtError, Result};
use crate::instance::Simulator;
use frt_cosim::context::CosimContext;
use frt_cosim::metadata::KernelSpec;
use frt_cosim::runner::verilator::VerilatorRunner;
use frt_cosim::runner::xsim::XsimRunner;
use frt_cosim::runner::SimRunner;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Child;
use std::process::Command;
use std::time::Instant;

enum TbDir {
    Temp(tempfile::TempDir),
    Fixed(PathBuf),
}

impl TbDir {
    fn path(&self) -> &Path {
        match self {
            Self::Temp(d) => d.path(),
            Self::Fixed(p) => p.as_path(),
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
    load_suspended: bool,
    store_suspended: bool,
}

struct RunningSimulation {
    child: Child,
    started_at: Instant,
    paused: bool,
}

enum SimulationState {
    Idle,
    Running(RunningSimulation),
    Finished,
}

pub struct CosimDevice {
    spec: KernelSpec,
    arg_names: HashMap<u32, String>,
    stream_arg_names: HashMap<u32, String>,
    ctx: CosimContext,
    runner: Box<dyn SimRunner>,
    tb_dir: TbDir,
    _extract_dir: tempfile::TempDir,
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

// SAFETY: CosimDevice is only accessed from a single owner thread.
// The raw `*mut u8` pointers in BufferBinding point to host memory whose
// lifetime is managed by the caller (the C++ compatibility layer) and
// outlives the device.
unsafe impl Send for CosimDevice {}

impl CosimDevice {
    pub fn open(path: &Path, sim: &Simulator) -> Result<Self> {
        let (spec, extract_dir) = frt_cosim::metadata::load_spec(path)?;
        let arg_names = spec
            .args
            .iter()
            .map(|arg| (arg.id, arg.name.clone()))
            .collect();
        let stream_arg_names = spec
            .args
            .iter()
            .filter_map(|arg| match arg.kind {
                frt_cosim::metadata::ArgKind::Stream { .. } => Some((arg.id, arg.name.clone())),
                frt_cosim::metadata::ArgKind::Scalar { .. }
                | frt_cosim::metadata::ArgKind::Mmap { .. } => None,
            })
            .collect();
        let opts = runtime_options();
        let tb_dir = make_tb_dir(opts.work_dir.as_deref(), opts.work_dir_parallel)?;
        let ctx = if opts.resume_from_post_sim {
            let config_path = tb_dir.path().join("dpi_config.json");
            let json = std::fs::read_to_string(&config_path).map_err(|e| {
                FrtError::MetadataParse(format!("failed to read {}: {e}", config_path.display()))
            })?;
            CosimContext::open_from_config(&spec, &json)?
        } else {
            CosimContext::new(&spec)?
        };

        let runner: Box<dyn SimRunner> = match sim {
            Simulator::Verilator => {
                let dpi = dpi_lib_path("verilator")?;
                Box::new(VerilatorRunner::find(dpi)?)
            }
            Simulator::Xsim { legacy } => {
                let dpi = dpi_lib_path("xsim")?;
                Box::new(XsimRunner::find(
                    dpi,
                    *legacy || env_bool(frt_shm::env::FRT_XSIM_LEGACY),
                    opts.save_waveform,
                    opts.start_gui,
                    opts.part_num_override.clone(),
                )?)
            }
        };

        Ok(Self {
            spec,
            arg_names,
            stream_arg_names,
            ctx,
            runner,
            tb_dir,
            _extract_dir: extract_dir,
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

    fn spawn_noop_process() -> Result<Child> {
        let mut cmd = Command::new("/bin/sh");
        cmd.args(["-c", ":"]);
        frt_cosim::runner::configure_sim_command(&mut cmd);
        Ok(cmd.spawn()?)
    }

    fn copy_back_to_host(&mut self) -> Result<()> {
        let started = Instant::now();
        for (index, binding) in &self.pending_buffers {
            if !binding.access.stores_to_host() || binding.store_suspended {
                continue;
            }
            if binding.ptr.is_null() && binding.bytes != 0 {
                return Err(FrtError::MetadataParse(format!(
                    "null pointer for buffer arg {index}"
                )));
            }
            let name = self.arg_name(*index)?.to_owned();
            if let Some(seg) = self.ctx.buffers.get(&name) {
                let len = binding.bytes.min(seg.len());
                if len > 0 {
                    // SAFETY: binding.ptr is non-null (checked above) and
                    // len <= binding.bytes, so the slice is within the
                    // caller-provided host buffer.
                    let dst = unsafe { std::slice::from_raw_parts_mut(binding.ptr, len) };
                    dst.copy_from_slice(&seg.as_slice()[..len]);
                }
            }
        }
        self.store_ns = started.elapsed().as_nanos() as u64;
        // Ensure non-zero to signal "copy-back completed" (callers use
        // store_ns() == 0 to mean "not yet run").
        if self.store_ns == 0 {
            self.store_ns = 1;
        }
        Ok(())
    }

    fn arg_name(&self, index: u32) -> Result<&str> {
        self.arg_names
            .get(&index)
            .map(String::as_str)
            .ok_or_else(|| FrtError::MetadataParse(format!("no arg at index {index}")))
    }

    fn stream_arg_name(&self, index: u32) -> Result<&str> {
        self.stream_arg_names
            .get(&index)
            .map(String::as_str)
            .ok_or_else(|| FrtError::MetadataParse(format!("no stream arg at index {index}")))
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

    fn pause_simulation(&mut self) -> Result<()> {
        if self.poll_simulation()? {
            return Ok(());
        }
        if let SimulationState::Running(run) = &mut self.simulation_state {
            if run.paused {
                return Ok(());
            }
            signal_child_group(&run.child, libc::SIGSTOP)?;
            run.paused = true;
        }
        Ok(())
    }

    fn resume_simulation(&mut self) -> Result<()> {
        if self.poll_simulation()? {
            return Ok(());
        }
        if let SimulationState::Running(run) = &mut self.simulation_state {
            if !run.paused {
                return Ok(());
            }
            signal_child_group(&run.child, libc::SIGCONT)?;
            run.paused = false;
        }
        Ok(())
    }
}

#[cfg(unix)]
fn signal_child_group(child: &Child, signal: libc::c_int) -> Result<()> {
    let pgid = child.id() as i32;
    // SAFETY: killpg sends a signal to a process group; pgid is a valid
    // process group id obtained from the child we spawned.
    if unsafe { libc::killpg(pgid, signal) } != 0 {
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() != Some(libc::ESRCH) {
            return Err(err.into());
        }
    }
    Ok(())
}

#[cfg(not(unix))]
fn signal_child_group(_child: &Child, _signal: libc::c_int) -> Result<()> {
    Ok(())
}

use crate::env_bool;
use frt_shm::env_non_empty;

fn runtime_options() -> RuntimeOptions {
    use frt_shm::env;
    RuntimeOptions {
        start_gui: env_bool(env::FRT_XSIM_START_GUI),
        save_waveform: env_bool(env::FRT_XSIM_SAVE_WAVEFORM),
        setup_only: env_bool(env::FRT_COSIM_SETUP_ONLY),
        resume_from_post_sim: env_bool(env::FRT_COSIM_RESUME_FROM_POST_SIM),
        work_dir: env_non_empty(env::FRT_COSIM_WORK_DIR).map(PathBuf::from),
        work_dir_parallel: env_bool(env::FRT_COSIM_WORK_DIR_PARALLEL),
        part_num_override: env_non_empty(env::FRT_XSIM_PART_NUM),
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
    // Prefer searching relative to libfrt.so itself (covers staging tests
    // where the host binary is compiled into /tmp but libfrt.so lives in
    // the install prefix).
    let self_path = self_lib_path().unwrap_or_else(|| std::env::current_exe().unwrap_or_default());
    dpi_lib_path_from_exe(&self_path, variant)
}

#[cfg(unix)]
fn self_lib_path() -> Option<PathBuf> {
    // Use dladdr to find the path of the shared library containing this function.
    #[allow(
        clippy::fn_to_numeric_cast_any,
        reason = "dladdr requires a function address as *const c_void"
    )]
    let ptr = self_lib_path as *const ();
    // SAFETY: zeroed is valid for Dl_info (it is a plain-old-data C struct).
    let mut info: libc::Dl_info = unsafe { std::mem::zeroed() };
    // SAFETY: dladdr resolves the shared-object path for a given address.
    // `ptr` is a valid function pointer in the current image.
    if unsafe { libc::dladdr(ptr.cast(), &raw mut info) } != 0 && !info.dli_fname.is_null() {
        // SAFETY: dli_fname is non-null (checked above) and points to a
        // NUL-terminated string managed by the dynamic linker.
        let path = unsafe { std::ffi::CStr::from_ptr(info.dli_fname) };
        path.to_str().ok().map(PathBuf::from)
    } else {
        None
    }
}

#[cfg(not(unix))]
fn self_lib_path() -> Option<PathBuf> {
    None
}

fn dpi_lib_path_from_exe(exe: &Path, variant: &str) -> Result<PathBuf> {
    let mut search_dirs = Vec::new();
    if let Some(dir) = exe.parent() {
        search_dirs.push(dir.to_path_buf());
        // Installed layout: bin/ is sibling of lib/
        if let Some(parent) = dir.parent() {
            search_dirs.push(parent.join("lib"));
        }
        for ancestor in dir.ancestors() {
            search_dirs.push(ancestor.to_path_buf());
            search_dirs.push(ancestor.join("fpga-runtime/cargo"));
            search_dirs.push(ancestor.join("cargo"));
        }
    }
    // Also search LD_LIBRARY_PATH (covers staging tests that copy binaries)
    if let Ok(ldpath) = std::env::var("LD_LIBRARY_PATH") {
        for dir in ldpath.split(':') {
            if !dir.is_empty() {
                search_dirs.push(PathBuf::from(dir));
            }
        }
    }
    let candidates = if cfg!(target_os = "macos") {
        [
            format!("libfrt_dpi_{variant}.dylib"),
            format!("libfrt_dpi_{variant}.so"),
        ]
    } else {
        [
            format!("libfrt_dpi_{variant}.so"),
            format!("libfrt_dpi_{variant}.dylib"),
        ]
    };
    for candidate in candidates {
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
        let name = self.arg_name(index)?.to_owned();
        if !self.ctx.buffers.contains_key(&name) {
            return Err(FrtError::MetadataParse(format!(
                "arg '{name}' is not an mmap buffer"
            )));
        }
        self.ctx.resize_buffer(&name, bytes)?;
        self.pending_buffers.insert(
            index,
            BufferBinding {
                ptr,
                bytes,
                access,
                load_suspended: false,
                store_suspended: false,
            },
        );
        Ok(())
    }

    fn set_stream_arg(&mut self, index: u32, shm_path: &str) -> Result<()> {
        if shm_path.is_empty() {
            return Ok(());
        }
        let name = match self.stream_arg_name(index) {
            Ok(n) => n.to_owned(),
            Err(_) if self.resume_from_post_sim => return Ok(()),
            Err(e) => return Err(e),
        };
        // In resume mode the context has no streams; skip binding.
        if self.ctx.streams.contains_key(&name) {
            self.ctx.bind_stream_path(&name, shm_path)?;
        }
        Ok(())
    }

    fn suspend_buffer(&mut self, index: u32) -> usize {
        let Some(binding) = self.pending_buffers.get_mut(&index) else {
            return 0;
        };
        let mut erased = 0;
        if binding.access.loads_from_host() && !binding.load_suspended {
            binding.load_suspended = true;
            erased += 1;
        }
        if binding.access.stores_to_host() && !binding.store_suspended {
            binding.store_suspended = true;
            erased += 1;
        }
        erased
    }

    fn write_to_device(&mut self) -> Result<()> {
        let started = Instant::now();
        for (index, binding) in &self.pending_buffers {
            if !binding.access.loads_from_host() || binding.load_suspended {
                continue;
            }
            if binding.ptr.is_null() && binding.bytes != 0 {
                return Err(FrtError::MetadataParse(format!(
                    "null pointer for buffer arg {index}"
                )));
            }
            let name = self.arg_name(*index)?.to_owned();
            if let Some(seg) = self.ctx.buffers.get_mut(&name) {
                let len = binding.bytes.min(seg.len());
                if len > 0 {
                    // SAFETY: binding.ptr is non-null (checked above) and
                    // len <= binding.bytes, so the slice is within the
                    // caller-provided host buffer.
                    let src = unsafe { std::slice::from_raw_parts(binding.ptr, len) };
                    seg.as_mut_slice()[..len].copy_from_slice(src);
                }
            }
        }
        self.load_ns = started.elapsed().as_nanos() as u64;
        Ok(())
    }

    fn read_from_device(&mut self) -> Result<()> {
        if matches!(self.simulation_state, SimulationState::Running(_)) {
            self.readback_scheduled = true;
            return Ok(());
        }
        self.copy_back_to_host()?;
        self.readback_scheduled = false;
        Ok(())
    }

    fn exec(&mut self) -> Result<()> {
        if self.resume_from_post_sim {
            let child = Self::spawn_noop_process()?;
            self.simulation_state = SimulationState::Running(RunningSimulation {
                child,
                started_at: Instant::now(),
                paused: false,
            });
            self.compute_ns = 0;
            return Ok(());
        }
        self.runner
            .prepare(&self.spec, &self.ctx, &self.scalars, self.tb_dir.path())?;
        if self.setup_only {
            let config_path = self.tb_dir.path().join("dpi_config.json");
            std::fs::write(&config_path, self.ctx.dpi_config_json())?;
            self.compute_ns = 0;
            self.simulation_state = SimulationState::Finished;
            return Ok(());
        }
        let child = self
            .runner
            .spawn(&self.spec, &self.ctx, self.tb_dir.path())?;
        self.simulation_state = SimulationState::Running(RunningSimulation {
            child,
            started_at: Instant::now(),
            paused: false,
        });
        Ok(())
    }

    fn pause(&mut self) -> Result<()> {
        self.pause_simulation()
    }

    fn resume(&mut self) -> Result<()> {
        self.resume_simulation()
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
                if run.paused {
                    let _ = signal_child_group(&run.child, libc::SIGCONT);
                    run.paused = false;
                }
                if let Err(err) = signal_child_group(&run.child, libc::SIGINT) {
                    tracing::warn!("failed to send SIGINT to simulator process group: {err}");
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
        fn prepare(
            &self,
            _spec: &KernelSpec,
            _ctx: &CosimContext,
            _scalar_values: &HashMap<u32, Vec<u8>>,
            _tb_dir: &Path,
        ) -> frt_cosim::error::Result<()> {
            Ok(())
        }

        fn spawn(
            &self,
            _spec: &KernelSpec,
            _ctx: &CosimContext,
            _tb_dir: &Path,
        ) -> frt_cosim::error::Result<Child> {
            let mut cmd = Command::new("/bin/sh");
            cmd.args(["-c", &format!("sleep {}", self.sleep_seconds)]);
            frt_cosim::runner::configure_sim_command(&mut cmd);
            let child = cmd.spawn()?;
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
        let arg_names = HashMap::new();
        let stream_arg_names = HashMap::new();
        let ctx = CosimContext::new(&spec).expect("create cosim context");
        CosimDevice {
            spec,
            arg_names,
            stream_arg_names,
            ctx,
            runner: Box::new(SleepRunner { sleep_seconds }),
            tb_dir: TbDir::Temp(tempfile::tempdir().expect("create temp dir")),
            _extract_dir: tempfile::tempdir().expect("create extract dir"),
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
        dev.arg_names = dev
            .spec
            .args
            .iter()
            .map(|arg| (arg.id, arg.name.clone()))
            .collect();
        dev.stream_arg_names = HashMap::new();
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
        assert!(!dev
            .is_finished()
            .expect("idle simulation should not be finished"));
        dev.kill().expect("kill idle simulation");
        assert!(dev.is_finished().expect("idle kill should mark finished"));
    }

    #[test]
    fn finish_before_exec_marks_finished() {
        let mut dev = make_test_device(0.01);
        assert!(!dev
            .is_finished()
            .expect("idle simulation should not be finished"));
        dev.finish().expect("finish idle simulation");
        assert!(dev.is_finished().expect("idle finish should mark finished"));
    }

    #[test]
    fn resume_from_post_sim_defers_copyback_until_finish() {
        let mut dev = make_test_device_with_mmap(true);
        let mut host_word = 10u32;
        dev.set_buffer_arg(
            0,
            (&raw mut host_word).cast::<u8>(),
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
    fn read_from_device_copies_back_immediately_when_idle() {
        let mut dev = make_test_device_with_mmap(false);
        let mut host_word = 10u32;
        dev.set_buffer_arg(
            0,
            (&raw mut host_word).cast::<u8>(),
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
        dev.read_from_device().expect("schedule readback");
        assert_eq!(host_word, 42);
        assert!(dev.store_ns() > 0);
        dev.finish().expect("finish idle readback");
        assert_eq!(host_word, 42);
    }

    #[test]
    fn large_buffer_is_not_truncated_before_write_to_device() {
        let mut dev = make_test_device_with_mmap(false);
        let bytes = 5 * 1024 * 1024 + 7;
        let mut host = vec![0u8; bytes];
        host[0] = 0x11;
        host[bytes - 1] = 0x22;
        dev.set_buffer_arg(0, host.as_mut_ptr(), host.len(), BufferAccess::ReadWrite)
            .expect("set buffer");
        assert_eq!(dev.ctx.buffers["buf0"].len(), bytes);
        dev.write_to_device().expect("write to device");
        let buf = dev.ctx.buffers.get("buf0").expect("buffer");
        assert_eq!(buf.len(), bytes);
        assert_eq!(buf.as_slice()[0], 0x11);
        assert_eq!(buf.as_slice()[bytes - 1], 0x22);
    }

    #[test]
    fn dpi_lib_path_finds_package_cargo_output_dir() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let fake_exe = tmp
            .path()
            .join("bazel-bin/tests/apps/bandwidth/bandwidth-host");
        let cargo_dir = tmp.path().join("bazel-bin/fpga-runtime/cargo");
        std::fs::create_dir_all(&cargo_dir).expect("create cargo dir");
        let dylib = cargo_dir.join("libfrt_dpi_verilator.dylib");
        std::fs::write(&dylib, []).expect("write dylib");

        let found = dpi_lib_path_from_exe(&fake_exe, "verilator").expect("find dpi lib");
        assert_eq!(found, dylib);
    }

    #[test]
    fn suspend_buffer_suppresses_load_and_store_transfers() {
        let mut dev = make_test_device_with_mmap(false);
        let mut host_word = 10u32;
        dev.set_buffer_arg(
            0,
            (&raw mut host_word).cast::<u8>(),
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

        assert_eq!(dev.suspend_buffer(0), 2);
        assert_eq!(dev.pending_buffers.len(), 1);
        host_word = 99;
        dev.write_to_device().expect("write to device");
        assert_eq!(
            &dev.ctx.buffers["buf0"].as_slice()[..4],
            &42u32.to_le_bytes()
        );
        dev.read_from_device().expect("read from device");
        assert_eq!(host_word, 99);
        dev.finish().expect("finish suspended readback");
        assert_eq!(host_word, 99);
    }
}
