use frt_cosim::metadata::{ArgKind, ArgSpec, KernelSpec, Mode, StreamDir};
use frt_cosim::tb::verilator::VerilatorTbGenerator;
use std::collections::HashMap;

fn hls_spec() -> KernelSpec {
    KernelSpec {
        top_name: "vadd".into(),
        mode: Mode::Hls,
        part_num: None,
        verilog_files: vec![],
        tcl_files: vec![],
        xci_files: vec![],
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
        tcl_files: vec![],
        xci_files: vec![],
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

fn banked_hls_spec() -> KernelSpec {
    KernelSpec {
        top_name: "Bandwidth".into(),
        mode: Mode::Hls,
        part_num: None,
        verilog_files: vec![],
        tcl_files: vec![],
        xci_files: vec![],
        scalar_register_map: HashMap::from([("n".into(), 0x18u32)]),
        args: vec![
            ArgSpec {
                name: "chan[0]".into(),
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
        ],
    }
}

#[test]
fn verilator_hls_tb_snapshot() {
    let spec = hls_spec();
    let base_addrs = std::collections::HashMap::from([("a".into(), 0x1000_0000u64)]);
    let buf_sizes = std::collections::HashMap::from([("a".into(), 4096usize)]);
    let scalar_vals = std::collections::HashMap::from([(1u32, vec![7u8, 0, 0, 0])]);
    let generator = VerilatorTbGenerator::new(
        &spec,
        std::path::Path::new("libfrt_dpi_verilator.so"),
        &base_addrs,
        &buf_sizes,
        &scalar_vals,
    );
    let tb = generator.render_tb().expect("render");
    assert!(tb.contains("service_all_axi"));
    assert!(tb.contains("tapa_stream_try_read"));
    assert!(tb.contains("m_axi_a_ARADDR"));
}

#[test]
fn verilator_hls_escapes_banked_mmap_names() {
    let spec = banked_hls_spec();
    let base_addrs = std::collections::HashMap::from([("chan[0]".into(), 0x1000_0000u64)]);
    let buf_sizes = std::collections::HashMap::from([("chan[0]".into(), 4096usize)]);
    let scalar_vals = std::collections::HashMap::from([(1u32, vec![7u8, 0, 0, 0])]);
    let generator = VerilatorTbGenerator::new(
        &spec,
        std::path::Path::new("libfrt_dpi_verilator.so"),
        &base_addrs,
        &buf_sizes,
        &scalar_vals,
    );
    let tb = generator.render_tb().expect("render");
    // Verilator strips brackets: chan[0] → chan_0 in C++ member names
    assert!(tb.contains("rd_chan_0"), "{tb}");
    assert!(tb.contains("dut->m_axi_chan_0_ARREADY"), "{tb}");
    // The load_from_shm call still uses the original name for port lookup
    assert!(tb.contains("load_from_shm(\"chan[0]\""), "{tb}");
}

#[test]
fn xsim_hls_tb_snapshot() {
    use frt_cosim::tb::xsim::XsimTbGenerator;
    let spec = hls_spec();
    let base_addrs = std::collections::HashMap::from([("a".into(), 0x1000_0000u64)]);
    let scalar_vals = std::collections::HashMap::from([(1u32, vec![3u8, 0, 0, 0])]);
    let generator = XsimTbGenerator::new(
        &spec,
        std::path::Path::new("/path/to/frt_dpi_xsim.so"),
        &base_addrs,
        &scalar_vals,
        "xc7a100tcsg324-1",
        false,
        false,
    );
    let tb = generator.render_tb().expect("render tb");
    assert!(tb.contains("module tb_vadd"));
    assert!(tb.contains("tapa_axi_read"));
    assert!(tb.contains("wait (ap_done === 1'b1);"));
    assert!(tb.contains("repeat (2) @(posedge ap_clk);"));
    assert!(!tb.contains("simulation timeout"));
    let tcl = generator
        .render_tcl(std::path::Path::new("/tmp/tb"))
        .expect("render tcl");
    assert!(tcl.contains("set_property top tb_vadd"));
}

#[test]
fn xsim_hls_escapes_banked_mmap_names() {
    use frt_cosim::tb::xsim::XsimTbGenerator;
    let spec = banked_hls_spec();
    let base_addrs = std::collections::HashMap::from([("chan[0]".into(), 0x1000_0000u64)]);
    let scalar_vals = std::collections::HashMap::from([(1u32, vec![7u8, 0, 0, 0])]);
    let generator = XsimTbGenerator::new(
        &spec,
        std::path::Path::new("/path/to/frt_dpi_xsim.so"),
        &base_addrs,
        &scalar_vals,
        "xc7a100tcsg324-1",
        false,
        false,
    );
    let tb = generator.render_tb().expect("render tb");
    assert!(tb.contains("m_axi_chan__05b0__05d_ARADDR"));
    assert!(tb.contains("\\m_axi_chan[0]_ARADDR"));
    assert!(tb.contains("\\chan[0]_offset"));
}

#[test]
fn xsim_vitis_tb_contains_control_sequence() {
    use frt_cosim::tb::xsim::XsimTbGenerator;
    let spec = vitis_spec();
    let base_addrs = std::collections::HashMap::from([("a".into(), 0x1000_0000u64)]);
    let scalar_vals = std::collections::HashMap::from([(2u32, vec![7u8, 0, 0, 0])]);
    let generator = XsimTbGenerator::new(
        &spec,
        std::path::Path::new("/path/to/frt_dpi_xsim.so"),
        &base_addrs,
        &scalar_vals,
        "xc7a100tcsg324-1",
        false,
        false,
    );
    let tb = generator.render_tb().expect("render tb");
    assert!(tb.contains("task automatic ctrl_write"));
    assert!(tb.contains("ctrl_write(8'h00, 32'h0000_0001);"));
    assert!(tb.contains("wait (interrupt === 1'b1);"));
    assert!(tb.contains("repeat (2) @(posedge ap_clk);"));
    assert!(!tb.contains("simulation timeout"));
}

#[test]
fn xsim_tcl_includes_xci_tcl_and_legacy_elab_property() {
    use frt_cosim::tb::xsim::XsimTbGenerator;
    let mut spec = vitis_spec();
    spec.verilog_files = vec![std::path::PathBuf::from("/tmp/rtl/top.v")];
    spec.xci_files = vec![std::path::PathBuf::from("/tmp/ip/example.xci")];
    spec.tcl_files = vec![std::path::PathBuf::from("/tmp/ip/setup_ip.tcl")];

    let base_addrs = std::collections::HashMap::from([("a".into(), 0x1000_0000u64)]);
    let scalar_vals = std::collections::HashMap::from([(2u32, vec![7u8, 0, 0, 0])]);
    let generator = XsimTbGenerator::new(
        &spec,
        std::path::Path::new("/path/to/frt_dpi_xsim.so"),
        &base_addrs,
        &scalar_vals,
        "xc7a100tcsg324-1",
        true,
        true,
    );
    let tcl = generator
        .render_tcl(std::path::Path::new("/tmp/tb"))
        .expect("render tcl");
    assert!(tcl.contains("add_files -norecurse -scan_for_includes /tmp/ip/example.xci"));
    assert!(tcl.contains("source /tmp/ip/setup_ip.tcl"));
    assert!(tcl.contains("upgrade_ip -quiet [get_ips *]"));
    assert!(tcl.contains("set_property -name {xelab.more_options}"));
    assert!(tcl.contains("set_property -name {xsim.simulate.wdb}"));
}
