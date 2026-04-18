//! End-to-end `.xo` packaging integration. Gated `#[ignore]`; the
//! body runs only when a `VARS.local.bzl`-style remote Xilinx host
//! is configured. The Vivado invocation flows through
//! `RemoteToolRunner`; local Vivado on the development host is
//! intentionally out of scope for this module.
//!
//! Exercises the full Rust `pack_xo` path: `emit_kernel_xml` →
//! Vivado `package_xo` TCL → real Vivado invocation via
//! `RemoteToolRunner` → download → reproducibility redaction.
//! After the Rust pipeline finishes, the test takes the
//! pre-redaction `.xo` Rust just produced, hands one copy to
//! Rust's `redact_xo` and another to Python's production
//! `_redact_and_zip`, and compares entry listing, per-entry
//! SHA-256, normalized timestamps, and the canonical inner
//! `kernel.xml` byte-for-byte. That is the single-artifact
//! Rust-vs-Python `.xo` parity proof.

mod common;

use std::io::Read;
use std::path::PathBuf;
use std::sync::Arc;

use tapa_xilinx::{
    emit_kernel_xml, pack_xo_without_redaction, redact_xo, DeviceInfo,
    KernelXmlArgs, KernelXmlPort, PackageXoInputs, PortCategory,
    RemoteToolRunner, SshMuxOptions, SshSession,
};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("manifest parent")
        .to_path_buf()
}

/// Per-entry metadata captured for the single-artifact `.xo`
/// parity comparison: filename, size, MS-DOS timestamp tuple, and
/// the SHA-256 hex digest of the content.
type ZipEntryInventory =
    (String, u64, (u16, u8, u8, u8, u8, u8), String);

/// Sorted `ZipEntryInventory` values for every entry in a `.xo`
/// archive. Used to compare Rust- and Python-redacted archives as
/// the single-artifact `.xo` parity check.
fn zip_inventory(bytes: &[u8]) -> Vec<ZipEntryInventory> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .expect("open archive for inventory");
    let mut out = Vec::with_capacity(archive.len());
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).expect("archive index");
        let name = entry.name().to_string();
        let size = entry.size();
        let dt = entry.last_modified().expect("mtime");
        let tuple = (
            dt.year(),
            dt.month(),
            dt.day(),
            dt.hour(),
            dt.minute(),
            dt.second(),
        );
        let mut buf: Vec<u8> = Vec::new();
        entry.read_to_end(&mut buf).expect("read entry");
        let digest = Sha256::digest(&buf);
        let mut hex = String::with_capacity(64);
        for b in digest {
            use std::fmt::Write as _;
            let _ = write!(hex, "{b:02x}");
        }
        out.push((name, size, tuple, hex));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn kernel_xml_args() -> KernelXmlArgs {
    KernelXmlArgs {
        top_name: "vadd".into(),
        clock_period: "3.33".into(),
        ports: vec![
            KernelXmlPort {
                name: "a".into(),
                category: PortCategory::MAxi,
                width: 32,
                port: String::new(),
                ctype: "int*".into(),
            },
            KernelXmlPort {
                name: "b".into(),
                category: PortCategory::MAxi,
                width: 32,
                port: String::new(),
                ctype: "int*".into(),
            },
            KernelXmlPort {
                name: "c".into(),
                category: PortCategory::MAxi,
                width: 32,
                port: String::new(),
                ctype: "int*".into(),
            },
            KernelXmlPort {
                name: "n".into(),
                category: PortCategory::Scalar,
                width: 32,
                port: String::new(),
                ctype: "int".into(),
            },
        ],
    }
}

