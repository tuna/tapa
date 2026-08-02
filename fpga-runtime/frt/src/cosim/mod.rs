use crate::device::{
    sorted_args_info, stage_scalar_arg, BufferAccess, Device, RuntimeArgCategory, RuntimeArgInfo,
};
use crate::error::{FrtError, Result};
use crate::instance::Simulator;
use frt_cosim::context::CosimContext;
use frt_cosim::metadata::KernelSpec;
use frt_cosim::runner::verilator::VerilatorRunner;
use frt_cosim::runner::xsim::XsimRunner;
use frt_cosim::runner::SimRunner;
use std::collections::{HashMap, HashSet};
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
            let ctx = CosimContext::open_from_config(&spec, &json)?;
            let config: serde_json::Value = serde_json::from_str(&json).map_err(|e| {
                FrtError::MetadataParse(format!("failed to parse {}: {e}", config_path.display()))
            })?;
            // Strict frontier: every declared stream arg must be recorded
            // in the resumed config (the only reference resume mode has).
            let resumed_streams = resumed_config_stream_names(&config);
            validate_resume_stream_bindings(&stream_arg_names, &resumed_streams)?;
            ctx
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
                    *legacy,
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

    /// Sorted `'name' (arg index N)` listing of the stream args the kernel
    /// spec declares, for strict resume-mode error messages.
    fn declared_stream_args_listing(&self) -> String {
        sorted_stream_arg_listing(
            self.stream_arg_names
                .iter()
                .map(|(i, n)| (*i, n.as_str()))
                .collect(),
        )
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

/// Stream names recorded under `"streams"` in a resumed `dpi_config.json`.
///
/// A missing or malformed `"streams"` section yields an empty set so that
/// every declared stream arg is reported by [`validate_resume_stream_bindings`].
fn resumed_config_stream_names(config: &serde_json::Value) -> HashSet<String> {
    config["streams"]
        .as_object()
        .map(|streams| streams.keys().cloned().collect())
        .unwrap_or_default()
}

/// Strict resume-from-post-sim stream validation: every stream arg the
/// kernel spec declares must resolve against the resumed reference — the
/// `"streams"` section of the work directory's `dpi_config.json` that the
/// earlier setup-only run recorded. All unresolved args are collected into
/// one named error rather than failing fast on the first.
/// Sorted `'name' (arg index N)` rendering shared by the resume-mode
/// strict-binding error messages.
fn sorted_stream_arg_listing(mut args: Vec<(u32, &str)>) -> String {
    args.sort_unstable();
    args.iter()
        .map(|(i, n)| format!("'{n}' (arg index {i})"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn validate_resume_stream_bindings(
    declared: &HashMap<u32, String>,
    resumed: &HashSet<String>,
) -> Result<()> {
    let unbound: Vec<(u32, &str)> = declared
        .iter()
        .filter(|(_, name)| !resumed.contains(name.as_str()))
        .map(|(index, name)| (*index, name.as_str()))
        .collect();
    if unbound.is_empty() {
        return Ok(());
    }
    let listed = sorted_stream_arg_listing(unbound);
    Err(FrtError::ResumeStreamBinding(format!(
        "kernel stream args with no entry in the resumed dpi_config.json: {listed}; the \
         resumed work directory is stale or was built for a different kernel — regenerate \
         it with --cosim_setup_only before using --cosim_resume_from_post_sim"
    )))
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
    // the install prefix). With FRT linked statically, dladdr resolves to
    // the host executable instead, so the install prefix is located via
    // TAPA_HOME (see `dpi_lib_path_from_exe`).
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
    // Installed layout with statically linked FRT: the libraries live under
    // the TAPA package root advertised via TAPA_HOME (this is the layout the
    // staging tests exercise after `install.sh`).
    if let Ok(home) = std::env::var("TAPA_HOME") {
        let home = PathBuf::from(home);
        search_dirs.push(home.join("usr/lib"));
        search_dirs.push(home.join("lib"));
    }
    // Also search LD_LIBRARY_PATH (covers staging tests that copy binaries)
    if let Ok(ldpath) = std::env::var("LD_LIBRARY_PATH") {
        for dir in ldpath.split(':') {
            if !dir.is_empty() {
                search_dirs.push(PathBuf::from(dir));
            }
        }
    }
    let candidates = dpi_library_candidates(variant);
    for candidate in candidates {
        for base in &search_dirs {
            let p = base.join(&candidate);
            if p.exists() {
                // Downstream cosim tools run from deep temp/obj subdirectories:
                // Verilator's `c++` link from `obj_dir` and XSIM's `-sv_root`
                // (derived from this path's parent) both resolve relative to
                // their own CWD, not ours. A path resolved relative to the test
                // CWD is unusable there, so return an absolute path.
                return Ok(std::fs::canonicalize(&p).unwrap_or(p));
            }
        }
    }
    Err(FrtError::MetadataParse(format!(
        "libfrt_dpi_{variant} shared library not found next to executable, \
         under TAPA_HOME, or on LD_LIBRARY_PATH"
    )))
}

fn dpi_library_candidates(variant: &str) -> [String; 2] {
    let native = format!(
        "{}frt_dpi_{variant}{}",
        std::env::consts::DLL_PREFIX,
        std::env::consts::DLL_SUFFIX,
    );
    let fallback_suffix = if std::env::consts::DLL_SUFFIX == ".so" {
        ".dylib"
    } else {
        ".so"
    };
    [native, format!("libfrt_dpi_{variant}{fallback_suffix}")]
}

impl Device for CosimDevice {
    fn set_scalar_arg(&mut self, index: u32, value: &[u8]) -> Result<()> {
        stage_scalar_arg(&mut self.scalars, index, value);
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
        if self.resume_from_post_sim {
            // In resume mode the context has no live streams (the previous
            // run already produced/consumed them; see
            // `CosimContext::open_from_config`), so there is nothing left to
            // *bind* — skipping the binding itself is required, not a bug.
            // What must not be skipped is a host/archive mismatch: the
            // bound index must be a declared stream arg. Alignment with the
            // resumed dpi_config.json itself was validated wholesale by
            // `CosimDevice::open`, the only production constructor.
            if self.stream_arg_name(index).is_err() {
                return Err(FrtError::ResumeStreamBinding(format!(
                    "arg index {index} is not a stream arg declared by the kernel \
                     spec (declared stream args: {}); the host binary and the kernel \
                     archive disagree — check that the host was built against this \
                     archive",
                    self.declared_stream_args_listing()
                )));
            }
            return Ok(());
        }
        let name = self.stream_arg_name(index)?.to_owned();
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
        sorted_args_info(self.spec.args.iter().map(|arg| {
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
            RuntimeArgInfo {
                index: arg.id,
                name: arg.name.clone(),
                type_name,
                category,
            }
        }))
    }

    crate::device::impl_ns_getters! {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use frt_cosim::metadata::{ArgKind, ArgSpec, Mode, StreamDir, StreamProtocol};
    use std::collections::{HashMap, HashSet};
    use std::process::{Child, Command};
    use std::time::Duration;

    /// Serializes tests that mutate process environment variables.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

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
        let library = cargo_dir.join(&dpi_library_candidates("verilator")[0]);
        std::fs::write(&library, []).expect("write DPI library");

        let found = dpi_lib_path_from_exe(&fake_exe, "verilator").expect("find dpi lib");
        // The resolved path must be absolute so it loads from the deep CWDs
        // Verilator/XSIM run their link steps in (see `dpi_lib_path_from_exe`).
        assert!(
            found.is_absolute(),
            "resolved DPI path must be absolute: {found:?}"
        );
        assert_eq!(
            std::fs::canonicalize(&found).expect("canonicalize found"),
            std::fs::canonicalize(&library).expect("canonicalize library"),
        );
    }

    #[test]
    fn dpi_lib_path_finds_installed_layout_via_tapa_home() {
        let tmp = tempfile::tempdir().expect("tempdir");
        // Installed layout: the DPI library lives at $TAPA_HOME/usr/lib while
        // the statically linked host binary can sit anywhere.
        let home = tmp.path().join("tapa-home");
        let lib_dir = home.join("usr/lib");
        std::fs::create_dir_all(&lib_dir).expect("create usr/lib");
        let library = lib_dir.join(&dpi_library_candidates("xsim")[0]);
        std::fs::write(&library, []).expect("write DPI library");
        let fake_exe = tmp.path().join("somewhere/else/vadd-host");

        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let prev = std::env::var_os("TAPA_HOME");
        std::env::set_var("TAPA_HOME", &home);
        let found = dpi_lib_path_from_exe(&fake_exe, "xsim");
        match prev {
            Some(v) => std::env::set_var("TAPA_HOME", v),
            None => std::env::remove_var("TAPA_HOME"),
        }

        let found = found.expect("find dpi lib via TAPA_HOME");
        assert_eq!(
            std::fs::canonicalize(&found).expect("canonicalize found"),
            std::fs::canonicalize(&library).expect("canonicalize library"),
        );
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

    fn declared_stream_args_for_test() -> HashMap<u32, String> {
        HashMap::from([(1u32, "s_in".to_owned()), (2u32, "s_out".to_owned())])
    }

    fn make_resume_stream_test_device() -> CosimDevice {
        let mut dev = make_test_device(0.01);
        dev.spec.args = vec![
            ArgSpec {
                name: "s_in".to_owned(),
                id: 1,
                kind: ArgKind::Stream {
                    width: 32,
                    depth: 16,
                    dir: StreamDir::In,
                    protocol: StreamProtocol::ApFifo,
                },
            },
            ArgSpec {
                name: "s_out".to_owned(),
                id: 2,
                kind: ArgKind::Stream {
                    width: 32,
                    depth: 16,
                    dir: StreamDir::Out,
                    protocol: StreamProtocol::ApFifo,
                },
            },
        ];
        dev.stream_arg_names = declared_stream_args_for_test();
        dev.arg_names = dev.stream_arg_names.clone();
        // `CosimContext::open_from_config` builds no streams in resume mode;
        // mirror that without allocating real shm queues.
        dev.ctx = CosimContext {
            buffers: HashMap::new(),
            streams: HashMap::new(),
            stream_path_overrides: HashMap::new(),
            base_addresses: HashMap::new(),
        };
        dev.resume_from_post_sim = true;
        dev
    }

    #[test]
    fn resumed_config_stream_names_reads_streams_section() {
        let config = serde_json::json!({
            "buffers": {},
            "streams": {
                "s_in": {"path": "/dev/shm/s_in", "dpi_width_bytes": 5},
                "s_out": {"path": "/dev/shm/s_out", "dpi_width_bytes": 5}
            }
        });
        let names = resumed_config_stream_names(&config);
        assert_eq!(names.len(), 2);
        assert!(names.contains("s_in"));
        assert!(names.contains("s_out"));

        // A missing or malformed "streams" section yields an empty set so
        // that validation reports every declared stream arg as unbound.
        assert!(resumed_config_stream_names(&serde_json::json!({"buffers": {}})).is_empty());
        assert!(resumed_config_stream_names(&serde_json::json!({"streams": []})).is_empty());
    }

    #[test]
    fn resume_validation_passes_when_all_declared_streams_are_recorded() {
        let declared = declared_stream_args_for_test();
        let resumed = HashSet::from(["s_in".to_owned(), "s_out".to_owned()]);
        validate_resume_stream_bindings(&declared, &resumed)
            .expect("all declared streams are recorded in the resumed config");
    }

    #[test]
    fn resume_validation_passes_when_kernel_declares_no_streams() {
        // Even a config without any recorded streams cannot make this fail:
        // there is nothing to bind. This is the resume-xosim fixture shape.
        validate_resume_stream_bindings(&HashMap::new(), &HashSet::new())
            .expect("nothing declared means nothing to reject");
    }

    #[test]
    fn resume_validation_lists_all_unbound_streams_in_one_error() {
        let mut declared = declared_stream_args_for_test();
        declared.insert(3, "s_extra".to_owned());
        let resumed = HashSet::from(["s_in".to_owned()]);
        let err = validate_resume_stream_bindings(&declared, &resumed)
            .expect_err("s_out and s_extra are not recorded in the resumed config");
        let msg = err.to_string();
        assert!(
            msg.contains("resume-from-post-sim stream binding error"),
            "{msg}"
        );
        // One collected error naming every unbound arg, not fail-fast...
        assert!(msg.contains("'s_out' (arg index 2)"), "{msg}");
        assert!(msg.contains("'s_extra' (arg index 3)"), "{msg}");
        // ...and never naming a bound arg, with the remediation attached.
        assert!(!msg.contains("'s_in'"), "{msg}");
        assert!(msg.contains("--cosim_setup_only"), "{msg}");
    }

    #[test]
    fn resume_set_stream_arg_accepts_declared_and_recorded_streams() {
        let mut dev = make_resume_stream_test_device();
        dev.set_stream_arg(1, "/dev/shm/host_s_in")
            .expect("declared and recorded stream binds as a resume no-op");
        dev.set_stream_arg(2, "/dev/shm/host_s_out")
            .expect("declared and recorded stream binds as a resume no-op");
    }

    #[test]
    fn resume_set_stream_arg_rejects_undeclared_arg_index() {
        let mut dev = make_resume_stream_test_device();
        let err = dev
            .set_stream_arg(0, "/dev/shm/nope")
            .expect_err("arg index 0 is not a declared stream arg");
        let msg = err.to_string();
        assert!(
            msg.contains("resume-from-post-sim stream binding error"),
            "{msg}"
        );
        assert!(msg.contains("arg index 0"), "{msg}");
        assert!(msg.contains("'s_in' (arg index 1)"), "{msg}");
        assert!(msg.contains("'s_out' (arg index 2)"), "{msg}");
    }

    #[test]
    fn resume_set_stream_arg_empty_path_is_a_no_op() {
        let mut dev = make_resume_stream_test_device();
        // Empty paths short-circuit before any stream validation, as before.
        dev.set_stream_arg(0, "")
            .expect("empty shm path is a no-op");
        dev.set_stream_arg(1, "")
            .expect("empty shm path is a no-op");
    }

    #[test]
    fn non_resume_set_stream_arg_binds_shm_path_override() {
        // Queue names live in system-wide shm; keep them process-unique.
        let stream_name = format!("frt_resume_strict_{}_s", std::process::id());
        let mut dev = make_test_device(0.01);
        dev.spec.args = vec![ArgSpec {
            name: stream_name.clone(),
            id: 1,
            kind: ArgKind::Stream {
                width: 32,
                depth: 16,
                dir: StreamDir::In,
                protocol: StreamProtocol::ApFifo,
            },
        }];
        dev.arg_names = HashMap::from([(1u32, stream_name.clone())]);
        dev.stream_arg_names = dev.arg_names.clone();
        dev.ctx = CosimContext::new(&dev.spec).expect("create cosim context");

        dev.set_stream_arg(1, "/dev/shm/host_s")
            .expect("declared stream binds outside resume mode");
        assert_eq!(
            dev.ctx
                .stream_path_overrides
                .get(&stream_name)
                .map(String::as_str),
            Some("/dev/shm/host_s"),
        );

        // Unknown stream indices were already an error outside resume mode.
        let err = dev
            .set_stream_arg(9, "/dev/shm/nope")
            .expect_err("unknown stream arg index");
        assert!(
            err.to_string().contains("no stream arg at index 9"),
            "{err}"
        );
    }
}
