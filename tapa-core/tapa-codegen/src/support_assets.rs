//! Static Verilog support assets emitted alongside generated RTL.

pub const VERILOG_SUPPORT_ASSETS: &[(&str, &str)] = &[
    ("arbiter.v", include_str!("../assets/verilog/arbiter.v")),
    (
        "axis_adapter.v",
        include_str!("../assets/verilog/axis_adapter.v"),
    ),
    (
        "async_mmap.v",
        include_str!("../assets/verilog/async_mmap.v"),
    ),
    (
        "axi_pipeline.v",
        include_str!("../assets/verilog/axi_pipeline.v"),
    ),
    (
        "axi_crossbar_addr.v",
        include_str!("../assets/verilog/axi_crossbar_addr.v"),
    ),
    (
        "axi_crossbar_rd.v",
        include_str!("../assets/verilog/axi_crossbar_rd.v"),
    ),
    (
        "axi_crossbar_wr.v",
        include_str!("../assets/verilog/axi_crossbar_wr.v"),
    ),
    (
        "axi_crossbar.v",
        include_str!("../assets/verilog/axi_crossbar.v"),
    ),
    (
        "axi_register_rd.v",
        include_str!("../assets/verilog/axi_register_rd.v"),
    ),
    (
        "axi_register_wr.v",
        include_str!("../assets/verilog/axi_register_wr.v"),
    ),
    (
        "detect_burst.v",
        include_str!("../assets/verilog/detect_burst.v"),
    ),
    ("fifo.v", include_str!("../assets/verilog/fifo.v")),
    ("fifo_bram.v", include_str!("../assets/verilog/fifo_bram.v")),
    ("fifo_fwd.v", include_str!("../assets/verilog/fifo_fwd.v")),
    ("fifo_srl.v", include_str!("../assets/verilog/fifo_srl.v")),
    (
        "generate_last.v",
        include_str!("../assets/verilog/generate_last.v"),
    ),
    (
        "priority_encoder.v",
        include_str!("../assets/verilog/priority_encoder.v"),
    ),
    (
        "relay_station.v",
        include_str!("../assets/verilog/relay_station.v"),
    ),
    (
        "a_axi_write_broadcastor_1_to_3.v",
        include_str!("../assets/verilog/a_axi_write_broadcastor_1_to_3.v"),
    ),
    (
        "a_axi_write_broadcastor_1_to_4.v",
        include_str!("../assets/verilog/a_axi_write_broadcastor_1_to_4.v"),
    ),
];
