use frt_cosim::metadata::{ArgKind, ArgSpec, KernelSpec, Mode, StreamDir};
use frt_cosim::tb::verilator::VerilatorTbGenerator;
use std::collections::HashMap;

fn hls_spec() -> KernelSpec {
    KernelSpec {
        top_name: "vadd".into(),
        mode: Mode::Hls,
        part_num: None,
        verilog_files: vec![],
        scalar_register_map: HashMap::from([("n".into(), 0x18u32)]),
        args: vec![
            ArgSpec {
                name: "a".into(),
                id: 0,
                kind: ArgKind::Mmap {
                    data_width: 512,
                    addr_width: 64,
                },
            },
            ArgSpec {
                name: "n".into(),
                id: 1,
                kind: ArgKind::Scalar { width: 32 },
            },
                ArgSpec {
                    name: "s".into(),
                    id: 2,
                    kind: ArgKind::Stream {
                        width: 32,
                        depth: 16,
                        dir: StreamDir::In,
                        protocol: frt_cosim::metadata::StreamProtocol::ApFifo,
                    },
                },
            ],
        }
}

fn vitis_spec() -> KernelSpec {
    KernelSpec {
        top_name: "vadd".into(),
        mode: Mode::Vitis,
        part_num: Some("xc7a100tcsg324-1".into()),
        verilog_files: vec![],
        scalar_register_map: HashMap::from([("a".into(), 0x10u32), ("n".into(), 0x1cu32)]),
        args: vec![
            ArgSpec {
                name: "a".into(),
                id: 0,
                kind: ArgKind::Mmap {
                    data_width: 32,
                    addr_width: 64,
                },
            },
            ArgSpec {
                name: "s".into(),
                id: 1,
                kind: ArgKind::Stream {
                    width: 32,
                    depth: 8,
                    dir: StreamDir::In,
                    protocol: frt_cosim::metadata::StreamProtocol::Axis,
                },
            },
            ArgSpec {
                name: "n".into(),
                id: 2,
                kind: ArgKind::Scalar { width: 32 },
            },
        ],
    }
}

#[test]
fn verilator_hls_tb_snapshot() {
    let spec = hls_spec();
    let base_addrs = std::collections::HashMap::from([("a".into(), 0x1000_0000u64)]);
    let scalar_vals = std::collections::HashMap::from([(1u32, 7u64)]);
    let generator =
        VerilatorTbGenerator::new(&spec, std::path::Path::new("libfrt_dpi_verilator.so"), &base_addrs, &scalar_vals);
    let tb = generator.render_tb().expect("render");
    assert!(tb.contains("service_all_axi"));
    assert!(tb.contains("tapa_stream_try_read"));
    assert!(tb.contains("m_axi_a_ARADDR"));
}

#[test]
fn xsim_hls_tb_snapshot() {
    use frt_cosim::tb::xsim::XsimTbGenerator;
    let spec = hls_spec();
    let base_addrs = std::collections::HashMap::from([("a".into(), 0x1000_0000u64)]);
    let scalar_vals = std::collections::HashMap::from([(1u32, 3u64)]);
    let generator = XsimTbGenerator::new(
        &spec,
        std::path::Path::new("/path/to/frt_dpi_xsim.so"),
        &base_addrs,
        &scalar_vals,
        "xc7a100tcsg324-1",
        false,
    );
    let tb = generator.render_tb().expect("render tb");
    assert!(tb.contains("module tb_vadd"));
    assert!(tb.contains("tapa_axi_read"));
    let tcl = generator
        .render_tcl(std::path::Path::new("/tmp/tb"))
        .expect("render tcl");
    assert!(tcl.contains("set_property top tb_vadd"));
}

#[test]
fn xsim_vitis_tb_contains_control_sequence() {
    use frt_cosim::tb::xsim::XsimTbGenerator;
    let spec = vitis_spec();
    let base_addrs = std::collections::HashMap::from([("a".into(), 0x1000_0000u64)]);
    let scalar_vals = std::collections::HashMap::from([(2u32, 7u64)]);
    let generator = XsimTbGenerator::new(
        &spec,
        std::path::Path::new("/path/to/frt_dpi_xsim.so"),
        &base_addrs,
        &scalar_vals,
        "xc7a100tcsg324-1",
        false,
    );
    let tb = generator.render_tb().expect("render tb");
    assert!(tb.contains("task automatic ctrl_write"));
    assert!(tb.contains("ctrl_write(8'h00, 32'h0000_0001);"));
    assert!(tb.contains("if (interrupt) begin"));
}
