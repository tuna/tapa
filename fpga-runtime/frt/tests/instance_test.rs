use frt::{BufferAccess, Simulator};
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

fn dpi_library_name(package: &str) -> String {
    #[cfg(target_os = "macos")]
    {
        format!("lib{}.dylib", package.replace('-', "_"))
    }
    #[cfg(not(target_os = "macos"))]
    {
        format!("lib{}.so", package.replace('-', "_"))
    }
}

fn ensure_dpi_lib_built(package: &str) -> PathBuf {
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
    let target_lib = target_root.join("debug").join(dpi_library_name(package));
    if target_lib.exists() {
        return target_lib;
    }
    let status = Command::new("cargo")
        .args(["build", "-p", package, "--target-dir"])
        .arg(target_root)
        .current_dir(workspace_root())
        .status()
        .expect("spawn cargo build for DPI library");
    assert!(status.success(), "failed to build {package}");
    assert!(
        target_lib.exists(),
        "{package} built but {} is missing",
        target_lib.display()
    );
    target_lib
}

fn make_manual_zip() -> PathBuf {
    let tmp = tempfile::tempdir().expect("tempdir");
    let zip_path = tmp.path().join("manual_kernel.zip");
    let file = File::create(&zip_path).expect("create zip");
    let mut zip = zip::ZipWriter::new(file);
    let opts: FileOptions<'_, '_, ()> = FileOptions::default();

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
    let opts: FileOptions<'_, '_, ()> = FileOptions::default();

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

fn check_manual_stream_output(queue: &mut SharedMemoryQueue) {
    // u<17> occupies four host bytes, then EOS and three bytes of object padding.
    let mut token = [0xff; 8];
    assert!(queue.pop_into(&mut token), "missing 17-bit payload");
    assert_eq!(
        token,
        [0x45, 0x23, 0x01, 0, 0, 0, 0, 0],
        "17-bit payload must preserve data and zero host padding"
    );
    assert!(queue.pop_into(&mut token), "missing 17-bit EoT");
    assert_eq!(
        token,
        [0, 0, 0, 0, 1, 0, 0, 0],
        "EoT must occupy the host flag byte with zero payload and padding"
    );
    assert!(queue.is_empty(), "output stream contains duplicate tokens");
}

#[test]
fn open_cosim_verilator_zip_mmap_stream_roundtrip() {
    if !ensure_verilator() {
        eprintln!("skipping: verilator not found in PATH");
        return;
    }
    ensure_dpi_lib_built("frt-dpi-verilator");

    let zip_path = make_manual_zip();
    let mut instance = frt::open(&zip_path, &Simulator::Verilator).expect("open cosim instance");

    let mut mmap_word = [10u32];
    instance
        .set_buffer_arg(
            0,
            mmap_word.as_mut_ptr().cast::<u8>(),
            std::mem::size_of_val(&mmap_word),
            BufferAccess::ReadWrite,
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
        .set_stream_arg(1, &stream_path)
        .expect("set stream shm path");

    let mut end_queue = SharedMemoryQueue::create("manual_end", 2, 2).expect("end queue");
    end_queue.try_push(&[0, 1]).expect("push 7-bit EoT");
    instance
        .set_stream_arg(2, &end_queue.path().to_string_lossy())
        .expect("bind 7-bit input");
    let mut output_queue = SharedMemoryQueue::create("manual_result", 4, 8).expect("output queue");
    instance
        .set_stream_arg(3, &output_queue.path().to_string_lossy())
        .expect("bind padded 17-bit output");

    instance.write_to_device().expect("write_to_device");
    instance.exec().expect("exec");
    instance.read_from_device().expect("read_from_device");
    instance.finish().expect("finish");

    assert_eq!(mmap_word[0], 17);
    assert!(end_queue.is_empty(), "7-bit EoT was not consumed");
    check_manual_stream_output(&mut output_queue);
}

#[cfg(target_os = "linux")]
fn run_xsim_manual_roundtrip(monitor: &str) {
    use frt_cosim::context::CosimContext;
    use frt_cosim::metadata::load_spec;
    use frt_cosim::tb::xsim::{XsimOptions, XsimTbGenerator};
    use std::collections::HashMap;

    if !ensure_verilator() {
        eprintln!("skipping: verilator not found in PATH");
        return;
    }
    let dpi_lib = ensure_dpi_lib_built("frt-dpi-xsim");
    let (spec, _extracted) = load_spec(&make_manual_zip()).expect("load manual kernel");
    let mut ctx = CosimContext::new(&spec).expect("create cosim context");
    ctx.resize_buffer("a", 4).expect("resize mmap");
    ctx.buffers
        .get_mut("a")
        .expect("mmap")
        .as_mut_slice()
        .copy_from_slice(&10u32.to_le_bytes());
    ctx.streams
        .get_mut(&spec.args[1].name)
        .expect("stream")
        .try_push(&pack_stream_elem_u32(7, false))
        .expect("push stream word");
    ctx.streams
        .get_mut(&spec.args[2].name)
        .expect("7-bit stream")
        .try_push(&[0, 1])
        .expect("push 7-bit EoT");
    let mut output_queue = SharedMemoryQueue::create("manual_result", 4, 8).expect("output queue");
    ctx.bind_stream_path(&spec.args[3].name, &output_queue.path().to_string_lossy())
        .expect("bind padded 17-bit output");

    let scalar_values = HashMap::new();
    let mut tb = XsimTbGenerator::new(
        &spec,
        &dpi_lib,
        &ctx.base_addresses,
        &scalar_values,
        "",
        XsimOptions::default(),
    )
    .render_tb()
    .expect("render Xsim testbench");
    tb.insert_str(
        tb.rfind("endmodule").expect("testbench module end"),
        monitor,
    );
    tb.insert_str(
        tb.rfind("endmodule").expect("testbench module end"),
        r#"
initial begin
  repeat (200) @(posedge ap_clk);
  $fatal(1, "mmap/stream roundtrip did not complete");
end
"#,
    );
    let run = run_xsim_testbench(&spec, &ctx, &dpi_lib, &tb);
    assert!(
        run.status.success(),
        "Xsim mmap/stream roundtrip failed:\n{}\n{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        ctx.buffers["a"].as_slice(),
        &17u32.to_le_bytes(),
        "Xsim must finish the mmap/stream work before exiting"
    );
    assert!(
        ctx.streams[&spec.args[2].name].is_empty(),
        "7-bit EoT was not consumed"
    );
    check_manual_stream_output(&mut output_queue);
}

// Bundled libc++ header directories of a staged LLVM toolchain: the
// arch-specific `include/<triple>/c++/v1` (carrying `__config_site`) and the
// generic `include/c++/v1`.
#[cfg(target_os = "linux")]
fn libcxx_include_dirs(dist: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dist.join("include")) {
        for entry in entries.flatten() {
            let dir = entry.path().join("c++/v1");
            if dir.is_dir() {
                dirs.push(dir);
            }
        }
    }
    dirs.push(dist.join("include/c++/v1"));
    dirs
}

// Locate `name` (`libc++.a`, `libc++abi.a`) under the staged toolchain's
// `lib`, which is laid out either flat or under an arch subdirectory.
#[cfg(target_os = "linux")]
fn libcxx_archive(dist: &Path, name: &str) -> Option<PathBuf> {
    let lib = dist.join("lib");
    let direct = lib.join(name);
    if direct.is_file() {
        return Some(direct);
    }
    std::fs::read_dir(lib)
        .ok()?
        .flatten()
        .map(|entry| entry.path().join(name))
        .find(|path| path.is_file())
}

#[cfg(target_os = "linux")]
fn run_xsim_testbench(
    spec: &frt_cosim::metadata::KernelSpec,
    ctx: &frt_cosim::context::CosimContext,
    dpi_lib: &Path,
    tb: &str,
) -> std::process::Output {
    let tmp = tempfile::tempdir().expect("testbench directory");
    let tb_path = tmp.path().join("tb.sv");
    std::fs::write(&tb_path, tb).expect("write Xsim testbench");
    let verilator =
        std::env::var_os("VERILATOR_BIN").map_or_else(|| PathBuf::from("verilator"), PathBuf::from);
    let verilator_root = std::env::var_os("VERILATOR_ROOT")
        .map(PathBuf::from)
        .or_else(|| {
            verilator
                .parent()?
                .parent()
                .filter(|root| root.join("include/verilated.h").exists())
                .map(PathBuf::from)
        });
    let verilator = std::fs::canonicalize(&verilator).unwrap_or(verilator);
    let mut build = Command::new(verilator);
    build
        .args([
            "--binary",
            "--timing",
            "--top-module",
            &format!("tb_{}", spec.top_name),
            "--timescale-override",
            "1ns/1ps",
            "-j",
            "2",
            "-CFLAGS",
            "-std=c++20",
            "-LDFLAGS",
        ])
        .arg(dpi_lib)
        .args(["-LDFLAGS", "-Wl,--export-dynamic"])
        .arg(&tb_path)
        .args(&spec.verilog_files)
        .env_remove("VERILATOR_BIN")
        .envs(
            verilator_root
                .into_iter()
                .map(|root| ("VERILATOR_ROOT", root)),
        )
        .current_dir(tmp.path());
    // Verilator's makefile hardcodes `CXX = c++` and appends its own -std
    // after -CFLAGS, so a --timing build is at the mercy of the sandbox's
    // default compiler, while the generated timing code needs C++20
    // coroutines: the CI image's GCC 7 cannot even parse -std=c++20, and its
    // libstdc++ has no <coroutine> header at all. When Bazel stages the LLVM
    // toolchain, override the build to use its clang++ and bundled libc++.
    if let Some(dist) = std::env::var_os("FRT_XSIM_LLVM_DIST") {
        let dist = PathBuf::from(dist);
        let cxx = dist.join("bin/clang++");
        assert!(cxx.is_file(), "staged clang++ missing: {}", cxx.display());
        let libcxx = libcxx_archive(&dist, "libc++.a").expect("bundled libc++.a");
        let libcxxabi = libcxx_archive(&dist, "libc++abi.a").expect("bundled libc++abi.a");
        build
            .arg("-MAKEFLAGS")
            .arg(format!("CXX={0} LINK={0} CFG_CXXFLAGS_STD=", cxx.display()));
        build.arg("-CFLAGS").arg("-nostdinc++");
        for dir in libcxx_include_dirs(&dist) {
            build.arg("-CFLAGS").arg("-isystem").arg("-CFLAGS").arg(dir);
        }
        build
            .arg("-LDFLAGS")
            .arg(libcxx)
            .arg("-LDFLAGS")
            .arg(libcxxabi);
    }
    let build = build.output().expect("build Xsim testbench with Verilator");
    assert!(
        build.status.success(),
        "Xsim testbench build failed:\n{}\n{}",
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr)
    );
    Command::new(tmp.path().join(format!("obj_dir/Vtb_{}", spec.top_name)))
        .env(frt_shm::env::TAPA_DPI_CONFIG, ctx.dpi_config_json())
        .current_dir(tmp.path())
        .output()
        .expect("run Xsim testbench")
}

