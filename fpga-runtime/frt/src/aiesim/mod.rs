//! A design's AI Engine graph, run in `x86simulator` or `aiesimulator` from
//! the package `tapa compile --aie-sim` builds, while the host program runs
//! every other task in software.
//!
//! The package holds the compiled graph (`Work/`) and `manifest.json`,
//! which `tapa-cli` writes (`steps/meta/aie_sim.rs`; the structs below read
//! that same contract). Each run gets a fresh directory beside the package's
//! `Work/`: `rtp.txt` holds the runtime parameters the graph's entry point
//! reads, and every PL <-> AI Engine stream (PLIO) is relayed live between
//! the host's shared-memory queue and the simulator's PLIO file. An input
//! goes through a named pipe the simulator reads as data arrives, padded to
//! a whole transfer once the host's producer has returned (the host then
//! closes the queue); an output is read as the simulator appends to it.

mod plio;

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use frt_shm::SharedMemoryQueue;
use serde::Deserialize;

use crate::device::{BufferAccess, Device, HostStream};
use crate::error::{FrtError, Result};

/// The file that makes a directory a package.
pub const MANIFEST: &str = "manifest.json";

#[derive(Debug, Deserialize)]
struct Manifest {
    simulator: Simulator,
    /// The top's scalar arguments by kernel argument index.
    scalars: Vec<ScalarArg>,
    /// The scalar behind each line of `rtp.txt`, in order.
    runtime_parameters: Vec<String>,
    plios: Vec<Plio>,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum Simulator {
    X86sim,
    Aiesim,
}

impl Simulator {
    const fn program(self) -> &'static str {
        match self {
            Self::X86sim => "x86simulator",
            Self::Aiesim => "aiesimulator",
        }
    }
}

/// Where a run's simulator writes its output PLIO files, under `data/`; its
/// input files are under `data/` of the run directory itself. Both are
/// passed explicitly: `aiesimulator` does not find them relative to its
/// working directory when `Work/` is a link.
const OUTPUT_DIR: &str = "out";

#[derive(Debug, Deserialize)]
struct ScalarArg {
    index: u32,
    name: String,
}

#[derive(Debug, Clone, Deserialize)]
struct Plio {
    index: u32,
    name: String,
    /// The PL writes it and the AI Engine reads it.
    input: bool,
    element_words: u32,
    beat_words: u32,
    transfer_words: u32,
}

/// What one relay moved.
#[derive(Debug, Default)]
struct Relayed {
    words: u64,
    beat_times_ns: Vec<f64>,
}

pub struct AieSimDevice {
    package: PathBuf,
    manifest: Manifest,
    scalars: BTreeMap<u32, u64>,
    streams: BTreeMap<u32, String>,
    state: State,
}

enum State {
    Idle,
    Running(Run),
    Finished { compute_ns: u64 },
}

struct Run {
    // Kept alive until the run is reaped: the simulator runs inside it.
    _dir: tempfile::TempDir,
    log: PathBuf,
    child: Child,
    started: Instant,
    exited: Arc<AtomicBool>,
    relays: Vec<(Plio, JoinHandle<Result<Relayed>>)>,
}

impl AieSimDevice {
    /// Whether `path` is a package.
    pub fn is_package(path: &Path) -> bool {
        path.join(MANIFEST).is_file()
    }

