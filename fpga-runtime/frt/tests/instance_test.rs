use frt::{Instance, Simulator};
use frt_shm::SharedMemoryQueue;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use zip::write::FileOptions;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("frt crate has parent")
        .to_path_buf()
}

fn ensure_verilator() -> bool {
    if let Some(bin) = std::env::var_os("VERILATOR_BIN") {
        // env_remove: Verilator's Perl frontend consumes VERILATOR_BIN to
        // pick its backend; pointed at a wrapper it re-execs itself forever.
        let status = Command::new(&bin)
            .arg("--version")
            .env_remove("VERILATOR_BIN")
            .status()
            .unwrap_or_else(|error| {
                panic!("failed to run VERILATOR_BIN={}: {error}", bin.display())
            });
        assert!(
            status.success(),
            "VERILATOR_BIN={} exited with {status}",
            bin.display()
        );
        return true;
    }
    Command::new("verilator")
        .arg("--version")
        .status()
        .is_ok_and(|s| s.success())
}

fn dpi_library_name() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "libfrt_dpi_verilator.dylib"
    }
    #[cfg(not(target_os = "macos"))]
    {
        "libfrt_dpi_verilator.so"
    }
}

fn ensure_dpi_lib_built() {
    static READY: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    READY.get_or_init(|| {
        // The runtime searches for the library starting from this test
        // executable, so build into the target dir the executable actually
        // runs from (`<target-root>/debug/deps/<test>`). Deriving it from
        // the env or a default breaks under harnesses that relocate the
        // target dir by flag, e.g. `cargo llvm-cov --target-dir`.
        let exe = std::env::current_exe().expect("test executable path");
        let target_root = exe
            .ancestors()
            .nth(3)
            .expect("test executable under <target-root>/debug/deps");
        let target_lib = target_root.join("debug").join(dpi_library_name());
        if target_lib.exists() {
            return;
        }
        let status = Command::new("cargo")
            .args(["build", "-p", "frt-dpi-verilator", "--target-dir"])
            .arg(target_root)
            .current_dir(workspace_root())
            .status()
            .expect("spawn cargo build for frt-dpi-verilator");
        assert!(status.success(), "failed to build frt-dpi-verilator");
        assert!(
            target_lib.exists(),
            "frt-dpi-verilator built but {} is missing",
            target_lib.display()
        );
    });
}

fn make_manual_zip() -> PathBuf {
    let tmp = tempfile::tempdir().expect("tempdir");
    let zip_path = tmp.path().join("manual_kernel.zip");
    let file = File::create(&zip_path).expect("create zip");
    let mut zip = zip::ZipWriter::new(file);
    let opts: FileOptions<'_, ()> = FileOptions::default();

    zip.start_file("tapa.json", opts).expect("add state");
    zip.write_all(include_bytes!("fixtures/manual_zip/tapa.json"))
        .expect("write state");

    zip.start_file("rtl/manual_hls_top.v", opts)
        .expect("add rtl");
    zip.write_all(include_bytes!("fixtures/manual_zip/rtl/manual_hls_top.v"))
        .expect("write rtl");

    zip.finish().expect("finish zip");

    // Persist by moving into workspace target tmp area.
    persist_test_artifact(&zip_path, "manual_kernel", "zip")
}

fn make_manual_xo() -> PathBuf {
    let tmp = tempfile::tempdir().expect("tempdir");
    let xo_path = tmp.path().join("manual_kernel.xo");
    let file = File::create(&xo_path).expect("create xo");
    let mut zip = zip::ZipWriter::new(file);
    let opts: FileOptions<'_, ()> = FileOptions::default();

    zip.start_file("kernel.xml", opts).expect("add kernel xml");
    zip.write_all(include_bytes!("fixtures/manual_xo/kernel.xml"))
        .expect("write kernel xml");

    zip.start_file("s_axi_control.v", opts)
        .expect("add s_axi_control");
    zip.write_all(include_bytes!("fixtures/manual_xo/s_axi_control.v"))
        .expect("write s_axi_control");

    zip.start_file("rtl/manual_vitis_top.v", opts)
        .expect("add rtl");
    zip.write_all(include_bytes!("fixtures/manual_xo/rtl/manual_vitis_top.v"))
        .expect("write rtl");

    zip.finish().expect("finish xo");
    persist_test_artifact(&xo_path, "manual_kernel", "xo")
}

