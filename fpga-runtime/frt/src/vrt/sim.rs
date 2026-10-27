//! RTL simulation of one SLASH VBIN through its `vpp_sim` process.

use super::vbin::{register_words, Arg, ArgKind, MemoryTarget, Platform, Vbin};
use super::zmq::{FrameTransport, ZmqTransport};
use crate::device::{BufferAccess, Device};
use crate::error::{FrtError, Result};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const HBM_BASE: u64 = 0x40_0000_0000;
const DDR_BASE: u64 = 0x600_0000_0000;
const MEMORY_WINDOW_BYTES: u64 = 128 * 1024 * 1024;
const BUFFER_ALIGNMENT: u64 = 4096;
const CTRL_OFFSET: u32 = 0;
const CTRL_START: u32 = 1;
const CTRL_DONE: u32 = 1 << 1;
const FINISH_TIMEOUT: Duration = Duration::from_secs(60);
const FINISH_POLL: Duration = Duration::from_millis(1);
const PROCESS_EXIT_TIMEOUT: Duration = Duration::from_secs(5);
const PROCESS_EXIT_POLL: Duration = Duration::from_millis(10);

#[derive(Clone, Copy)]
enum ExecutionState {
    Idle,
    Running(Instant),
    Finished,
}

#[derive(Clone, Copy)]
struct BufferBinding {
    ptr: *mut u8,
    bytes: usize,
    access: BufferAccess,
    address: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct BumpRange {
    next: u64,
    end: u64,
}

impl BumpRange {
    fn new(base: u64) -> Self {
        Self {
            next: base,
            end: base + MEMORY_WINDOW_BYTES,
        }
    }

    fn allocate(&mut self, bytes: u64, target: &str) -> Result<u64> {
        if bytes == 0 {
            return Err(FrtError::Vrt(
                "zero-length simulation buffer allocation".into(),
            ));
        }
        let reserved = bytes
            .checked_add(BUFFER_ALIGNMENT - 1)
            .map(|size| size & !(BUFFER_ALIGNMENT - 1))
            .ok_or_else(|| FrtError::Vrt(format!("{target} simulation buffer size overflow")))?;
        let address = self.next;
        let next = address
            .checked_add(reserved)
            .ok_or_else(|| FrtError::Vrt(format!("{target} simulation buffer address overflow")))?;
        if next > self.end {
            return Err(FrtError::Vrt(format!(
                "{target} simulation allocation of {bytes} bytes exceeds the 128MiB window at {address:#x}"
            )));
        }
        self.next = next;
        Ok(address)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct AddressAllocators {
    // Every HBM connection reaches the same simulated BRAM window. Sharing
    // this cursor across HBM bank numbers prevents overlapping allocations.
    hbm: BumpRange,
    ddr: BumpRange,
}

impl AddressAllocators {
    fn new() -> Self {
        Self {
            hbm: BumpRange::new(HBM_BASE),
            ddr: BumpRange::new(DDR_BASE),
        }
    }

    fn allocate(&mut self, target: MemoryTarget, bytes: u64) -> Result<u64> {
        match target {
            MemoryTarget::Ddr => self.ddr.allocate(bytes, "DDR"),
            MemoryTarget::Hbm(_) | MemoryTarget::HbmVnoc => self.hbm.allocate(bytes, "HBM"),
        }
    }
}

struct SimProcess {
    child: Option<Child>,
}

impl SimProcess {
    fn spawn(executable: &Path, directory: &Path, endpoint: &str) -> Result<Self> {
        let child = Command::new(executable)
            .current_dir(directory)
            .arg(endpoint)
            .stdin(Stdio::null())
            .spawn()
            .map_err(|error| {
                FrtError::Vrt(format!(
                    "cannot launch SLASH simulator `{}`: {error}",
                    executable.display()
                ))
            })?;
        Ok(Self { child: Some(child) })
    }

    fn shutdown(&mut self) {
        let Some(child) = &mut self.child else {
            return;
        };
        let started = Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    if !status.success() {
                        tracing::warn!(%status, "SLASH simulator exited unsuccessfully");
                    }
                    self.child = None;
                    return;
                }
                Ok(None) => {}
                Err(error) => {
                    tracing::warn!(%error, "cannot poll the SLASH simulator during cleanup");
                    break;
                }
            }
            if started.elapsed() >= PROCESS_EXIT_TIMEOUT {
                tracing::warn!("SLASH simulator did not exit after the exit command; killing it");
                break;
            }
            std::thread::sleep(PROCESS_EXIT_POLL);
        }

