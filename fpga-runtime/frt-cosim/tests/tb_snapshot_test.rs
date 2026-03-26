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
                },
            },
        ],
    }
}

#[test]
fn verilator_hls_tb_snapshot() {
    let spec = hls_spec();
    let base_addrs = std::collections::HashMap::from([("a".into(), 0x1000_0000u64)]);
    let generator = VerilatorTbGenerator::new(
        &spec,
        std::path::Path::new("libfrt_dpi_verilator.so"),
        &base_addrs,
    );
    let tb = generator.render_tb().expect("render");
    insta::assert_snapshot!(tb);
}

#[test]
fn xsim_hls_tb_snapshot() {
    use frt_cosim::tb::xsim::XsimTbGenerator;
    let spec = hls_spec();
    let base_addrs = std::collections::HashMap::from([("a".into(), 0x1000_0000u64)]);
    let generator = XsimTbGenerator::new(
        &spec,
        std::path::Path::new("/path/to/frt_dpi_xsim.so"),
        &base_addrs,
        "xc7a100tcsg324-1",
        false,
    );
    insta::assert_snapshot!("xsim_tb_sv", generator.render_tb().expect("render tb"));
    insta::assert_snapshot!(
        "xsim_tcl",
        generator
            .render_tcl(std::path::Path::new("/tmp/tb"))
            .expect("render tcl")
    );
}
