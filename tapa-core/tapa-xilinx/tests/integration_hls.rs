//! End-to-end HLS integration. Gated `#[ignore]`; the body runs only
//! when the environment supplies either a local Xilinx install or
//! `VARS.local.bzl`-style remote host variables.
//!
//! The live coverage drives the real `run_hls_with_retry` against a
//! trivial self-contained C++ kernel (no TAPA runtime dependency) so
//! we exercise the full TCL-emit → upload → invoke-tool →
//! collect-HDL → parse-report pipeline end-to-end without requiring
//! the TAPA include tree to be present on the remote host.

mod common;

use std::sync::Arc;

use tapa_xilinx::{
    run_hls_with_retry, HlsJob, LocalToolRunner, RemoteToolRunner, SshMuxOptions,
    SshSession, ToolInvocation, ToolRunner,
};

fn write_standalone_kernel(dir: &std::path::Path) -> std::path::PathBuf {
    // Tiny C++ kernel HLS synthesises cleanly without any TAPA or
    // vendor headers. Pinned top name is `vadd`.
    let src = dir.join("vadd.cpp");
    std::fs::write(
        &src,
        b"void vadd(const int* a, const int* b, int* c, int n) {\n\
          #pragma HLS INTERFACE m_axi port=a offset=slave\n\
          #pragma HLS INTERFACE m_axi port=b offset=slave\n\
          #pragma HLS INTERFACE m_axi port=c offset=slave\n\
          #pragma HLS INTERFACE s_axilite port=a bundle=control\n\
          #pragma HLS INTERFACE s_axilite port=b bundle=control\n\
          #pragma HLS INTERFACE s_axilite port=c bundle=control\n\
          #pragma HLS INTERFACE s_axilite port=n bundle=control\n\
          #pragma HLS INTERFACE s_axilite port=return bundle=control\n\
          for (int i = 0; i < n; ++i) c[i] = a[i] + b[i];\n\
        }\n",
    )
    .expect("write vadd.cpp");
    src
}

