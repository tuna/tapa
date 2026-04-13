use super::{configure_sim_command, environ::xilinx_environ, SimRunner};
use crate::context::CosimContext;
use crate::error::{CosimError, Result};
use crate::metadata::KernelSpec;
use crate::tb::xsim::XsimTbGenerator;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use which::which;

pub const XSIM_READY_FILE: &str = ".xsim-ready";
const XSIM_STARTUP_LOCK_ENV: &str = frt_shm::env::FRT_XSIM_STARTUP_LOCK;
const XSIM_STARTUP_POLL: Duration = Duration::from_millis(50);
const XSIM_START_RELEASE_QUIET_PERIOD: Duration = Duration::from_secs(1);

pub struct XsimRunner {
    pub dpi_lib: PathBuf,
    pub legacy: bool,
    pub save_waveform: bool,
    pub start_gui: bool,
    pub part_num_override: Option<String>,
}

impl XsimRunner {
    pub fn find(
        dpi_lib: PathBuf,
        legacy: bool,
        save_waveform: bool,
        start_gui: bool,
        part_num_override: Option<String>,
    ) -> Result<Self> {
        Ok(Self {
            dpi_lib,
            legacy,
            save_waveform,
            start_gui,
            part_num_override,
        })
    }
}

impl SimRunner for XsimRunner {
    fn prepare(
        &self,
        spec: &KernelSpec,
        ctx: &CosimContext,
        scalar_values: &HashMap<u32, Vec<u8>>,
        tb_dir: &Path,
    ) -> Result<()> {
        let part = self
            .part_num_override
            .as_deref()
            .or(spec.part_num.as_deref())
            .unwrap_or("xc7a100tcsg324-1");
        apply_default_nettype_wire(spec)?;
        let generator = XsimTbGenerator::new(
            spec,
            &self.dpi_lib,
            &ctx.base_addresses,
            scalar_values,
            part,
            self.save_waveform,
            self.legacy,
        );
        let tb_file = format!("tb_{}.sv", spec.top_name);

        std::fs::write(tb_dir.join(&tb_file), generator.render_tb()?)?;
        std::fs::write(tb_dir.join("run_cosim.tcl"), generator.render_tcl(tb_dir)?)?;
        Ok(())
    }

    fn spawn(
        &self,
        _spec: &KernelSpec,
        ctx: &CosimContext,
        tb_dir: &Path,
    ) -> Result<std::process::Child> {
        let ready_file = tb_dir.join(XSIM_READY_FILE);
        let _ = std::fs::remove_file(&ready_file);
        let start_go_file = tb_dir.join(".xsim-start-go");
        let start_stamp_file = tb_dir.join(".xsim-start-stamp");
        let start_token = format!(
            "{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        );
        let _ = std::fs::remove_file(&start_go_file);
        std::fs::write(&start_stamp_file, &start_token)?;
        let startup_gate = XsimStartupGate::acquire()?;
        let mode = if self.start_gui { "gui" } else { "batch" };
        let home = tb_dir.join("run");
        std::fs::create_dir_all(&home)?;
        let vivado_bin = which("vivado").map_err(|_e| CosimError::ToolNotFound("vivado".into()))?;
        let mut cmd = Command::new(vivado_bin);
        cmd.args(["-mode", mode, "-source", "run_cosim.tcl"])
            .current_dir(tb_dir)
            .env("HOME", home.as_os_str())
            .env("TMPDIR", home.as_os_str())
            .env(frt_shm::env::FRT_XSIM_WAIT_FOR_GO, "1")
            .env(frt_shm::env::TAPA_DPI_CONFIG, ctx.dpi_config_json())
            .envs(xilinx_environ());
        configure_sim_command(&mut cmd);
        let child = cmd.spawn()?;
        startup_gate.release_when_ready(
            child.id(),
            ready_file,
            start_token,
            start_stamp_file,
            start_go_file,
        );
        Ok(child)
    }
}

struct XsimStartupGate {
    #[cfg(unix)]
    file: std::fs::File,
}

impl XsimStartupGate {
    fn acquire() -> Result<Self> {
        #[cfg(unix)]
        {
            let file = super::acquire_exclusive_lock(&xsim_startup_lock_path())?;
            Ok(Self { file })
        }
        #[cfg(not(unix))]
        {
            Ok(Self {})
        }
    }