        let Some(mut child) = self.child.take() else {
            return;
        };
        if let Err(error) = child.kill() {
            if error.kind() != std::io::ErrorKind::InvalidInput {
                tracing::warn!(%error, "cannot kill the SLASH simulator");
            }
        }
        match child.wait() {
            Ok(status) if !status.success() => {
                tracing::debug!(%status, "SLASH simulator stopped during cleanup");
            }
            Ok(_) => {}
            Err(error) => {
                tracing::warn!(%error, "cannot reap the SLASH simulator");
            }
        }
    }
}

impl Drop for SimProcess {
    fn drop(&mut self) {
        self.shutdown();
    }
}

pub struct SimDevice {
    transport: Box<dyn FrameTransport>,
    process: Option<SimProcess>,
    // Own the archive and its TempDir until after the process and transport
    // have stopped; xsim.dir and the IPC endpoint live under this directory.
    vbin: Vbin,
    allocators: AddressAllocators,
    scalars: BTreeMap<u32, Vec<u8>>,
    buffers: BTreeMap<u32, BufferBinding>,
    execution_state: ExecutionState,
    session_active: bool,
    load_ns: u64,
    compute_ns: u64,
    store_ns: u64,
}

// SAFETY: SimDevice is moved, never shared; all operations require &mut self.
// Native transport/process handles are exclusively owned and host pointers use
// the same caller-lifetime contract as the other raw-buffer Device backends.
unsafe impl Send for SimDevice {}

fn elapsed_ns(since: Instant) -> u64 {
    u64::try_from(since.elapsed().as_nanos()).unwrap_or(u64::MAX)
}

fn reply_error(reply: &[u8], command: &str) -> Result<()> {
    if reply == b"ERR" || reply.starts_with(b"ERR:") || reply.starts_with(b"ERR ") {
        return Err(FrtError::Vrt(format!(
            "SLASH simulation `{command}` failed: {}",
            String::from_utf8_lossy(reply)
        )));
    }
    Ok(())
}

fn expect_ok(reply: &[u8], command: &str) -> Result<()> {
    reply_error(reply, command)?;
    if reply == b"OK" {
        Ok(())
    } else {
        Err(FrtError::Vrt(format!(
            "SLASH simulation `{command}` returned an unexpected reply: {}",
            String::from_utf8_lossy(reply)
        )))
    }
}

fn json_reply<T: DeserializeOwned>(reply: &[u8], what: &str) -> Result<T> {
    reply_error(reply, what)?;
    serde_json::from_slice(reply).map_err(|error| {
        FrtError::Vrt(format!(
            "invalid SLASH simulation {what} reply `{}`: {error}",
            String::from_utf8_lossy(reply)
        ))
    })
}

fn validated_vpp_sim(vbin: &Vbin) -> Result<PathBuf> {
    if vbin.platform != Platform::Simulation {
        return Err(FrtError::Vrt(
            "SimDevice requires a Platform=Simulation VBIN".into(),
        ));
    }
    let executable = vbin
        .simulation_executable
        .as_ref()
        .ok_or_else(|| FrtError::Vrt("simulation VBIN contains no vpp_sim executable".into()))?;
    let root = fs::canonicalize(vbin.extraction_dir()).map_err(|error| {
        FrtError::Vrt(format!(
            "cannot validate the VBIN extraction directory: {error}"
        ))
    })?;
    let executable = fs::canonicalize(executable)
        .map_err(|error| FrtError::Vrt(format!("cannot validate extracted vpp_sim: {error}")))?;
    if !executable.starts_with(&root)
        || executable.file_name().and_then(|n| n.to_str()) != Some("vpp_sim")
    {
        return Err(FrtError::Vrt(
            "extracted vpp_sim is outside the VBIN extraction directory".into(),
        ));
    }
    let metadata = fs::symlink_metadata(&executable)
        .map_err(|error| FrtError::Vrt(format!("cannot inspect extracted vpp_sim: {error}")))?;
    if !metadata.is_file() {
        return Err(FrtError::Vrt(
            "extracted vpp_sim is not a regular file".into(),
        ));
    }
    // Do not rely on archived mode bits for execution. Restore only owner
    // execute on the validated simulator, never on xsim payload files.
    let mut permissions = metadata.permissions();
    permissions.set_mode(permissions.mode() | 0o100);
    fs::set_permissions(&executable, permissions).map_err(|error| {
        FrtError::Vrt(format!(
            "cannot make extracted vpp_sim executable `{}`: {error}",
            executable.display()
        ))
    })?;
    Ok(executable)
}