fn run_vadd_hls<R: ToolRunner>(runner: &R) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let src = write_standalone_kernel(tmp.path());
    let reports = tmp.path().join("reports");
    let hdl = tmp.path().join("hdl");
    let job = HlsJob {
        task_name: "vadd".into(),
        cpp_source: src,
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
    let out = match run_hls_with_retry(runner, &job, 3) {
        Ok(out) => out,
        Err(e) => panic!(
            "real run_hls must succeed against vadd fixture — err: {e}\n\
             details: {e:?}"
        ),
    };
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

fn repo_root() -> std::path::PathBuf {
    // tapa-core/tapa-xilinx/tests/ → …/tapa (repo root)
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("manifest parent")
        .to_path_buf()
}

#[test]
#[ignore = "requires real vitis_hls + TAPA tapa-lib include on the runner"]
#[allow(
    clippy::too_many_lines,
    reason = "shared-fixture flow checks multiple task modules + reports in one linear pass; splitting would hurt readability"
)]
fn vitis_hls_round_trips_shared_vadd_fixture() {
    // Exercises the shared `tests/apps/vadd/vadd.cpp` fixture
    // against real Vitis HLS.
    // That source uses `tapa.h`, so the test only runs when the
    // repo's `tapa-lib` include tree is available (which the
    // RemoteToolRunner uploads via `-I<dir>` → rootfs mirror). The
    // simpler self-contained `vitis_hls_round_trips_vadd_fixture`
    // above stays in place for the runner-plumbing check.
    if common::should_skip_without_env() {
        eprintln!(
            "integration_hls: no XILINX_HLS and no REMOTE_HOST; skipping"
        );
        return;
    }
    // The shared vadd kernel depends on the full TAPA runtime +
    // vendor include chain (gflags, glog, `ap_int`, …) being
    // reachable from HLS. Callers that have staged that on the
    // runner set `TAPA_SHARED_VADD_HLS=1`; otherwise we can't
    // distinguish a legitimate HLS regression from a missing
    // prerequisite (soft-skipping real failures is disallowed).
    // Gate up front so the test only ever runs when it is
    // expected to fully succeed.
    if std::env::var("TAPA_SHARED_VADD_HLS").ok().as_deref() != Some("1") {
        eprintln!(
            "integration_hls: TAPA_SHARED_VADD_HLS=1 not set; skipping shared vadd fixture"
        );
        return;
    }
    let tapa_lib = repo_root().join("tapa-lib");
    assert!(
        tapa_lib.join("tapa.h").is_file(),
        "TAPA_SHARED_VADD_HLS=1 set but tapa-lib/tapa.h missing at {}",
        tapa_lib.display()
    );
    let vadd_cpp =
        repo_root().join("tests").join("apps").join("vadd").join("vadd.cpp");
    assert!(
        vadd_cpp.is_file(),
        "TAPA_SHARED_VADD_HLS=1 set but {} missing",
        vadd_cpp.display()
    );
    let tmp = tempfile::tempdir().expect("tempdir");
    let reports = tmp.path().join("reports");
    let hdl = tmp.path().join("hdl");
    let job = HlsJob {
        task_name: "vadd".into(),
        cpp_source: vadd_cpp,
        cflags: vec![
            "-std=c++17".into(),
            format!("-I{}", tapa_lib.display()),
            "-DAP_INT_MAX_W=4096".into(),
        ],
        target_part: "xcu250-figd2104-2L-e".into(),
        top_name: "VecAdd".into(),
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
    let runner: Box<dyn ToolRunner> =
        if let Some(cfg) = common::has_remote_config() {
            let session = Arc::new(SshSession::new(cfg, SshMuxOptions::default()));
            session.ensure_established().expect("ssh setup");
            Box::new(RemoteToolRunner::new(session))
        } else {
            Box::new(LocalToolRunner::new())
        };
    // Once prerequisites (env + tapa-lib + fixture) are all present,
    // a real HLS failure must fail the test. Soft-skipping would
    // mask regressions in the shared-fixture code path. If the
    // remote is missing vendor pieces needed to build the fixture,
    // gate on that via the explicit skips above instead.
    let out = run_hls_with_retry(runner.as_ref(), &job, 3)
        .expect("run_hls_with_retry on shared vadd fixture must succeed");

    // Exact-parity check against the committed golden. The
    // manifest lists every field of `HlsOutput` plus the expected
    // report/HDL basenames. Any divergence — extra reports,
    // missing modules, drifted csynth scalar — fails the test.
    assert!(
        reports.is_dir() && hdl.is_dir(),
        "output dirs missing: {} / {}",
        reports.display(),
        hdl.display()
    );
    // Load the committed golden manifest and compare the live
    // `HlsOutput` to every field it names. The manifest captures
    // what Python's `tapa.backend.xilinx_hls::RunHls` produces for
    // the same shared fixture against the same Vitis HLS
    // toolchain. Keeping the manifest on disk (instead of
    // hard-coded assertions) lets reviewers update the golden by
    // editing a JSON file when HLS output evolves, without
    // touching the Rust test logic.
    let golden_path = repo_root()
        .join("tapa-core")
        .join("tapa-xilinx")
        .join("testdata")
        .join("xilinx")
        .join("real")
        .join("vadd_shared_hls_golden.json");
    let golden_text = std::fs::read_to_string(&golden_path)
        .unwrap_or_else(|e| panic!("read golden {}: {e}", golden_path.display()));
    let golden: serde_json::Value = serde_json::from_str(&golden_text)
        .expect("golden manifest must be valid JSON");
    let expect_csynth = &golden["csynth"];
    assert_eq!(
        out.csynth.top,
        expect_csynth["top"].as_str().expect("golden csynth.top"),
        "csynth top drifted from golden"
    );
    assert_eq!(
        out.csynth.part,
        expect_csynth["part"].as_str().expect("golden csynth.part"),
        "csynth part drifted from golden"
    );
    assert_eq!(
        out.csynth.target_clock_period_ns,
        expect_csynth["target_clock_period_ns"]
            .as_str()
            .expect("golden csynth.target_clock_period_ns"),
        "csynth target clock period drifted from golden"
    );
    // `estimated_clock_period_ns`: may legitimately drift run to
    // run on real tools, so the golden records `null` to accept
    // any non-empty value. A string entry in the golden enforces
    // exact equality; `null` only requires the field to be
    // non-empty (catches entirely missing reports).
    if expect_csynth["estimated_clock_period_ns"].is_string() {
        let expected = expect_csynth["estimated_clock_period_ns"]
            .as_str()
            .expect("estimated_clock_period_ns");
        assert_eq!(
            out.csynth.estimated_clock_period_ns, expected,
            "csynth estimated clock period drifted from golden"
        );
    } else {
        assert!(
            !out.csynth.estimated_clock_period_ns.is_empty(),
            "csynth estimated_clock_period_ns must be non-empty"
        );
    }
    // Exact HDL inventory — sorted set equality, no extras or
    // missing modules.
    let hdl_basenames: std::collections::BTreeSet<String> = out
        .verilog_files
        .iter()
        .filter_map(|p| p.file_name().and_then(|n| n.to_str()).map(String::from))
        .collect();
    let expected_hdl: std::collections::BTreeSet<String> = golden
        ["hdl_module_basenames"]
        .as_array()
        .expect("hdl_module_basenames")
        .iter()
        .filter_map(|v| v.as_str().map(String::from))
        .collect();
    assert_eq!(
        hdl_basenames, expected_hdl,
        "HDL inventory drifted from golden (set difference fails exact parity)"
    );
    // Exact report inventory — same treatment.
    let report_basenames: std::collections::BTreeSet<String> = out
        .report_paths
        .iter()
        .filter_map(|p| p.file_name().and_then(|n| n.to_str()).map(String::from))
        .collect();
    let expected_reports: std::collections::BTreeSet<String> = golden
        ["per_task_report_basenames"]
        .as_array()
        .expect("per_task_report_basenames")
        .iter()
        .filter_map(|v| v.as_str().map(String::from))
        .collect();
    assert_eq!(
        report_basenames, expected_reports,
        "report inventory drifted from golden (set difference fails exact parity)"
    );
    // Normalized report content markers. Each per-task `_csynth.xml`
    // must contain the TopModuleName + Part markers the golden
    // lists. This verifies HLS actually synthesized the named task
    // (not just emitted an empty report) and targeted the right
    // part, on top of the inventory check above.
    let report_content_markers = golden["per_task_report_content_markers"]
        .as_object()
        .expect("per_task_report_content_markers");
    for (basename, markers) in report_content_markers {
        let path = out
            .report_paths
            .iter()
            .find(|p| p.file_name().and_then(|n| n.to_str()) == Some(basename.as_str()))
            .unwrap_or_else(|| panic!("report {basename} missing from staged outputs"));
        let body = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        for marker in markers.as_array().expect("markers array") {
            let needle = marker.as_str().expect("marker string");
            assert!(
                body.contains(needle),
                "report {basename} missing content marker `{needle}`"
            );
        }
    }
    // Normalized HDL content markers. Each per-task HDL file must
    // contain `module <Task>` and `endmodule`, so a corrupt or
    // truncated HDL emission fails the test.
    let hdl_content_markers = golden["hdl_content_markers"]
        .as_object()
        .expect("hdl_content_markers");
    for (basename, markers) in hdl_content_markers {
        let path = out
            .verilog_files
            .iter()
            .find(|p| p.file_name().and_then(|n| n.to_str()) == Some(basename.as_str()))
            .unwrap_or_else(|| panic!("HDL {basename} missing from staged outputs"));
        let body = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        for marker in markers.as_array().expect("markers array") {
            let needle = marker.as_str().expect("marker string");
            assert!(
                body.contains(needle),
                "HDL {basename} missing content marker `{needle}`"
            );
        }
    }
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