#[test]
#[cfg(target_os = "linux")]
fn xsim_write_response_survives_ready_withdrawal() {
    // Drive the DUT register so both endpoints see the same readiness. Change
    // it only on falling edges: the WLAST edge sees BREADY=1 and BVALID=0,
    // then the response must remain pending while BREADY is withdrawn.
    run_xsim_manual_roundtrip(
        r#"
initial begin : stall_write_response
  logic [64:0] accepted_id;
  wait (ap_rst_n);
  @(negedge ap_clk);
  force dut.m_axi_a_BREADY = 1'b1;
  do @(negedge ap_clk); while (!(m_axi_a_AWVALID && m_axi_a_AWREADY));
  accepted_id = m_axi_a_AWID;
  do @(negedge ap_clk); while (!(m_axi_a_WVALID && m_axi_a_WREADY && m_axi_a_WLAST));
  if (m_axi_a_BVALID) $fatal(1, "write response arrived before WLAST");
  @(negedge ap_clk);
  force dut.m_axi_a_BREADY = 1'b0;
  if (!m_axi_a_BVALID || m_axi_a_BID != accepted_id)
    $fatal(1, "write response missing after WLAST");
  repeat (2) begin
    @(negedge ap_clk);
    if (!m_axi_a_BVALID || m_axi_a_BID != accepted_id)
      $fatal(1, "write response withdrawn before BVALID/BREADY handshake");
  end
  force dut.m_axi_a_BREADY = 1'b1;
  @(negedge ap_clk);
  if (m_axi_a_BVALID)
    $fatal(1, "write response duplicated after BVALID/BREADY handshake");
  // Releasing a forced register retains its last value until its next write.
  // Restore inactive readiness before the DUT starts its second transaction.
  force dut.m_axi_a_BREADY = 1'b0;
  release dut.m_axi_a_BREADY;
end
"#,
    );
}