fn endpoint(vbin: &Vbin) -> Result<String> {
    let socket = vbin.extraction_dir().join("zmq.socket");
    let socket = socket
        .to_str()
        .ok_or_else(|| FrtError::Vrt("non-UTF-8 SLASH simulation IPC socket path".into()))?;
    Ok(format!("ipc://{socket}"))
}

impl SimDevice {
    pub fn open(vbin: Vbin) -> Result<Self> {
        let executable = validated_vpp_sim(&vbin)?;
        let endpoint = endpoint(&vbin)?;
        // Load/connect libzmq before creating a child. If spawning then fails,
        // transport Drop closes the socket and context without libvrtd involvement.
        let transport = ZmqTransport::connect(&endpoint)?;
        let process = SimProcess::spawn(&executable, vbin.extraction_dir(), &endpoint)?;
        let mut this = Self::new(vbin, Box::new(transport), Some(process));
        this.start_session()?;
        Ok(this)
    }

    fn new(vbin: Vbin, transport: Box<dyn FrameTransport>, process: Option<SimProcess>) -> Self {
        Self {
            transport,
            process,
            vbin,
            allocators: AddressAllocators::new(),
            scalars: BTreeMap::new(),
            buffers: BTreeMap::new(),
            execution_state: ExecutionState::Idle,
            session_active: false,
            load_ns: 0,
            compute_ns: 0,
            store_ns: 0,
        }
    }

    fn request(&mut self, command: &Value, binary: Option<&[u8]>) -> Result<Vec<u8>> {
        let json = serde_json::to_vec(command)
            .map_err(|error| FrtError::Vrt(format!("cannot encode ZMQ command: {error}")))?;
        match binary {
            Some(binary) => self.transport.request(&[&json, binary]),
            None => self.transport.request(&[&json]),
        }
    }

    fn start_session(&mut self) -> Result<()> {
        let reply = self.request(&json!({"command": "start"}), None)?;
        expect_ok(&reply, "start")?;
        self.session_active = true;
        Ok(())
    }

    fn exit_session(&mut self) -> Result<()> {
        let result = self
            .request(&json!({"command": "exit"}), None)
            .and_then(|reply| expect_ok(&reply, "exit"));
        self.session_active = false;
        result
    }

    fn register_address(&self, offset: u32) -> Result<u64> {
        self.vbin
            .kernel
            .base_address
            .checked_add(u64::from(offset))
            .ok_or_else(|| FrtError::Vrt("simulation register address overflow".into()))
    }

    fn write_register(&mut self, offset: u32, value: u32) -> Result<()> {
        let address = self.register_address(offset)?;
        let reply = self.request(
            &json!({"command": "reg", "addr": address, "val": value}),
            None,
        )?;
        expect_ok(&reply, "reg")
    }

    fn read_register(&mut self, offset: u32) -> Result<u32> {
        let address = self.register_address(offset)?;
        let reply = self.request(
            &json!({"command": "fetch", "type": "scalar", "addr": address}),
            None,
        )?;
        json_reply(&reply, "scalar fetch")
    }

    fn populate(&mut self, address: u64, data: &[u8]) -> Result<()> {
        let reply = self.request(
            &json!({"command": "populate", "addr": address, "size": data.len()}),
            Some(data),
        )?;
        expect_ok(&reply, "populate")
    }

    fn fetch_buffer(&mut self, address: u64, size: usize) -> Result<Vec<u8>> {
        let reply = self.request(
            &json!({"command": "fetch", "type": "buffer", "addr": address, "size": size}),
            None,
        )?;
        let data: Vec<u8> = json_reply(&reply, "buffer fetch")?;
        if data.len() != size {
            return Err(FrtError::Vrt(format!(
                "SLASH simulation buffer fetch at {address:#x} returned {} bytes, expected {size}",
                data.len()
            )));
        }
        Ok(data)
    }