fn persist_test_artifact(src: &Path, stem: &str, ext: &str) -> PathBuf {
    let persist = workspace_root()
        .join("target")
        .join("frt-tests")
        .join(format!(
            "{}_{}_{}.{}",
            stem,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
            ext
        ));
    std::fs::create_dir_all(
        persist
            .parent()
            .expect("persisted test artifact has parent directory"),
    )
    .expect("create persisted test artifact directory");
    std::fs::copy(src, &persist).expect("copy test artifact to persisted location");
    persist
}

fn pack_stream_elem_u32(value: u32, eot: bool) -> [u8; 5] {
    let mut bytes = [0u8; 5];
    bytes[..4].copy_from_slice(&value.to_le_bytes());
    bytes[4] = u8::from(eot);
    bytes
}

#[test]
fn open_cosim_verilator_zip_mmap_stream_roundtrip() {
    if !ensure_verilator() {
        eprintln!("skipping: verilator not found in PATH");
        return;
    }
    ensure_dpi_lib_built();

    let zip_path = make_manual_zip();
    let mut instance =
        Instance::open(&zip_path, &Simulator::Verilator).expect("open cosim instance");

    let mut mmap_word = [10u32];
    instance
        .set_buffer_arg_raw(
            0,
            mmap_word.as_mut_ptr().cast::<u8>(),
            std::mem::size_of_val(&mmap_word),
        )
        .expect("set mmap arg");

    let stream_name = format!("manual_stream_{}_{}", std::process::id(), mmap_word[0]);
    let mut stream_q =
        SharedMemoryQueue::create(&stream_name, 16, 5).expect("create stream shm queue");
    stream_q
        .try_push(&pack_stream_elem_u32(7, false))
        .expect("push stream word");
    let stream_path = stream_q.path().to_string_lossy().to_string();
    instance
        .set_stream_arg_raw(1, &stream_path)
        .expect("set stream shm path");

    instance.write_to_device().expect("write_to_device");
    instance.exec().expect("exec");
    instance.read_from_device().expect("read_from_device");
    instance.finish().expect("finish");

    assert_eq!(mmap_word[0], 17);
}

#[test]
fn open_cosim_verilator_xo_mmap_axis_roundtrip() {
    if !ensure_verilator() {
        eprintln!("skipping: verilator not found in PATH");
        return;
    }
    ensure_dpi_lib_built();

    let xo_path = make_manual_xo();
    let mut instance =
        Instance::open(&xo_path, &Simulator::Verilator).expect("open cosim instance");

    let mut mmap_word = [10u32];
    instance
        .set_buffer_arg_raw(
            0,
            mmap_word.as_mut_ptr().cast::<u8>(),
            std::mem::size_of_val(&mmap_word),
        )
        .expect("set mmap arg");
    instance
        .set_scalar_arg_bytes(2, &2u64.to_le_bytes())
        .expect("set scalar arg");

    let stream_name = format!("manual_axis_{}_{}", std::process::id(), mmap_word[0]);
    let mut stream_q =
        SharedMemoryQueue::create(&stream_name, 16, 5).expect("create stream shm queue");
    stream_q
        .try_push(&pack_stream_elem_u32(7, false))
        .expect("push stream word");
    let stream_path = stream_q.path().to_string_lossy().to_string();
    instance
        .set_stream_arg_raw(1, &stream_path)
        .expect("set stream shm path");

    instance.write_to_device().expect("write_to_device");
    instance.exec().expect("exec");
    instance.read_from_device().expect("read_from_device");
    instance.finish().expect("finish");

    assert_eq!(mmap_word[0], 19);
}