#[test]
#[cfg(target_os = "linux")]
fn xsim_completion_is_sampled_on_rising_edge() {
    // Generated child FSMs sample done on rising edges. A combinational pulse
    // strictly between those edges must leave the mmap/stream work running.
    run_xsim_manual_roundtrip(
        "
initial begin : completion_glitch
  wait (ap_start);
  @(negedge ap_clk);
  force dut.ap_done = 1'b1;
  #1;
  force dut.ap_done = 1'b0;
  @(negedge ap_clk);
  release dut.ap_done;
end
",
    );
}

#[test]
fn open_cosim_verilator_xo_mmap_axis_roundtrip() {
    if !ensure_verilator() {
        eprintln!("skipping: verilator not found in PATH");
        return;
    }
    ensure_dpi_lib_built("frt-dpi-verilator");

    let xo_path = make_manual_xo();
    let mut instance = frt::open(&xo_path, &Simulator::Verilator).expect("open cosim instance");

    let mut mmap_word = [10u32];
    instance
        .set_buffer_arg(
            0,
            mmap_word.as_mut_ptr().cast::<u8>(),
            std::mem::size_of_val(&mmap_word),
            BufferAccess::ReadWrite,
        )
        .expect("set mmap arg");
    instance
        .set_scalar_arg(2, &2u64.to_le_bytes())
        .expect("set scalar arg");

    let stream_name = format!("manual_axis_{}_{}", std::process::id(), mmap_word[0]);
    let mut stream_q =
        SharedMemoryQueue::create(&stream_name, 16, 5).expect("create stream shm queue");
    stream_q
        .try_push(&pack_stream_elem_u32(7, false))
        .expect("push stream word");
    let stream_path = stream_q.path().to_string_lossy().to_string();
    instance
        .set_stream_arg(1, &stream_path)
        .expect("set stream shm path");

    instance.write_to_device().expect("write_to_device");
    instance.exec().expect("exec");
    instance.read_from_device().expect("read_from_device");
    instance.finish().expect("finish");

    assert_eq!(mmap_word[0], 19);
}

