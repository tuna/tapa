//! A design's AI Engine graph, run in `x86simulator` or `aiesimulator` from
//! the package `tapa compile --aie-sim` builds, beside the host program.
//!
//! The package holds the compiled graph (`Work/`) and `manifest.json`,
//! which `tapa-cli` writes (`steps/meta/aie_sim.rs`; the structs below read
//! that same contract). Each run gets a fresh directory beside the package's
//! `Work/`: `rtp.txt` holds the runtime parameters the graph's entry point
//! reads, and each PL <-> AI Engine stream (PLIO) has a file there.
//!
//! `x86simulator` runs live: the host runs every task off the graph's side
//! in software, and each stream is relayed between the host's
//! shared-memory queue and the simulator's file. A top with no PL task
//! moves its stream arguments over GMIO instead of PLIO; the graph's entry
//! point then moves their frames between the files and the graph. An input goes through a
//! named pipe the simulator reads as data arrives, padded to a whole
//! transfer once the host's producer has returned (the host then closes
//! the queue); an output is read as the simulator appends to it.
//!
//! `aiesimulator` reads each input once, to its end, when it starts, so it
//! replays: the host runs the whole design in software and binds a copy of
//! each stream, closed once the software run ends. The copies of the inputs
//! become the simulator's input files, and each output it writes must match
//! the copy of what software simulation pushed on that stream; each output
//! is printed with the simulated time of its beats. A GMIO output also
//! tells the entry point how many frames software simulation wrote.

mod plio;

use std::collections::BTreeMap;
use std::fmt::Write as _;
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

use crate::device::{AieSimulation, BufferAccess, Device, HostStream};
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
    /// The GMIOs of a PL-less top: its own stream arguments.
    #[serde(default)]
    gmios: Vec<Plio>,
    /// The upper task instances the graph was built with inlined.
    inlined: Vec<tapa_ir::InlinedInstance>,
    /// The AI Engine tasks' `async_mmap` ports, served over PLIOs.
    #[serde(default)]
    memories: Vec<tapa_ir::MemoryCrossing>,
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

    /// `x86simulator` reads a named pipe as data arrives; `aiesimulator`
    /// reads each input once, to its end, when it starts, so it can only
    /// replay a finished software run.
    const fn mode(self) -> AieSimulation {
        match self {
            Self::X86sim => AieSimulation::Live,
            Self::Aiesim => AieSimulation::Replay,
        }
    }
}

/// Where a run's simulator writes its output PLIO files, under `data/`; its
/// input files are under `data/` of the run directory itself. Both are
/// passed explicitly: `aiesimulator` does not find them relative to its
/// working directory when `Work/` is a link.
const OUTPUT_DIR: &str = "out";

/// The file of a replay's expected frame count per output GMIO, one line
/// each in manifest order, which the graph's entry point reads
/// (`tapa-xilinx`'s `GMIO_FRAMES_FILE`).
const GMIO_FRAMES_FILE: &str = "frames.txt";

#[derive(Debug, Deserialize)]
struct ScalarArg {
    index: u32,
    name: String,
}

/// A stream between the host and the graph: a PLIO, or a GMIO whose
/// beat and transfer are a frame.
#[derive(Debug, Clone, Deserialize)]
struct Plio {
    index: u32,
    name: String,
    /// The host side writes it and the AI Engine reads it.
    input: bool,
    element_words: u32,
    beat_words: u32,
    transfer_words: u32,
    /// Listed among the GMIOs; its file is the entry point's, under `data/`
    /// either way.
    #[serde(skip)]
    gmio: bool,
}

pub struct AieSimDevice {
    package: PathBuf,
    manifest: Manifest,
    scalars: BTreeMap<u32, u64>,
    streams: BTreeMap<u32, String>,
    state: State,
}

/// A PLIO and the words pushed on it.
type Recorded = (Plio, Vec<u32>);

enum State {
    Idle,
    /// The simulator runs while every stream is relayed.
    Live(Run),
    /// The host's software run is still pushing on the streams.
    Recording {
        stop: Arc<AtomicBool>,
        copies: Vec<(Plio, JoinHandle<Result<Vec<u32>>>)>,
    },
    /// The simulator replays the recorded inputs; the recorded outputs are
    /// what it must write back.
    Replaying {
        run: Run,
        expected: Vec<Recorded>,
    },
    Finished {
        compute_ns: u64,
    },
}

