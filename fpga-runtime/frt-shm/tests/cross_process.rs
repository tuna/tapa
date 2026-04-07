use frt_shm::MmapSegment;
use std::path::PathBuf;

const CHILD_TEST_NAME: &str = "child_shm_reader";
const ENV_PATH: &str = "FRT_SHM_CHILD_PATH";
const ENV_SIZE: &str = "FRT_SHM_CHILD_SIZE";
const ENV_OUT: &str = "FRT_SHM_CHILD_OUT";

#[test]
fn cross_process_read_write() {
    let mut seg = MmapSegment::create("xproc_test", 8).expect("create shm segment");
    let path = seg.path().to_str().expect("utf8 path").to_owned();
    let size = seg.len();
    seg.as_mut_slice()[..5].copy_from_slice(b"xproc");

    let out_file = std::env::temp_dir().join(format!(
        "frt-shm-xproc-{}-{}.bin",
        std::process::id(),
        std::thread::current().name().unwrap_or("t")
    ));
    let _ = std::fs::remove_file(&out_file);

    let out = std::process::Command::new(std::env::current_exe().expect("current_exe"))
        .args(["--ignored", "--exact", CHILD_TEST_NAME])
        .env(ENV_PATH, &path)
        .env(ENV_SIZE, size.to_string())
        .env(ENV_OUT, out_file.to_string_lossy().to_string())
        .output()
        .expect("spawn child");

    assert!(
        out.status.success(),
        "child failed: stdout=\n{}\nstderr=\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let got = std::fs::read(&out_file).expect("read child output");
    assert_eq!(got, b"xproc");
    let _ = std::fs::remove_file(out_file);
}

#[test]
#[ignore]
fn child_shm_reader() {
    let Some(path) = std::env::var_os(ENV_PATH) else {
        return;
    };
    let Some(size_raw) = std::env::var_os(ENV_SIZE) else {
        return;
    };
    let Some(out_path) = std::env::var_os(ENV_OUT) else {
        return;
    };

    let size = size_raw
        .to_string_lossy()
        .parse::<usize>()
        .expect("parse size");
    let seg = MmapSegment::open(&path.to_string_lossy(), size).expect("open segment");
    let bytes = &seg.as_slice()[..5];
    std::fs::write(PathBuf::from(out_path), bytes).expect("write child output");
}