    fn write_value(&mut self, arg: &Arg, value: &[u8]) -> Result<()> {
        for (offset, word) in register_words(arg.offset, arg.range, value)? {
            self.write_register(offset, word)?;
        }
        Ok(())
    }

    fn ensure_not_running(&self, action: &str) -> Result<()> {
        if matches!(self.execution_state, ExecutionState::Running(_)) {
            Err(FrtError::Vrt(format!(
                "cannot {action} while the SLASH simulation kernel is running"
            )))
        } else {
            Ok(())
        }
    }

    fn shutdown(&mut self) {
        if self.session_active {
            if let Err(error) = self.exit_session() {
                tracing::warn!(%error, "cannot stop the SLASH simulation session cleanly");
            }
        }
        self.transport.close();
        if let Some(process) = &mut self.process {
            process.shutdown();
        }
    }
}

impl Drop for SimDevice {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl Device for SimDevice {
    fn set_scalar_arg(&mut self, index: u32, value: &[u8]) -> Result<()> {
        self.ensure_not_running("change an argument")?;
        let arg = self.vbin.kernel.arg(index)?.clone();
        if arg.kind != ArgKind::Scalar {
            return Err(FrtError::Vrt(format!(
                "argument {index} (`{}`) is not a scalar",
                arg.name
            )));
        }
        register_words(arg.offset, arg.range, value)?;
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
        self.ensure_not_running("change an argument")?;
        if bytes == 0 {
            return Err(FrtError::Vrt(format!(
                "zero-length buffer argument {index}"
            )));
        }
        if ptr.is_null() {
            return Err(FrtError::Vrt(format!(
                "null host pointer for buffer argument {index}"
            )));
        }
        let arg = self.vbin.kernel.arg(index)?.clone();
        if arg.kind != ArgKind::Buffer || arg.range != 64 {
            return Err(FrtError::Vrt(format!(
                "argument {index} (`{}`) is not a 64-bit buffer pointer",
                arg.name
            )));
        }
        let target = self.vbin.kernel.memory_target(&arg)?;
        let size = u64::try_from(bytes)
            .map_err(|error| FrtError::Vrt(format!("buffer argument {index}: {error}")))?;
        let address = self.allocators.allocate(target, size)?;
        self.buffers.insert(
            index,
            BufferBinding {
                ptr,
                bytes,
                access,
                address,
            },
        );
        Ok(())
    }

    fn set_stream_arg(&mut self, index: u32, _shm_path: &str) -> Result<()> {
        Err(FrtError::Vrt(format!(
            "argument {index}: host streams are unsupported by the SLASH simulation backend"
        )))
    }

    fn write_to_device(&mut self) -> Result<()> {
        self.ensure_not_running("upload buffers")?;
        let started = Instant::now();
        let buffers = self
            .buffers
            .iter()
            .filter(|(_, binding)| binding.access.loads_from_host())
            .map(|(index, binding)| (*index, *binding))
            .collect::<Vec<_>>();
        for (index, binding) in buffers {
            // SAFETY: the caller keeps this host region readable for bytes;
            // libzmq copies it before populate returns.
            let data = unsafe { std::slice::from_raw_parts(binding.ptr, binding.bytes) };
            self.populate(binding.address, data).map_err(|error| {
                FrtError::Vrt(format!(
                    "cannot upload simulation buffer argument {index}: {error}"
                ))
            })?;
        }
        self.load_ns = elapsed_ns(started);
        Ok(())
    }