    fn release_when_ready(
        self,
        child_pid: u32,
        ready_file: PathBuf,
        start_token: String,
        start_stamp_file: PathBuf,
        start_go_file: PathBuf,
    ) {
        #[cfg(unix)]
        std::thread::spawn(move || {
            let file = self.file;
            while child_still_running(child_pid) && !ready_file.exists() {
                std::thread::sleep(XSIM_STARTUP_POLL);
            }
            if !child_still_running(child_pid) {
                return;
            }
            drop(file);
            std::thread::sleep(XSIM_START_RELEASE_QUIET_PERIOD);
            if !child_still_running(child_pid) {
                return;
            }
            let current_token = std::fs::read_to_string(&start_stamp_file).ok();
            if current_token.as_deref().map(str::trim) == Some(start_token.as_str()) {
                let _ = std::fs::write(start_go_file, b"go\n");
            }
        });
        #[cfg(not(unix))]
        let _ = (
            self,
            child_pid,
            ready_file,
            start_token,
            start_stamp_file,
            start_go_file,
        );
    }
}

fn xsim_startup_lock_path() -> PathBuf {
    std::env::var_os(XSIM_STARTUP_LOCK_ENV)
        .filter(|path| !path.is_empty())
        .map_or_else(
            || std::env::temp_dir().join("frt-xsim-startup.lock"),
            PathBuf::from,
        )
}

#[cfg(unix)]
fn child_still_running(pid: u32) -> bool {
    // SAFETY: `kill(pid, 0)` sends no signal; it only checks whether the process exists.
    // The pid comes from a just-spawned child, so the value is a valid process ID.
    let rc = unsafe { libc::kill(pid as i32, 0) };
    if rc == 0 {
        return true;
    }
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

fn apply_default_nettype_wire(spec: &KernelSpec) -> Result<()> {
    for file in &spec.verilog_files {
        let Some(ext) = file.extension().and_then(|e| e.to_str()) else {
            continue;
        };
        if !matches!(ext, "v" | "sv") {
            continue;
        }
        let content = std::fs::read_to_string(file)?;
        if content.starts_with("`default_nettype") {
            continue;
        }
        std::fs::write(file, format!("`default_nettype wire\n{content}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{mpsc, Mutex, OnceLock};
    use tempfile::tempdir;

    static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    #[test]
    fn ready_child_releases_startup_lock_before_quiet_period() {
        let _guard = ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
        let temp = tempdir().expect("tempdir");
        let lock_path = temp.path().join("startup.lock");
        let ready_file = temp.path().join(XSIM_READY_FILE);
        let start_stamp_file = temp.path().join("start.stamp");
        let start_go_file = temp.path().join("start.go");
        let start_token = "token-1".to_owned();

        // SAFETY: Test is single-threaded and guarded by ENV_LOCK.
        unsafe { std::env::set_var(XSIM_STARTUP_LOCK_ENV, &lock_path) };
        std::fs::write(&start_stamp_file, &start_token).expect("write stamp");

        let gate = XsimStartupGate::acquire().expect("acquire first gate");
        let mut child = Command::new("/bin/sh")
            .args(["-c", "sleep 5"])
            .spawn()
            .expect("spawn child");
        gate.release_when_ready(
            child.id(),
            ready_file.clone(),
            start_token,
            start_stamp_file,
            start_go_file,
        );

        std::fs::write(&ready_file, b"ready\n").expect("write ready");

        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let started = std::time::Instant::now();
            let _gate = XsimStartupGate::acquire().expect("acquire second gate");
            tx.send(started.elapsed()).expect("send acquire latency");
        });

        let acquired_after = rx
            .recv_timeout(Duration::from_millis(500))
            .expect("second gate should acquire before quiet period expires");
        assert!(acquired_after < Duration::from_millis(400));

        let _ = child.kill();
        let _ = child.wait();
        // SAFETY: Test is single-threaded and guarded by ENV_LOCK.
        unsafe { std::env::remove_var(XSIM_STARTUP_LOCK_ENV) };
    }
}