    pub fn open(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path.join(MANIFEST))?;
        let manifest: Manifest = serde_json::from_str(&text).map_err(|e| {
            FrtError::MetadataParse(format!("{}: {e}", path.join(MANIFEST).display()))
        })?;
        Ok(Self {
            package: std::path::absolute(path)?,
            manifest,
            scalars: BTreeMap::new(),
            streams: BTreeMap::new(),
            state: State::Idle,
        })
    }

    fn start(&self) -> Result<Run> {
        let dir = tempfile::Builder::new().prefix("tapa-aiesim-").tempdir()?;
        std::os::unix::fs::symlink(self.package.join("Work"), dir.path().join("Work"))?;
        std::fs::create_dir_all(dir.path().join("data"))?;
        let mut rtp = String::new();
        for name in &self.manifest.runtime_parameters {
            let index = self
                .manifest
                .scalars
                .iter()
                .find(|arg| &arg.name == name)
                .map(|arg| arg.index)
                .ok_or_else(|| FrtError::AieSim(format!("no scalar argument `{name}`")))?;
            let bits = self.scalars.get(&index).ok_or_else(|| {
                FrtError::AieSim(format!("the scalar argument `{name}` was never set"))
            })?;
            rtp.push_str(&bits.to_string());
            rtp.push('\n');
        }
        std::fs::write(dir.path().join("rtp.txt"), rtp)?;
        for plio in self.manifest.plios.iter().filter(|plio| plio.input) {
            make_fifo(&dir.path().join("data").join(format!("{}.txt", plio.name)))?;
        }

        let simulator = self.manifest.simulator;
        let log = dir.path().join("simulator.log");
        let out = File::create(&log)?;
        // The simulator signals its whole process group when it exits, so it
        // gets one of its own instead of the host program's.
        let child = Command::new(simulator.program())
            .arg("--pkg-dir=./Work")
            .arg(format!("--input-dir={}", dir.path().display()))
            .arg(format!(
                "--output-dir={}",
                dir.path().join(OUTPUT_DIR).display()
            ))
            .current_dir(dir.path())
            .stdin(Stdio::null())
            .stdout(out.try_clone()?)
            .stderr(out)
            .process_group(0)
            .spawn()
            .map_err(|e| {
                FrtError::AieSim(format!(
                    "cannot run `{}` ({e}); source the Vitis `settings64.sh` so it is on PATH",
                    simulator.program()
                ))
            })?;

        let exited = Arc::new(AtomicBool::new(false));
        let mut relays = Vec::new();
        for plio in &self.manifest.plios {
            let queue = self.streams.get(&plio.index).cloned().ok_or_else(|| {
                FrtError::AieSim(format!("the stream `{}` was never bound", plio.name))
            })?;
            let file = if plio.input {
                dir.path().join("data")
            } else {
                dir.path().join(OUTPUT_DIR).join("data")
            }
            .join(format!("{}.txt", plio.name));
            let (relay, exited) = (plio.clone(), Arc::clone(&exited));
            let handle = std::thread::Builder::new()
                .name(format!("aiesim-{}", plio.name))
                .spawn(move || {
                    block_sigpipe();
                    if relay.input {
                        relay_input(&file, &queue, &relay, &exited)
                    } else {
                        relay_output(&file, &queue, &relay, &exited)
                    }
                })?;
            relays.push((plio.clone(), handle));
        }
        Ok(Run {
            _dir: dir,
            log,
            child,
            started: Instant::now(),
            exited,
            relays,
        })
    }

    fn reap(&mut self, mut run: Run) -> Result<()> {
        let status = run.child.wait()?;
        run.exited.store(true, Ordering::Release);
        let compute_ns = crate::elapsed_ns(run.started);
        let mut failure = (!status.success()).then_some(FrtError::SimFailed(status));
        for (plio, handle) in run.relays {
            match handle.join() {
                Ok(Ok(relayed)) => {
                    if self.manifest.simulator == Simulator::Aiesim && !plio.input {
                        report_timing(&plio.name, &relayed);
                    }
                }
                Ok(Err(error)) => failure = failure.or(Some(error)),
                Err(_) => {
                    failure = failure.or_else(|| {
                        Some(FrtError::AieSim(format!(
                            "the relay of `{}` panicked",
                            plio.name
                        )))
                    });
                }
            }
        }
        self.state = State::Finished { compute_ns };
        if let Some(error) = failure {
            if let Ok(text) = std::fs::read_to_string(&run.log) {
                let lines: Vec<&str> = text.lines().collect();
                eprintln!("{}", lines[lines.len().saturating_sub(30)..].join("\n"));
            }
            return Err(error);
        }
        Ok(())
    }
}

impl Device for AieSimDevice {
    fn host_streams(&self) -> Vec<HostStream> {
        self.manifest
            .plios
            .iter()
            .map(|plio| HostStream {
                index: plio.index,
                name: plio.name.clone(),
                kernel_reads: plio.input,
            })
            .collect()
    }

    fn simulates_aie(&self) -> bool {
        true
    }

    fn set_scalar_arg(&mut self, index: u32, value: &[u8]) -> Result<()> {
        let mut bits = [0u8; 8];
        let len = value.len().min(8);
        bits[..len].copy_from_slice(&value[..len]);
        self.scalars.insert(index, u64::from_le_bytes(bits));
        Ok(())
    }

    // The AI Engine tasks take no memory; the PL tasks run in software on
    // the host's own buffers.
    fn set_buffer_arg(&mut self, _: u32, _: *mut u8, _: usize, _: BufferAccess) -> Result<()> {
        Ok(())
    }