    fn read_from_device(&mut self) -> Result<()> {
        if matches!(self.execution_state, ExecutionState::Running(_)) {
            self.finish()?;
        }
        let started = Instant::now();
        let buffers = self
            .buffers
            .iter()
            .filter(|(_, binding)| binding.access.stores_to_host())
            .map(|(index, binding)| (*index, *binding))
            .collect::<Vec<_>>();
        for (index, binding) in buffers {
            let data = self
                .fetch_buffer(binding.address, binding.bytes)
                .map_err(|error| {
                    FrtError::Vrt(format!(
                        "cannot download simulation buffer argument {index}: {error}"
                    ))
                })?;
            // SAFETY: the caller keeps this host region writable for bytes;
            // fetch_buffer validated exactly that returned length.
            unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), binding.ptr, binding.bytes) };
        }
        self.store_ns = elapsed_ns(started);
        Ok(())
    }

    fn exec(&mut self) -> Result<()> {
        self.ensure_not_running("start the kernel again")?;
        for arg in self.vbin.kernel.args.clone() {
            if !arg.writable {
                continue;
            }
            let value = match arg.kind {
                ArgKind::Scalar => self.scalars.get(&arg.index).cloned().ok_or_else(|| {
                    FrtError::Vrt(format!(
                        "unbound scalar argument {} (`{}`)",
                        arg.index, arg.name
                    ))
                })?,
                ArgKind::Buffer => self
                    .buffers
                    .get(&arg.index)
                    .map(|binding| binding.address.to_le_bytes().to_vec())
                    .ok_or_else(|| {
                        FrtError::Vrt(format!(
                            "unbound buffer argument {} (`{}`)",
                            arg.index, arg.name
                        ))
                    })?,
            };
            self.write_value(&arg, &value)?;
        }
        let started = Instant::now();
        self.write_register(CTRL_OFFSET, CTRL_START)?;
        self.compute_ns = 0;
        self.execution_state = ExecutionState::Running(started);
        Ok(())
    }

    fn finish(&mut self) -> Result<()> {
        if !matches!(self.execution_state, ExecutionState::Running(_)) {
            return Ok(());
        }
        let wait_started = Instant::now();
        while !self.is_finished()? {
            if wait_started.elapsed() >= FINISH_TIMEOUT {
                return Err(FrtError::Vrt(format!(
                    "SLASH simulation kernel did not finish within {}s",
                    FINISH_TIMEOUT.as_secs()
                )));
            }
            std::thread::sleep(FINISH_POLL);
        }
        Ok(())
    }

    fn kill(&mut self) -> Result<()> {
        Err(FrtError::Vrt(
            "SLASH simulation has no kernel cancellation; closing the device exits vpp_sim".into(),
        ))
    }

    fn is_finished(&mut self) -> Result<bool> {
        let ExecutionState::Running(started) = self.execution_state else {
            return Ok(matches!(self.execution_state, ExecutionState::Finished));
        };
        if self.read_register(CTRL_OFFSET)? & CTRL_DONE == 0 {
            return Ok(false);
        }
        self.compute_ns = elapsed_ns(started);
        self.execution_state = ExecutionState::Finished;
        Ok(true)
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
    use super::{
        endpoint, validated_vpp_sim, AddressAllocators, FrameTransport, SimDevice, SimProcess,
        DDR_BASE, HBM_BASE, MEMORY_WINDOW_BYTES,
    };
    use crate::device::{BufferAccess, Device};
    use crate::error::{FrtError, Result};
    use crate::vrt::vbin::{MemoryTarget, Vbin};
    use serde_json::Value;
    use std::collections::BTreeMap;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use tar::{Builder, Header};

    const BASE: u64 = 0x202_0000_0000;
    const MAP: &str = r#"<SystemMap>
  <Platform>Simulation</Platform>
  <ClockFrequency>300000000</ClockFrequency>
  <Kernel>
    <Name>VecAdd_0</Name>
    <BaseAddress>0x20200000000</BaseAddress>
    <Range>0x1000</Range>
    <functional_args>
      <arg idx="0" name="a" type="buffer" offset="0x10" range="64" r="1" w="1" port="m_axi_a" />
      <arg idx="1" name="n" type="scalar" offset="0x34" range="64" r="1" w="1" />
    </functional_args>
    <connection port="m_axi_a" target="HBM0" />
  </Kernel>
</SystemMap>"#;

    #[derive(Debug)]
    struct RecordedRequest {
        command: Value,
        frames: Vec<Vec<u8>>,
    }

    #[derive(Default)]
    struct FakeState {
        requests: Vec<RecordedRequest>,
        registers: BTreeMap<u64, u32>,
        memory: BTreeMap<u64, Vec<u8>>,
        closed: bool,
    }

    struct FakeTransport {
        state: Arc<Mutex<FakeState>>,
        exit_marker: Option<PathBuf>,
    }

    impl FakeTransport {
        fn new(state: Arc<Mutex<FakeState>>) -> Self {
            Self {
                state,
                exit_marker: None,
            }
        }

        fn with_exit_marker(state: Arc<Mutex<FakeState>>, marker: PathBuf) -> Self {
            Self {
                state,
                exit_marker: Some(marker),
            }
        }
    }

    impl FrameTransport for FakeTransport {
        fn request(&mut self, frames: &[&[u8]]) -> Result<Vec<u8>> {
            let command: Value = serde_json::from_slice(frames[0])
                .map_err(|error| FrtError::Vrt(format!("bad fake request: {error}")))?;
            let name = command["command"].as_str().unwrap_or_default().to_owned();
            self.state
                .lock()
                .expect("fake state")
                .requests
                .push(RecordedRequest {
                    command: command.clone(),
                    frames: frames.iter().map(|frame| frame.to_vec()).collect(),
                });
            match name.as_str() {
                "start" => Ok(b"OK".to_vec()),
                "exit" => {
                    if let Some(marker) = &self.exit_marker {
                        fs::write(marker, []).map_err(FrtError::from)?;
                    }
                    Ok(b"OK".to_vec())
                }
                "reg" => {
                    let address = command["addr"].as_u64().expect("register address");
                    let value = command["val"]
                        .as_u64()
                        .and_then(|value| u32::try_from(value).ok())
                        .expect("register value");
                    self.state
                        .lock()
                        .expect("fake state")
                        .registers
                        .insert(address, value);
                    Ok(b"OK".to_vec())
                }
                "populate" => {
                    if frames.len() != 2 {
                        return Err(FrtError::Vrt("populate needs two frames".into()));
                    }
                    let address = command["addr"].as_u64().expect("buffer address");
                    let size = usize::try_from(command["size"].as_u64().expect("buffer size"))
                        .expect("fake buffer size");
                    if frames[1].len() != size {
                        return Err(FrtError::Vrt("populate size does not match frame 1".into()));
                    }
                    self.state
                        .lock()
                        .expect("fake state")
                        .memory
                        .insert(address, frames[1].to_vec());
                    Ok(b"OK".to_vec())
                }
                "fetch" if command["type"] == "scalar" => {
                    let address = command["addr"].as_u64().expect("scalar address");
                    let value = self
                        .state
                        .lock()
                        .expect("fake state")
                        .registers
                        .get(&address)
                        .copied()
                        .unwrap_or(0);
                    Ok(serde_json::to_vec(&value).expect("scalar reply"))
                }
                "fetch" if command["type"] == "buffer" => {
                    let address = command["addr"].as_u64().expect("buffer address");
                    let size = usize::try_from(command["size"].as_u64().expect("buffer size"))
                        .expect("fake buffer size");
                    let state = self.state.lock().expect("fake state");
                    let data = state.memory.get(&address).ok_or_else(|| {
                        FrtError::Vrt(format!("fake has no buffer at {address:#x}"))
                    })?;
                    if data.len() != size {
                        return Err(FrtError::Vrt("fake buffer has the wrong size".into()));
                    }
                    Ok(serde_json::to_vec(data).expect("buffer reply"))
                }
                _ => Err(FrtError::Vrt(format!("unknown fake command `{name}`"))),
            }
        }

        fn close(&mut self) {
            self.state.lock().expect("fake state").closed = true;
        }
    }

    fn append(archive: &mut Builder<&mut Vec<u8>>, path: &str, bytes: &[u8], mode: u32) {
        let mut header = Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(mode);
        header.set_cksum();
        archive
            .append_data(&mut header, path, bytes)
            .expect("append simulation fixture");
    }

    fn simulation_vbin(script: &[u8]) -> Vbin {
        let temp = tempfile::tempdir().expect("archive temp dir");
        let path = temp.path().join("design.vbin");
        let mut bytes = Vec::new();
        let mut archive = Builder::new(&mut bytes);
        append(&mut archive, "system_map.xml", MAP.as_bytes(), 0o644);
        append(&mut archive, "vpp_sim", script, 0o644);
        append(
            &mut archive,
            "xsim.dir/top_wrapper_behav/xsimk.so",
            b"model",
            0o644,
        );
        archive.finish().expect("finish simulation VBIN");
        drop(archive);
        fs::write(&path, bytes).expect("write simulation VBIN");
        Vbin::open(&path).expect("open simulation VBIN")
    }

    fn fake_device() -> (SimDevice, Arc<Mutex<FakeState>>) {
        let state = Arc::new(Mutex::new(FakeState::default()));
        let device = SimDevice::new(
            simulation_vbin(b"not launched"),
            Box::new(FakeTransport::new(Arc::clone(&state))),
            None,
        );
        (device, state)
    }

    #[test]
    fn protocol_uses_json_frame_zero_and_binary_populate_frame_one() {
        let (mut device, state) = fake_device();
        device.start_session().expect("start");
        device.write_register(0x34, 0x89ab_cdef).expect("reg");
        assert_eq!(
            device.read_register(0x34).expect("fetch scalar"),
            0x89ab_cdef
        );
        device.populate(HBM_BASE, &[0, 1, 255]).expect("populate");
        assert_eq!(
            device.fetch_buffer(HBM_BASE, 3).expect("fetch buffer"),
            vec![0, 1, 255]
        );
        device.exit_session().expect("exit");

        let state = state.lock().expect("fake state");
        let requests = &state.requests;
        assert_eq!(requests.len(), 6);
        assert_eq!(
            requests
                .iter()
                .map(|request| request.command["command"].as_str().expect("command"))
                .collect::<Vec<_>>(),
            vec!["start", "reg", "fetch", "populate", "fetch", "exit"]
        );
        assert_eq!(
            requests
                .iter()
                .map(|request| request.frames.len())
                .collect::<Vec<_>>(),
            vec![1, 1, 1, 2, 1, 1]
        );
        assert_eq!(requests[1].command["addr"], BASE + 0x34);
        assert_eq!(requests[1].command["val"], 0x89ab_cdef_u32);
        assert_eq!(requests[2].command["type"], "scalar");
        assert_eq!(requests[3].command["addr"], HBM_BASE);
        assert_eq!(requests[3].command["size"], 3);
        assert_eq!(requests[3].frames[1], vec![0, 1, 255]);
        assert_eq!(requests[4].command["type"], "buffer");
        assert!(requests.iter().all(|request| {
            serde_json::from_slice::<Value>(&request.frames[0]).is_ok()
                && !request.frames[0].ends_with(&[0])
        }));
    }

    #[test]
    fn device_lowers_scalars_and_buffer_addresses_to_absolute_words() {
        let (mut device, state) = fake_device();
        let mut input = [0_u8; 4];
        device
            .set_buffer_arg(0, input.as_mut_ptr(), input.len(), BufferAccess::ReadOnly)
            .expect("bind buffer");
        device
            .set_scalar_arg(1, &0x0123_4567_89ab_cdef_u64.to_le_bytes())
            .expect("bind scalar");
        device.exec().expect("exec");

        let state = state.lock().expect("fake state");
        let registers = state
            .requests
            .iter()
            .filter(|request| request.command["command"] == "reg")
            .map(|request| {
                (
                    request.command["addr"].as_u64().expect("address"),
                    request.command["val"].as_u64().expect("value"),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            registers,
            vec![
                (BASE + 0x10, u64::from(HBM_BASE as u32)),
                (BASE + 0x14, HBM_BASE >> 32),
                (BASE + 0x34, 0x89ab_cdef),
                (BASE + 0x38, 0x0123_4567),
                (BASE, 1),
            ]
        );
    }

    #[test]
    fn device_syncs_buffers_and_finishes_on_ctrl_done() {
        let (mut device, state) = fake_device();
        let mut host = [1_u8, 2, 3, 255];
        device
            .set_buffer_arg(0, host.as_mut_ptr(), host.len(), BufferAccess::ReadWrite)
            .expect("bind buffer");
        device
            .set_scalar_arg(1, &7_u32.to_le_bytes())
            .expect("bind scalar");
        device.write_to_device().expect("upload");
        assert_eq!(
            state.lock().expect("fake state").memory[&HBM_BASE],
            vec![1, 2, 3, 255]
        );
        device.exec().expect("exec");
        let mut fake = state.lock().expect("fake state");
        fake.registers.insert(BASE, 2);
        fake.memory.insert(HBM_BASE, vec![9, 8, 7, 6]);
        drop(fake);
        device.read_from_device().expect("finish and download");
        assert_eq!(host, [9, 8, 7, 6]);
        assert!(device.is_finished().expect("finished"));
        assert!(device
            .kill()
            .expect_err("kill is unsupported")
            .to_string()
            .contains("no kernel cancellation"));
        assert!(device
            .set_stream_arg(9, "unused")
            .expect_err("streams are unsupported")
            .to_string()
            .contains("host streams are unsupported"));
    }

    #[test]
    fn allocation_is_aligned_checked_and_nonoverlapping() {
        let mut allocators = AddressAllocators::new();
        assert_eq!(
            allocators.allocate(MemoryTarget::Hbm(0), 1).expect("HBM0"),
            HBM_BASE
        );
        assert_eq!(
            allocators.allocate(MemoryTarget::Hbm(1), 1).expect("HBM1"),
            HBM_BASE + 4096
        );
        assert_eq!(
            allocators.allocate(MemoryTarget::Ddr, 4097).expect("DDR"),
            DDR_BASE
        );
        assert_eq!(allocators.ddr.next, DDR_BASE + 8192);

        let mut full = AddressAllocators::new();
        assert_eq!(
            full.allocate(MemoryTarget::HbmVnoc, MEMORY_WINDOW_BYTES)
                .expect("exact HBM window"),
            HBM_BASE
        );
        assert!(full
            .allocate(MemoryTarget::Hbm(63), 1)
            .expect_err("HBM window is full")
            .to_string()
            .contains("128MiB"));
        assert!(full
            .allocate(MemoryTarget::Hbm(0), u64::MAX)
            .expect_err("size overflow")
            .to_string()
            .contains("overflow"));
        assert_eq!(
            full.allocate(MemoryTarget::Ddr, 1)
                .expect("independent DDR"),
            DDR_BASE
        );
    }

    #[test]
    fn process_receives_ipc_argument_and_is_exited_and_reaped() {
        let script = br#"#!/bin/sh
printf '%s\n' "$#" > argc.txt
printf '%s\n' "$1" > endpoint.txt
pwd > cwd.txt
: > ready
while [ ! -f exit.requested ]; do sleep 0.02; done
exit 0
"#;
        let vbin = simulation_vbin(script);
        let root = vbin.extraction_dir().to_path_buf();
        let model = root.join("xsim.dir/top_wrapper_behav/xsimk.so");
        let executable = validated_vpp_sim(&vbin).expect("validate vpp_sim");
        assert_ne!(
            fs::metadata(&executable)
                .expect("sim mode")
                .permissions()
                .mode()
                & 0o100,
            0
        );
        assert_eq!(
            fs::metadata(&model)
                .expect("model mode")
                .permissions()
                .mode()
                & 0o111,
            0
        );

        let endpoint = endpoint(&vbin).expect("endpoint");
        let process = SimProcess::spawn(&executable, &root, &endpoint).expect("spawn fake vpp_sim");
        let pid = process.child.as_ref().expect("child").id();
        let state = Arc::new(Mutex::new(FakeState::default()));
        let transport =
            FakeTransport::with_exit_marker(Arc::clone(&state), root.join("exit.requested"));
        let mut device = SimDevice::new(vbin, Box::new(transport), Some(process));
        device.session_active = true;

        for _ in 0..200 {
            if root.join("ready").exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(root.join("ready").exists(), "fake vpp_sim did not start");
        assert_eq!(
            fs::read_to_string(root.join("argc.txt"))
                .expect("argc")
                .trim(),
            "1"
        );
        assert_eq!(
            fs::read_to_string(root.join("endpoint.txt"))
                .expect("argv1")
                .trim(),
            endpoint
        );
        assert_eq!(
            Path::new(
                fs::read_to_string(root.join("cwd.txt"))
                    .expect("cwd")
                    .trim()
            )
            .canonicalize()
            .expect("canonical cwd"),
            root.canonicalize().expect("canonical extraction dir")
        );

        drop(device);
        assert!(state.lock().expect("fake state").closed);
        assert!(state
            .lock()
            .expect("fake state")
            .requests
            .iter()
            .any(|request| { request.command["command"] == "exit" }));
        // Drop waited and reaped the direct child rather than leaving a running
        // simulator or a zombie behind.
        let pid = i32::try_from(pid).expect("pid fits C int");
        // SAFETY: signal zero only probes the already-reaped child PID.
        let rc = unsafe { libc::kill(pid, 0) };
        assert_eq!(rc, -1, "simulator process {pid} still exists");
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        );
    }

    #[test]
    fn malformed_and_error_replies_are_rejected() {
        let error = super::expect_ok(b"ERR no model", "start").expect_err("ERR reply");
        assert!(error.to_string().contains("no model"));
        super::json_reply::<u32>(b"[]", "scalar fetch").expect_err("not a scalar");
        super::expect_ok(b"null", "reg").expect_err("not OK");
    }
}
