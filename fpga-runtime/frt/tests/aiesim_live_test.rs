//! A live AI Engine simulation against a stand-in `x86simulator`.
//!
//! Its own test binary: the stand-in is found on `PATH`, which the test
//! changes for the whole process.

use std::os::unix::fs::PermissionsExt;
use std::time::{Duration, Instant};

use frt::Simulator;
use frt_shm::SharedMemoryQueue;

/// Writes 100 words to the output PLIO `o`, then runs until killed.
const SIMULATOR: &str = "#!/bin/sh
for arg in \"$@\"; do
  case $arg in --output-dir=*) out=${arg#--output-dir=};; esac
done
mkdir -p \"$out/data\"
seq 1 100 > \"$out/data/o.txt\"
exec sleep 60
";

const MANIFEST: &str = r#"{"simulator": "x86sim", "scalars": [],
  "runtime_parameters": [], "inlined": [],
  "plios": [{"index": 0, "name": "o", "input": false,
             "element_words": 1, "beat_words": 1, "transfer_words": 1}]}"#;

/// The host may stop reading an output (it is being interrupted): killing
/// the run must still return rather than wait for room in the host's queue.
#[test]
fn kill_returns_while_the_host_reads_no_more_output() {
    let root = tempfile::tempdir().expect("temp dir");
    let bin = root.path().join("bin");
    std::fs::create_dir_all(&bin).expect("bin dir");
    let simulator = bin.join("x86simulator");
    std::fs::write(&simulator, SIMULATOR).expect("write simulator");
    std::fs::set_permissions(&simulator, std::fs::Permissions::from_mode(0o755))
        .expect("make simulator executable");
    let package = root.path().join("package");
    std::fs::create_dir_all(package.join("Work")).expect("package dir");
    std::fs::write(package.join("manifest.json"), MANIFEST).expect("write manifest");
    let path = std::env::var_os("PATH").unwrap_or_default();
    let mut paths = vec![bin];
    paths.extend(std::env::split_paths(&path));
    std::env::set_var("PATH", std::env::join_paths(paths).expect("PATH"));

    // One slot: the relay fills it and then waits for the host.
    let queue = SharedMemoryQueue::create("aiesim_live_out", 1, 5).expect("queue");
    let mut device = frt::open(&package, &Simulator::Xsim { legacy: false }).expect("open");
    device
        .set_stream_arg(0, &queue.path().to_string_lossy())
        .expect("bind output");
    device.exec().expect("exec");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !queue.is_full() {
        assert!(
            Instant::now() < deadline,
            "the output never reached the host"
        );
        std::thread::sleep(Duration::from_millis(10));
    }

    let (killed, result) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = killed.send(device.kill().map_err(|error| error.to_string()));
    });
    result
        .recv_timeout(Duration::from_secs(10))
        .expect("kill must return though the host takes no more output")
        .expect("kill");
}