#[test]
#[ignore = "requires real Vivado or configured remote host"]
#[allow(
    clippy::too_many_lines,
    reason = "single-artifact .xoparity flow is linear; splitting would obscure the inputs→outputs pipeline"
)]
fn live_pack_xo_roundtrips_vadd_rtl() {
    // This module is intentionally remote-only: the Vivado
    // invocation goes through `RemoteToolRunner`. A local-Vivado
    // path is out of scope here.
    let Some(cfg) = common::has_remote_config() else {
        eprintln!(
            "integration_pack_xo: no REMOTE_HOST; skipping live package_xo"
        );
        return;
    };
    let real_hdl = repo_root()
        .join("tapa-core")
        .join("tapa-xilinx")
        .join("testdata")
        .join("xilinx")
        .join("real");
    if !real_hdl.join("vadd.v").is_file() {
        eprintln!(
            "integration_pack_xo: real vadd.v fixture missing at {}; skipping",
            real_hdl.display()
        );
        return;
    }

    let tmp = tempfile::tempdir().expect("tempdir");
    let hdl_dir = tmp.path().join("hdl");
    std::fs::create_dir_all(&hdl_dir).expect("mkdir hdl");
    std::fs::copy(real_hdl.join("vadd.v"), hdl_dir.join("vadd.v"))
        .expect("stage HDL fixture");
    let xo_out = tmp.path().join("vadd.xo");
    let inputs = PackageXoInputs {
        top_name: "vadd".into(),
        hdl_dir,
        device_info: DeviceInfo {
            part_num: "xcu250-figd2104-2L-e".into(),
            clock_period: "3.33".into(),
        },
        clock_period: "3.33".into(),
        kernel_xml: kernel_xml_args(),
        kernel_out_path: xo_out,
        cpp_kernels: vec![],
        m_axi_params: vec![],
        s_axi_ifaces: PackageXoInputs::default_s_axi(),
    };

    let session = Arc::new(SshSession::new(cfg, SshMuxOptions::default()));
    session.ensure_established().expect("ssh setup");
    let runner = RemoteToolRunner::new(session);

    // Drive Rust's live `pack_xo` pipeline but stop short of the
    // reproducibility redaction pass: that gives us a pristine,
    // pre-redaction Rust-produced `.xo` we can hand to **both** the
    // Rust redactor and Python's production `_redact_and_zip` for a
    // single-artifact Rust-vs-Python parity comparison.
    let produced = pack_xo_without_redaction(&runner, &inputs)
        .expect("live pack_xo_without_redaction must succeed");
    assert!(
        produced.is_file(),
        "pack_xo returned {} but the file is missing",
        produced.display()
    );

    // Split the pre-redaction artifact into two independent copies:
    // one for Rust `redact_xo`, one for Python `_redact_and_zip`.
    let rust_xo = tmp.path().join("rust.xo");
    let py_xo = tmp.path().join("py.xo");
    std::fs::copy(&produced, &rust_xo).expect("copy to rust side");
    std::fs::copy(&produced, &py_xo).expect("copy to python side");

    redact_xo(&rust_xo).expect("rust redact_xo must succeed");

    // Redact the other copy through Python's production
    // `_redact_and_zip` via an inline `python3` subprocess. This is
    // the single-artifact Rust-vs-Python parity path .xoasks for
    // — both sides see the exact same input bytes, so any
    // observable divergence is a real parity bug.
    let py_script = "import zipfile, shutil, sys\n\
         from tapa.program.pack import _redact_and_zip\n\
         src = sys.argv[1]\n\
         staging = src + '.staging'\n\
         shutil.copy(src, staging)\n\
         with zipfile.ZipFile(staging, 'r') as zi, zipfile.ZipFile(src, 'w') as zo:\n\
         \x20   _redact_and_zip(zi, zo)\n";
    let repo_env_pp = repo_root();
    let py_status = std::process::Command::new("python3")
        .env("PYTHONPATH", repo_env_pp.as_os_str())
        .arg("-c")
        .arg(py_script)
        .arg(py_xo.as_os_str())
        .status()
        .expect("python3 must be on PATH for .xoparity");
    assert!(
        py_status.success(),
        "python `_redact_and_zip` exited with {py_status}"
    );

    // .xoparity: listing + per-entry SHA-256 + normalized
    // timestamps + canonical inner kernel.xml.
    let rust_archive = std::fs::read(&rust_xo).expect("read rust.xo");
    let py_archive = std::fs::read(&py_xo).expect("read py.xo");
    assert_eq!(
        zip_inventory(&rust_archive),
        zip_inventory(&py_archive),
        "Rust and Python .xo redaction listings drifted"
    );

    // Canonical inner kernel.xml equality + Rust-emitter parity.
    let archive_bytes = std::fs::read(&rust_xo).expect("read produced .xo");
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(archive_bytes))
        .expect("open .xo");
    // Vivado `package_xo` stores `kernel.xml` under a directory
    // path inside the produced `.xo`; scan the archive for the
    // canonical entry rather than assuming a root-level filename.
    let kernel_xml_entry_name: String = (0..archive.len())
        .map(|i| archive.by_index(i).unwrap().name().to_string())
        .find(|n| n.ends_with("/kernel.xml") || n == "kernel.xml")
        .unwrap_or_else(|| {
            panic!(
                "produced .xo does not contain any kernel.xml entry: {:?}",
                (0..archive.len())
                    .map(|i| archive.by_index(i).unwrap().name().to_string())
                    .collect::<Vec<_>>()
            )
        });
    let mut kernel_xml_body = String::new();
    archive
        .by_name(&kernel_xml_entry_name)
        .expect("kernel.xml entry")
        .read_to_string(&mut kernel_xml_body)
        .expect("read kernel.xml");
    let rust_emitted = emit_kernel_xml(&kernel_xml_args()).expect("emit");
    let lhs: String = kernel_xml_body.split_whitespace().collect::<Vec<_>>().join(" ");
    let rhs: String = rust_emitted.split_whitespace().collect::<Vec<_>>().join(" ");
    assert_eq!(
        lhs, rhs,
        "kernel.xml inside live-packaged .xo must match Rust emit_kernel_xml"
    );

    // Redaction idempotence: every entry's date_time must be the
    // MS-DOS epoch.
    for i in 0..archive.len() {
        let entry = archive.by_index(i).expect("entry");
        let name = entry.name().to_string();
        let dt = entry.last_modified().expect("zip entry mtime");
        assert_eq!(
            (
                dt.year(),
                dt.month(),
                dt.day(),
                dt.hour(),
                dt.minute(),
                dt.second()
            ),
            (1980, 1, 1, 0, 0, 0),
            "entry {name} kept a non-epoch timestamp: {dt:?}"
        );
    }
}
