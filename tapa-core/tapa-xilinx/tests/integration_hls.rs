//! End-to-end HLS integration. Gated `#[ignore]`; the body runs only
//! when the environment supplies either a local Xilinx install or
//! `VARS.local.bzl`-style remote host variables.
//!
//! The live coverage drives the real `run_hls_with_retry` against the
//! shared `tests/apps/vadd` C++ kernel so we exercise the full
//! TCL-emit → invoke-tool → collect-HDL → parse-report pipeline
//! instead of a banner check.

mod common;

use std::path::PathBuf;
use std::sync::Arc;

use tapa_xilinx::{
    run_hls_with_retry, HlsJob, LocalToolRunner, RemoteToolRunner, SshMuxOptions,
    SshSession, ToolInvocation, ToolRunner,
};

fn repo_root() -> PathBuf {
    // tapa-core/tapa-xilinx/tests/ → …/tapa (repo root)
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("manifest parent")
        .to_path_buf()
}

fn vadd_cpp() -> PathBuf {
    repo_root().join("tests").join("apps").join("vadd").join("vadd.cpp")
}

fn run_vadd_hls<R: ToolRunner>(runner: &R) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let reports = tmp.path().join("reports");
    let hdl = tmp.path().join("hdl");
    let job = HlsJob {
        task_name: "vadd".into(),
        cpp_source: vadd_cpp(),
        cflags: vec!["-std=c++17".into()],
        target_part: "xcu250-figd2104-2L-e".into(),
        top_name: "vadd".into(),
        clock_period: "3.33".into(),
        reports_out_dir: reports.clone(),
        hdl_out_dir: hdl.clone(),
        uploads: vec![],
        downloads: vec![],
        other_configs: String::new(),
        solution_name: "solution1".into(),
        reset_low: true,
        auto_prefix: true,
        transient_patterns: None,
    };
    let out = run_hls_with_retry(runner, &job, 3)
        .expect("real run_hls must succeed against vadd fixture");
    assert!(
        !out.verilog_files.is_empty(),
        "no HDL produced; stdout={}",
        out.stdout
    );
    assert!(
        !out.report_paths.is_empty(),
        "no reports produced; stdout={}",
        out.stdout
    );
    // Real HLS artifacts must land in the caller-visible output dirs
    // (Python parity: `tapa/backend/xilinx_hls.py` copies
    // `project/<solution>/syn/report` and `.../syn/verilog` onto the
    // paths the caller provides).
    assert!(
        reports.is_dir(),
        "reports_out_dir missing: {}",
        reports.display()
    );
    assert!(hdl.is_dir(), "hdl_out_dir missing: {}", hdl.display());
    let csynth_xml_canonical = reports.join("vadd_csynth.xml");
    let csynth_xml_legacy = reports.join("vadd.csynth.xml");
    assert!(
        csynth_xml_canonical.is_file() || csynth_xml_legacy.is_file(),
        "csynth.xml not staged under {}",
        reports.display()
    );
    let has_verilog = std::fs::read_dir(&hdl)
        .expect("read hdl_out_dir")
        .filter_map(Result::ok)
        .any(|ent| {
            ent.path()
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e == "v" || e == "sv" || e == "vhd")
        });
    assert!(has_verilog, "no HDL files under {}", hdl.display());
}

#[test]
#[ignore = "requires real vitis_hls or configured remote host"]
fn vitis_hls_round_trips_vadd_fixture() {
    if common::should_skip_without_env() {
        eprintln!("integration_hls: no XILINX_HLS and no REMOTE_HOST; skipping");
        return;
    }
    if let Some(cfg) = common::has_remote_config() {
        let session = Arc::new(SshSession::new(cfg, SshMuxOptions::default()));
        session.ensure_established().expect("ssh setup");
        let runner = RemoteToolRunner::new(session);
        // Cheap preflight so that if the remote is wedged we produce a
        // readable failure at the -version step before starting HLS.
        let ver = runner
            .run(&ToolInvocation::new("vitis_hls").arg("-version"))
            .expect("vitis_hls -version preflight");
        assert!(
            ver.stdout.contains("Vitis HLS") || ver.stderr.contains("Vitis HLS"),
            "vitis_hls not reachable on remote: {}{}",
            ver.stdout,
            ver.stderr
        );
        run_vadd_hls(&runner);
    } else {
        run_vadd_hls(&LocalToolRunner::new());
    }
}