    fn set_stream_arg(&mut self, index: u32, shm_path: &str) -> Result<()> {
        if !shm_path.is_empty() {
            self.streams.insert(index, shm_path.to_owned());
        }
        Ok(())
    }

    fn write_to_device(&mut self) -> Result<()> {
        Ok(())
    }

    fn read_from_device(&mut self) -> Result<()> {
        Ok(())
    }

    fn exec(&mut self) -> Result<()> {
        let run = self.start()?;
        self.state = State::Running(run);
        Ok(())
    }

    fn finish(&mut self) -> Result<()> {
        match std::mem::replace(&mut self.state, State::Finished { compute_ns: 0 }) {
            State::Running(run) => self.reap(run),
            State::Idle => Ok(()),
            finished @ State::Finished { .. } => {
                self.state = finished;
                Ok(())
            }
        }
    }

    fn kill(&mut self) -> Result<()> {
        if let State::Running(run) = &mut self.state {
            kill_group(&run.child);
        }
        self.finish().or(Ok(()))
    }

    fn is_finished(&mut self) -> Result<bool> {
        Ok(match &mut self.state {
            State::Idle => false,
            State::Running(run) => run.child.try_wait()?.is_some(),
            State::Finished { .. } => true,
        })
    }

    fn compute_ns(&self) -> u64 {
        match &self.state {
            State::Finished { compute_ns } => *compute_ns,
            State::Idle | State::Running(_) => 0,
        }
    }
}

impl Drop for AieSimDevice {
    // The simulator runs in its own process group, so nothing else ends it
    // when the host program stops early.
    fn drop(&mut self) {
        if let State::Running(run) = &mut self.state {
            kill_group(&run.child);
            let _ = run.child.wait();
            run.exited.store(true, Ordering::Release);
        }
    }
}

fn kill_group(child: &Child) {
    if let Ok(pid) = libc::pid_t::try_from(child.id()) {
        // SAFETY: signals the simulator's own process group.
        unsafe { libc::kill(-pid, libc::SIGKILL) };
    }
}

