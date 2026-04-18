//! Fixture smoke tests: exercise parsers/emitters against the committed
//! testdata files. Any fixture rename or deletion breaks at least one
//! of these, satisfying AC-16's negative case.

use std::path::PathBuf;

use tapa_xilinx::{
    emit_kernel_xml, parse_csynth_xml, parse_hpfm_xml_via_device, parse_utilization_rpt,
    KernelXmlArgs, KernelXmlPort, PortCategory,
};

fn testdata(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("testdata")
        .join("xilinx")
        .join(name)
}

#[test]
fn fixture_hpfm_parses() {
    let xml = std::fs::read(testdata("sample.hpfm")).unwrap();
    let info = parse_hpfm_xml_via_device(&xml).unwrap();
    assert_eq!(info.part_num, "xcu250-figd2104-2L-e");
    assert_eq!(info.clock_period, "3.333");
}

#[test]
fn fixture_csynth_parses() {
    let xml = std::fs::read(testdata("sample.csynth.xml")).unwrap();
    let r = parse_csynth_xml(&xml).unwrap();
    assert_eq!(r.top, "vadd");
    assert_eq!(r.target_clock_period_ns, "3.333");
}

#[test]
fn fixture_utilization_parses() {
    let text = std::fs::read_to_string(testdata("sample.utilization.rpt")).unwrap();
    let r = parse_utilization_rpt(&text).unwrap();
    assert_eq!(r.device, "xcu250");
    assert_eq!(r.instance, "top");
}

#[test]
fn fixture_transient_stderr_matches_default_predicate() {
    let text = std::fs::read_to_string(testdata("hls_transient_stderr.txt")).unwrap();
    let matched = tapa_xilinx::DEFAULT_TRANSIENT_HLS_PATTERNS
        .iter()
        .any(|p| text.contains(p));
    assert!(matched, "captured stderr fixture should match a transient pattern");
}

#[test]
fn xpfm_fixture_parses() {
    let bytes = std::fs::read(testdata("sample.xpfm")).unwrap();
    let info = tapa_xilinx::parse_xpfm(&bytes).unwrap();
    assert_eq!(info.part_num, "xcu250-figd2104-2L-e");
    assert_eq!(info.clock_period, "3.333");
}

#[test]
fn kernel_xml_matches_golden_fixture() {
    let args = KernelXmlArgs {
        top_name: "vadd".into(),
        clock_period: "3.33".into(),
        ports: vec![
            KernelXmlPort {
                name: "a".into(),
                category: PortCategory::MAxi,
                width: 512,
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
    };
    let xml = emit_kernel_xml(&args).unwrap();
    let golden = std::fs::read_to_string(testdata("kernel_xml.golden.xml")).unwrap();
    // Trim surrounding whitespace for newline tolerance.
    assert_eq!(
        xml.trim(),
        golden.trim(),
        "kernel.xml drifted from golden fixture"
    );
}

#[test]
fn kernel_xml_emitter_round_trips_simple_fixture() {
    let args = KernelXmlArgs {
        top_name: "vadd".into(),
        clock_period: "3.33".into(),
        ports: vec![
            KernelXmlPort {
                name: "a".into(),
                category: PortCategory::MAxi,
                width: 512,
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
    };
    let xml = emit_kernel_xml(&args).unwrap();
    assert!(xml.contains("kernel name=\"vadd\""));
    assert!(xml.contains("m_axi_a"));
    assert!(xml.contains("hwControlProtocol=\"ap_ctrl_hs\""));
}