#[test]
fn open_cosim_verilator_axis_handshakes_use_settled_inputs() {
    if !ensure_verilator() {
        eprintln!("skipping: verilator not found in PATH");
        return;
    }
    ensure_dpi_lib_built("frt-dpi-verilator");

    let tmp = tempfile::tempdir().expect("archive directory");
    let xo_path = tmp.path().join("axis_handshake.xo");
    let mut archive = zip::ZipWriter::new(File::create(&xo_path).expect("create archive"));
    let opts: FileOptions<'_, '_, ()> = FileOptions::default();
    archive.start_file("kernel.xml", opts).expect("kernel XML");
    archive
        .write_all(
            br#"<root><kernel name="axis_handshake"><ports>
  <port name="s" mode="read_only" dataWidth="32"/>
  <port name="result" mode="write_only" dataWidth="32"/>
</ports><args>
  <arg name="s" addressQualifier="4" id="0" port="s" depth="2"/>
  <arg name="result" addressQualifier="4" id="1" port="result" depth="1"/>
</args></kernel></root>"#,
        )
        .expect("write kernel XML");
    archive
        .start_file("rtl/axis_handshake.v", opts)
        .expect("RTL entry");
    archive
        .write_all(include_bytes!(
            "../../../tapa-core/testdata/rtl/axis_handshake.v"
        ))
        .expect("write RTL");
    archive.finish().expect("finish archive");

    let mut input = SharedMemoryQueue::create("axis_settle_input", 2, 5).expect("input queue");
    input
        .try_push(&pack_stream_elem_u32(1, false))
        .expect("first input");
    input
        .try_push(&pack_stream_elem_u32(2, true))
        .expect("second input");
    // A single output slot forces the second transfer to wait for host readback.
    let mut output = SharedMemoryQueue::create("axis_settle_output", 1, 5).expect("output queue");
    let mut instance = frt::open(&xo_path, &Simulator::Verilator).expect("open instance");
    instance
        .set_stream_arg(0, &input.path().to_string_lossy())
        .expect("bind input");
    instance
        .set_stream_arg(1, &output.path().to_string_lossy())
        .expect("bind output");
    instance.exec().expect("execute");

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut received = Vec::new();
    let completed = loop {
        let mut token = [0u8; 5];
        while output.pop_into(&mut token) {
            received.push(token);
        }
        if instance.is_finished().expect("poll simulation") {
            while output.pop_into(&mut token) {
                received.push(token);
            }
            break true;
        }
        if std::time::Instant::now() >= deadline {
            instance.kill().expect("stop timed-out simulation");
            break false;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    };
    instance.finish().expect("finish simulation");

    assert!(completed, "AXIS transfers did not finish: {received:?}");
    assert!(input.is_empty(), "an input token was not consumed");
    assert_eq!(
        received,
        [
            pack_stream_elem_u32(1, false),
            pack_stream_elem_u32(3, true)
        ],
        "every settled AXIS handshake must transfer its payload exactly once"
    );
}

#[test]
#[cfg(target_os = "linux")]
fn cosim_aligned_narrow_incr_bursts_use_requested_bytes_and_bus_lanes() {
    use frt_cosim::context::CosimContext;
    use frt_cosim::metadata::{ArgKind, ArgSpec, KernelSpec, Mode};
    use frt_cosim::runner::{verilator::VerilatorRunner, SimRunner};
    use frt_cosim::tb::xsim::{XsimOptions, XsimTbGenerator};
    use std::collections::HashMap;

    if !ensure_verilator() {
        eprintln!("skipping: verilator not found in PATH");
        return;
    }
    let spec = KernelSpec {
        top_name: "narrow_axi_incr".into(),
        mode: Mode::Hls,
        args: vec![ArgSpec {
            name: "a".into(),
            id: 0,
            kind: ArgKind::Mmap {
                data_width: 64,
                addr_width: 64,
            },
        }],
        part_num: None,
        verilog_files: vec![Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tapa-core/testdata/rtl/narrow_axi_incr.v")],
        tcl_files: vec![],
        xci_files: vec![],
        scalar_register_map: HashMap::new(),
        inlined: Vec::new(),
        memories: Vec::new(),
        rings: Vec::new(),
    };
    let initial = [0x1122_3344_u32, 0x5566_7788, 0x99aa_bbcc]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect::<Vec<_>>();
    let expected = [0x11b2_33d4_u32, 0xe5f6_0718, 0x993a_bb5c]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect::<Vec<_>>();
    let scalars = HashMap::new();
    let mut failures = Vec::new();
    for xsim in [false, true] {
        let mut ctx = CosimContext::new(&spec).expect("cosim context");
        // Three four-byte transfers on an eight-byte bus include an upper-lane
        // beat and a final beat that must stay inside the twelve-byte mapping.
        ctx.resize_buffer("a", initial.len()).expect("resize mmap");
        ctx.buffers
            .get_mut("a")
            .expect("mmap")
            .as_mut_slice()
            .copy_from_slice(&initial);
        let name = if xsim { "Xsim" } else { "Verilator" };
        let dpi = ensure_dpi_lib_built(if xsim {
            "frt-dpi-xsim"
        } else {
            "frt-dpi-verilator"
        });
        let success = if xsim {
            let mut tb = XsimTbGenerator::new(
                &spec,
                &dpi,
                &ctx.base_addresses,
                &scalars,
                "",
                XsimOptions::default(),
            )
            .render_tb()
            .expect("Xsim testbench");
            tb.insert_str(
                tb.rfind("endmodule").expect("module end"),
                "initial begin repeat (200) @(posedge ap_clk); \
                 $fatal(1, \"narrow AXI burst did not finish\"); end\n",
            );
            let run = run_xsim_testbench(&spec, &ctx, &dpi, &tb);
            if !run.status.success() {
                failures.push(format!(
                    "{name} failed: {}\n{}",
                    String::from_utf8_lossy(&run.stdout),
                    String::from_utf8_lossy(&run.stderr)
                ));
            }
            run.status.success()
        } else {
            let runner = VerilatorRunner { dpi_lib: dpi };
            let tmp = tempfile::tempdir().expect("Verilator work directory");
            runner
                .prepare(&spec, &ctx, &scalars, tmp.path())
                .expect("prepare Verilator");
            let mut child = runner
                .spawn(&spec, &ctx, tmp.path())
                .expect("start Verilator");
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            let status = loop {
                if let Some(status) = child.try_wait().expect("poll Verilator") {
                    break status;
                }
                if std::time::Instant::now() >= deadline {
                    child.kill().expect("stop timed-out Verilator");
                    break child.wait().expect("reap Verilator");
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            };
            if !status.success() {
                failures.push(format!("{name} failed: {status}"));
            }
            status.success()
        };
        if success && ctx.buffers["a"].as_slice() != expected {
            failures.push(format!(
                "{name} corrupted selected lanes or inactive bytes: {:?}",
                ctx.buffers["a"].as_slice()
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
