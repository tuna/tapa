"""Tests for Verilator TB AXI helper generation."""

from tapa.cosim.common import AXI, Arg, Port
from tapa.cosim.verilator_tb_axi import (
    generate_axi_helpers,
    generate_ctrl_writes,
    generate_hls_port_setup,
)


def test_generate_axi_helpers_includes_axi_services() -> None:
    lines = generate_axi_helpers([AXI("ddr", 64, 32)], "vitis")
    text = "\n".join(lines)
    assert "static void service_all_axi() {" in text
    assert "dut->m_axi_ddr_ARREADY" in text
    assert "static void ctrl_write(uint8_t addr, uint32_t data) {" in text


def test_generate_ctrl_writes_and_hls_port_setup() -> None:
    args = [
        Arg("weights", 1, 0, Port("weights", "read_only", 32)),
        Arg("scale", 0, 1, Port("scale", "read_only", 32)),
    ]
    config = {"scalar_to_val": {"scale": "'h2a"}}
    reg_addrs = {
        "weights_offset": ["'h10", "'h14"],
        "scale": ["'h20", "'h24"],
    }

    ctrl_text = "\n".join(generate_ctrl_writes(args, config, reg_addrs))
    assert "ctrl_write(0x04, 1);" in ctrl_text
    assert "ctrl_write(0x10, (uint32_t)weights_BASE);" in ctrl_text
    assert "ctrl_write(0x20, (uint32_t)0x2a);" in ctrl_text
    assert "ctrl_write(0x00, 1);" in ctrl_text

    hls_text = "\n".join(
        generate_hls_port_setup(args, [AXI("weights", 32, 32)], config)
    )
    assert "dut->weights_offset = 0x10000000ULL;" in hls_text
    assert "dut->scale = 0x2a;" in hls_text
    assert "dut->ap_start = 1;" in hls_text
