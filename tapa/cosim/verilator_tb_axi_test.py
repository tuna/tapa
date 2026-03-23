"""Tests for Verilator TB AXI helper generation."""

from tapa.cosim.common import AXI
from tapa.cosim.verilator_tb_axi import generate_axi_helpers


def test_generate_axi_helpers_includes_axi_services() -> None:
    lines = generate_axi_helpers([AXI("ddr", 64, 32)], "vitis")
    text = "\n".join(lines)
    assert "static void service_all_axi() {" in text
    assert "dut->m_axi_ddr_ARREADY" in text
    assert "static void ctrl_write(uint8_t addr, uint32_t data) {" in text