fn make_fifo(path: &Path) -> Result<()> {
    let c_path = std::ffi::CString::new(path.as_os_str().as_encoded_bytes())
        .map_err(|e| FrtError::AieSim(e.to_string()))?;
    // SAFETY: `c_path` is a valid NUL-terminated path.
    if unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

/// A write to a pipe the simulator has closed must fail with `EPIPE`, not
/// end the host program.
fn block_sigpipe() {
    let mut set = std::mem::MaybeUninit::<libc::sigset_t>::uninit();
    // SAFETY: `sigemptyset` initializes the set it is given.
    unsafe { libc::sigemptyset(set.as_mut_ptr()) };
    // SAFETY: `set` was initialized just above.
    let mut set = unsafe { set.assume_init() };
    // SAFETY: `set` is an initialized signal set.
    unsafe { libc::sigaddset(&raw mut set, libc::SIGPIPE) };
    // SAFETY: changes only the calling thread's signal mask.
    unsafe { libc::pthread_sigmask(libc::SIG_BLOCK, &raw const set, std::ptr::null_mut()) };
}

const POLL: Duration = Duration::from_micros(100);

/// A host queue slot holds one `elem_t<T>`: the element's data, then its
/// end-of-transmission flag byte, then any padding to the slot width. Both
/// of the host's encodings put the flag right after the data, which for an
/// AI Engine stream element is a whole number of 32-bit words.
const fn eot_offset(plio: &Plio) -> usize {
    plio.element_words as usize * 4
}

/// The element's data as little-endian 32-bit words.
fn element_words(slot: &[u8], plio: &Plio) -> Vec<u32> {
    slot[..eot_offset(plio)]
        .as_chunks::<4>()
        .0
        .iter()
        .map(|w| u32::from_le_bytes(*w))
        .collect()
}

/// Whether the queue's slots can hold the PLIO's elements and their flag.
fn check_width(queue: &SharedMemoryQueue, plio: &Plio) -> Result<()> {
    if queue.width() <= eot_offset(plio) {
        return Err(FrtError::AieSim(format!(
            "the host stream `{}` has {}-byte slots, too narrow for {}-word elements",
            plio.name,
            queue.width(),
            plio.element_words
        )));
    }
    Ok(())
}

/// Host queue -> named pipe, until the host closes the queue.
fn relay_input(file: &Path, queue: &str, plio: &Plio, exited: &AtomicBool) -> Result<Relayed> {
    let mut queue = SharedMemoryQueue::open(queue)?;
    check_width(&queue, plio)?;
    // The open fails with ENXIO until the simulator opens the pipe to read,
    // which it may never do if the graph ends first.
    let mut pipe = loop {
        match OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(file)
        {
            Ok(pipe) => break pipe,
            Err(e) if e.raw_os_error() == Some(libc::ENXIO) => {
                if exited.load(Ordering::Acquire) {
                    return Ok(Relayed::default());
                }
                std::thread::sleep(POLL);
            }
            Err(e) => return Err(e.into()),
        }
    };
    set_blocking(&pipe)?;
    let mut beats = plio::BeatWriter::new(plio.beat_words, plio.transfer_words);
    let mut element = vec![0u8; queue.width()];
    loop {
        if !queue.pop_into(&mut element) {
            if exited.load(Ordering::Acquire) {
                return Ok(Relayed {
                    words: beats.words,
                    ..Relayed::default()
                });
            }
            std::thread::sleep(POLL);
            continue;
        }
        if element[eot_offset(plio)] != 0 {
            break;
        }
        let lines = beats.push(&element_words(&element, plio));
        if write_lines(&mut pipe, &lines)?.is_none() {
            return Ok(Relayed::default());
        }
    }
    let rest = beats.finish();
    write_lines(&mut pipe, &rest)?;
    Ok(Relayed {
        words: beats.words,
        ..Relayed::default()
    })
}

/// `Some(())` once written, `None` when the simulator closed the pipe.
fn write_lines(pipe: &mut File, lines: &str) -> Result<Option<()>> {
    match pipe.write_all(lines.as_bytes()) {
        Ok(()) => Ok(Some(())),
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn set_blocking(file: &File) -> Result<()> {
    use std::os::fd::AsRawFd;
    let fd = file.as_raw_fd();
    // SAFETY: `fd` is an open descriptor owned by `file`.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    // SAFETY: as above; only the status flags change.
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags & !libc::O_NONBLOCK) } < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

/// Simulator output file -> host queue, until the simulator has exited and
/// the file is read to its end. Words short of a whole element at the end
/// are the zero padding of the final beat and are dropped.
fn relay_output(file: &Path, queue: &str, plio: &Plio, exited: &AtomicBool) -> Result<Relayed> {
    let mut queue = SharedMemoryQueue::open(queue)?;
    check_width(&queue, plio)?;
    let width = queue.width();
    let per_element = plio.element_words as usize;
    let mut parser = plio::LineParser::default();
    let (mut words, mut moved) = (Vec::new(), 0u64);
    let mut input: Option<File> = None;
    let mut chunk = vec![0u8; 1 << 16];
    loop {
        let done = exited.load(Ordering::Acquire);
        if input.is_none() {
            input = File::open(file).ok();
        }
        let read = match &mut input {
            Some(f) => f.read(&mut chunk)?,
            None => 0,
        };
        if read == 0 {
            if done {
                break;
            }
            std::thread::sleep(POLL);
            continue;
        }
        parser.feed(&chunk[..read], &mut words);
        let whole = words.len() / per_element * per_element;
        for value in words[..whole].chunks(per_element) {
            // The flag and the padding stay clear.
            let mut element: Vec<u8> = value.iter().flat_map(|w| w.to_le_bytes()).collect();
            element.resize(width, 0);
            while queue.try_push(&element).is_err() {
                std::thread::sleep(POLL);
            }
            moved += per_element as u64;
        }
        words.drain(..whole);
    }
    Ok(Relayed {
        words: moved,
        beat_times_ns: parser.beat_times_ns,
    })
}

fn report_timing(name: &str, relayed: &Relayed) {
    let times = &relayed.beat_times_ns;
    let (Some(first), Some(last)) = (times.first(), times.last()) else {
        println!("{name}: {} words, no timed beats", relayed.words);
        return;
    };
    // A lone beat has no interval to average.
    let gaps = u32::try_from(times.len() - 1).unwrap_or(u32::MAX);
    let spacing = if gaps > 0 {
        format!(" ({:.2} ns apart)", (last - first) / f64::from(gaps))
    } else {
        String::new()
    };
    println!(
        "{name}: {} words, beats at {first:.1}..{last:.1} ns{spacing}",
        relayed.words
    );
}