struct Run {
    // The simulator runs inside it; kept until the run is reaped.
    dir: tempfile::TempDir,
    log: PathBuf,
    child: Child,
    started: Instant,
    exited: Arc<AtomicBool>,
    relays: Vec<(Plio, JoinHandle<Result<()>>)>,
}

impl AieSimDevice {
    /// Whether `path` is a package.
    pub fn is_package(path: &Path) -> bool {
        path.join(MANIFEST).is_file()
    }

    pub fn open(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path.join(MANIFEST))?;
        let mut manifest: Manifest = serde_json::from_str(&text).map_err(|e| {
            FrtError::MetadataParse(format!("{}: {e}", path.join(MANIFEST).display()))
        })?;
        for gmio in &mut manifest.gmios {
            gmio.gmio = true;
        }
        Ok(Self {
            package: std::path::absolute(path)?,
            manifest,
            scalars: BTreeMap::new(),
            streams: BTreeMap::new(),
            state: State::Idle,
        })
    }

    fn queue(&self, plio: &Plio) -> Result<String> {
        self.streams
            .get(&plio.index)
            .cloned()
            .ok_or_else(|| FrtError::AieSim(format!("the stream `{}` was never bound", plio.name)))
    }

    /// A fresh run directory beside the package's graph, holding the
    /// runtime parameters.
    fn run_dir(&self) -> Result<tempfile::TempDir> {
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
        Ok(dir)
    }

    fn spawn(&self, dir: tempfile::TempDir) -> Result<Run> {
        let simulator = self.manifest.simulator;
        let log = dir.path().join("simulator.log");
        let out = File::create(&log)?;
        // The simulator signals its whole process group when it exits, so it
        // gets one of its own instead of the host program's.
        let mut command = Command::new(simulator.program());
        // x86simulator stops a graph waiting on a GMIO no transfer is posted
        // to, as a deadlock; the graph's entry point posts each input frame
        // from a thread as it arrives, so such waits are not deadlocks.
        if simulator == Simulator::X86sim && !self.manifest.gmios.is_empty() {
            command.arg("--disable-stop-on-deadlock");
        }
        let child = command
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
        Ok(Run {
            dir,
            log,
            child,
            started: Instant::now(),
            exited: Arc::new(AtomicBool::new(false)),
            relays: Vec::new(),
        })
    }

    /// Every stream the host relays or records.
    fn streams(&self) -> impl Iterator<Item = &Plio> {
        self.manifest.plios.iter().chain(&self.manifest.gmios)
    }

    fn start_live(&self) -> Result<Run> {
        let dir = self.run_dir()?;
        for plio in self.streams().filter(|plio| plio.input) {
            make_fifo(&plio_file(dir.path(), plio))?;
        }
        let mut run = self.spawn(dir)?;
        for plio in self.streams() {
            let queue = self.queue(plio)?;
            let file = plio_file(run.dir.path(), plio);
            let (relay, exited) = (plio.clone(), Arc::clone(&run.exited));
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
            run.relays.push((plio.clone(), handle));
        }
        Ok(run)
    }

    fn start_recording(&self) -> Result<State> {
        let stop = Arc::new(AtomicBool::new(false));
        let mut copies = Vec::new();
        for plio in self.streams() {
            let queue = self.queue(plio)?;
            let (copy, stop) = (plio.clone(), Arc::clone(&stop));
            let handle = std::thread::Builder::new()
                .name(format!("aiesim-{}", plio.name))
                .spawn(move || record(&queue, &copy, &stop))?;
            copies.push((plio.clone(), handle));
        }
        Ok(State::Recording { stop, copies })
    }

    fn start_replay(&self, recorded: Vec<Recorded>) -> Result<State> {
        let dir = self.run_dir()?;
        let mut expected = Vec::new();
        let mut frames = String::new();
        for (plio, words) in recorded {
            if plio.input {
                let mut beats = plio::BeatWriter::new(plio.beat_words, plio.transfer_words);
                let mut text = beats.push(&words);
                text.push_str(&beats.finish());
                std::fs::write(plio_file(dir.path(), &plio), text)?;
            } else {
                if plio.gmio {
                    let frame = (plio.transfer_words as usize).max(1);
                    let _ = writeln!(frames, "{}", words.len().div_ceil(frame));
                }
                expected.push((plio, words));
            }
        }
        std::fs::write(dir.path().join(GMIO_FRAMES_FILE), frames)?;
        Ok(State::Replaying {
            run: self.spawn(dir)?,
            expected,
        })
    }

    fn reap(&mut self, mut run: Run, expected: &[Recorded]) -> Result<()> {
        let status = run.child.wait()?;
        run.exited.store(true, Ordering::Release);
        self.state = State::Finished {
            compute_ns: crate::elapsed_ns(run.started),
        };
        let mut failure = (!status.success()).then_some(FrtError::SimFailed(status));
        for (plio, handle) in run.relays {
            let error = match handle.join() {
                Ok(result) => result.err(),
                Err(_) => Some(FrtError::AieSim(format!(
                    "the relay of `{}` panicked",
                    plio.name
                ))),
            };
            failure = failure.or(error);
        }
        if failure.is_none() && !expected.is_empty() {
            failure = check_outputs(run.dir.path(), expected).err();
        }
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

/// Where a stream's file is in a run directory: the simulator writes a
/// PLIO output under `out/`, and everything else is under `data/`.
fn plio_file(dir: &Path, plio: &Plio) -> PathBuf {
    if plio.input || plio.gmio {
        dir.join("data")
    } else {
        dir.join(OUTPUT_DIR).join("data")
    }
    .join(format!("{}.txt", plio.name))
}

/// The recorded copies once the host has closed every one.
fn join_copies(copies: Vec<(Plio, JoinHandle<Result<Vec<u32>>>)>) -> Result<Vec<Recorded>> {
    copies
        .into_iter()
        .map(|(plio, handle)| {
            let words = handle.join().map_err(|_panic| {
                FrtError::AieSim(format!("the copy of `{}` panicked", plio.name))
            })??;
            Ok((plio, words))
        })
        .collect()
}

/// Compares each output the simulator wrote with what software simulation
/// pushed on it, printing one line per output with its timing.
fn check_outputs(dir: &Path, expected: &[Recorded]) -> Result<()> {
    let mut differing = Vec::new();
    for (plio, want) in expected {
        let mut parser = plio::LineParser::default();
        let mut got = Vec::new();
        // An output the graph never wrote to has no file.
        if let Ok(text) = std::fs::read(plio_file(dir, plio)) {
            parser.feed(&text, &mut got);
            parser.feed(b"\n", &mut got);
        }
        let mut line = format!("{}: {} of {} words", plio.name, got.len(), want.len());
        line.push_str(&plio::timing(&parser.beat_times_ns));
        if let Some(index) = plio::first_mismatch(&got, want, plio.transfer_words as usize) {
            differing.push(plio.name.clone());
            let _ = match (got.get(index), want.get(index)) {
                (Some(got), Some(want)) => write!(
                    line,
                    ", first mismatch at word {index}: got {got}, expected {want}"
                ),
                _ => write!(line, ", unexpected data at word {index}"),
            };
        }
        println!("{line}");
    }
    if differing.is_empty() {
        println!("AI Engine simulation matches software simulation");
        Ok(())
    } else {
        Err(FrtError::AieSim(format!(
            "AI Engine simulation differs from software simulation on {}",
            differing.join(", ")
        )))
    }
}

impl Device for AieSimDevice {
    fn host_streams(&self) -> Vec<HostStream> {
        self.streams()
            .map(|plio| HostStream {
                index: plio.index,
                name: plio.name.clone(),
                kernel_reads: plio.input,
                ring: None,
            })
            .collect()
    }

    fn inlined_instances(&self) -> Vec<tapa_ir::InlinedInstance> {
        self.manifest.inlined.clone()
    }
    fn memory_crossings(&self) -> Vec<tapa_ir::MemoryCrossing> {
        self.manifest.memories.clone()
    }

    fn aie_simulation(&self) -> Option<AieSimulation> {
        Some(self.manifest.simulator.mode())
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
        self.state = match self.manifest.simulator.mode() {
            AieSimulation::Live => State::Live(self.start_live()?),
            AieSimulation::Replay => self.start_recording()?,
        };
        Ok(())
    }

    fn finish(&mut self) -> Result<()> {
        match std::mem::replace(&mut self.state, State::Finished { compute_ns: 0 }) {
            State::Live(run) => self.reap(run, &[]),
            State::Recording { copies, .. } => {
                let replay = self.start_replay(join_copies(copies)?)?;
                self.state = replay;
                self.finish()
            }
            State::Replaying { run, expected } => self.reap(run, &expected),
            State::Idle => Ok(()),
            finished @ State::Finished { .. } => {
                self.state = finished;
                Ok(())
            }
        }
    }

    fn kill(&mut self) -> Result<()> {
        match &mut self.state {
            State::Live(run) | State::Replaying { run, .. } => kill_group(&run.child),
            State::Recording { stop, .. } => {
                stop.store(true, Ordering::Release);
                self.state = State::Finished { compute_ns: 0 };
            }
            State::Idle | State::Finished { .. } => {}
        }
        self.finish().or(Ok(()))
    }

    // A replay starts its simulator once the host has closed every copy.
    fn is_finished(&mut self) -> Result<bool> {
        if let State::Recording { copies, .. } = &self.state {
            if copies.iter().all(|(_, handle)| handle.is_finished()) {
                let State::Recording { copies, .. } =
                    std::mem::replace(&mut self.state, State::Finished { compute_ns: 0 })
                else {
                    unreachable!("matched just above")
                };
                self.state = self.start_replay(join_copies(copies)?)?;
            }
        }
        Ok(match &mut self.state {
            State::Idle | State::Recording { .. } => false,
            State::Live(run) | State::Replaying { run, .. } => run.child.try_wait()?.is_some(),
            State::Finished { .. } => true,
        })
    }

    fn compute_ns(&self) -> u64 {
        match &self.state {
            State::Finished { compute_ns } => *compute_ns,
            State::Idle | State::Live(_) | State::Recording { .. } | State::Replaying { .. } => 0,
        }
    }
}

impl Drop for AieSimDevice {
    // The simulator runs in its own process group, so nothing else ends it
    // when the host program stops early.
    fn drop(&mut self) {
        match &mut self.state {
            State::Live(run) | State::Replaying { run, .. } => {
                kill_group(&run.child);
                let _ = run.child.wait();
                run.exited.store(true, Ordering::Release);
            }
            State::Recording { stop, .. } => stop.store(true, Ordering::Release),
            State::Idle | State::Finished { .. } => {}
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
fn relay_input(file: &Path, queue: &str, plio: &Plio, exited: &AtomicBool) -> Result<()> {
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
                    return Ok(());
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
                return Ok(());
            }
            std::thread::sleep(POLL);
            continue;
        }
        if element[eot_offset(plio)] != 0 {
            break;
        }
        let lines = beats.push(&element_words(&element, plio));
        if write_lines(&mut pipe, &lines)?.is_none() {
            return Ok(());
        }
    }
    let rest = beats.finish();
    write_lines(&mut pipe, &rest)?;
    Ok(())
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
fn relay_output(file: &Path, queue: &str, plio: &Plio, exited: &AtomicBool) -> Result<()> {
    let mut queue = SharedMemoryQueue::open(queue)?;
    check_width(&queue, plio)?;
    let width = queue.width();
    let per_element = plio.element_words as usize;
    let mut parser = plio::LineParser::default();
    let mut words = Vec::new();
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
        }
        words.drain(..whole);
    }
    Ok(())
}

/// The host's copy of a stream -> words, until the host closes the copy.
fn record(queue: &str, plio: &Plio, stop: &AtomicBool) -> Result<Vec<u32>> {
    let mut queue = SharedMemoryQueue::open(queue)?;
    check_width(&queue, plio)?;
    let mut element = vec![0u8; queue.width()];
    let mut words = Vec::new();
    loop {
        if !queue.pop_into(&mut element) {
            if stop.load(Ordering::Acquire) {
                return Ok(words);
            }
            std::thread::sleep(POLL);
            continue;
        }
        if element[eot_offset(plio)] != 0 {
            return Ok(words);
        }
        words.extend(element_words(&element, plio));
    }
}
