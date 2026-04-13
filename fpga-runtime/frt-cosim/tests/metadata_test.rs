use frt_cosim::metadata::{self, ArgKind, Mode, StreamDir, StreamProtocol};

const KERNEL_XML: &str = r#"<?xml version="1.0"?>
<root>
  <kernel name="vadd">
    <args>
      <arg name="a" addressQualifier="1" id="0" port="m_axi_a" dataWidth="512" addrWidth="64"/>
      <arg name="n" addressQualifier="0" id="1" width="32"/>
      <arg name="s" addressQualifier="4" id="2" port="s_axis_s" dataWidth="32" depth="16"/>
    </args>
  </kernel>
</root>"#;

#[test]
fn parse_vitis_kernel_xml() {
    let spec =
        metadata::xo::parse_kernel_xml(KERNEL_XML, std::path::Path::new("/tmp")).expect("parse");
    assert_eq!(spec.top_name, "vadd");
    assert_eq!(spec.mode, Mode::Vitis);
    assert_eq!(spec.args.len(), 3);
    assert!(matches!(spec.args[0].kind, ArgKind::Mmap { .. }));
    assert!(matches!(spec.args[1].kind, ArgKind::Scalar { .. }));
    assert!(matches!(
        spec.args[2].kind,
        ArgKind::Stream {
            protocol: StreamProtocol::Axis,
            ..
        }
    ));
}

const GRAPH_YAML: &str = "
top: vadd
args:
  - name: a
    id: 0
    type: mmap
    width: 512
    addr_width: 64
  - name: n
    id: 1
    type: scalar
    width: 32
  - name: s
    id: 2
    type: stream
    width: 32
    depth: 16
    dir: in
";

#[test]
fn parse_hls_graph_yaml() {
    let spec = metadata::zip_pkg::parse_graph_yaml(GRAPH_YAML, std::path::Path::new("/tmp"))
        .expect("parse");
    assert_eq!(spec.top_name, "vadd");
    assert_eq!(spec.mode, Mode::Hls);
    assert_eq!(spec.args.len(), 3);
    assert!(matches!(spec.args[0].kind, ArgKind::Mmap { .. }));
    assert!(matches!(spec.args[1].kind, ArgKind::Scalar { .. }));
    assert!(matches!(
        &spec.args[2].kind,
        ArgKind::Stream {
            dir: StreamDir::In,
            protocol: StreamProtocol::ApFifo,
            ..
        }
    ));
}

const LEGACY_GRAPH_YAML: &str = "
top: vadd
tasks:
  vadd:
    level: lower
    ports:
      - name: a
        cat: mmap
        width: 32
      - name: s
        cat: istream
        width: 32
        depth: 8
      - name: out
        cat: ostreams
        width: 32
        depth: 8
        chan_count: 2
";

#[test]
fn parse_legacy_graph_yaml() {
    let spec = metadata::zip_pkg::parse_graph_yaml(LEGACY_GRAPH_YAML, std::path::Path::new("/tmp"))
        .expect("parse");
    assert_eq!(spec.top_name, "vadd");
    assert_eq!(spec.mode, Mode::Hls);
    assert_eq!(spec.args[0].name, "a");
    assert_eq!(spec.args[1].name, "s_s");
    assert_eq!(spec.args[2].name, "out_0");
    assert_eq!(spec.args[3].name, "out_1");
    assert!(matches!(
        &spec.args[1].kind,
        ArgKind::Stream {
            dir: StreamDir::In,
            protocol: StreamProtocol::ApFifo,
            ..
        }
    ));
}
