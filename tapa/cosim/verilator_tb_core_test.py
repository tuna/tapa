"""Golden tests for Verilator C++ testbench generation."""

import re
from pathlib import Path

from tapa.cosim.verilator_tb_core import generate_cpp_testbench

_FIXTURES = Path(__file__).with_name("testdata").joinpath("verilator_empty")


def _canonicalize(text: str) -> str:
    return re.sub(r"\s+", "", text)


def test_generate_cpp_testbench_matches_golden_output() -> None:
    config = {
        "top_name": "top",
        "args": [],
        "verilog_path": "",
        "mode": "hls",
        "scalar_to_val": {},
        "axi_to_data_file": {},
        "axi_to_c_array_size": {},
        "axis_to_data_file": {},
    }

    rendered = _canonicalize(
        generate_cpp_testbench("top", [], [], config, reg_addrs={}, mode="hls")
    )
    expected = _canonicalize(_FIXTURES.joinpath("tb.cpp").read_text(encoding="utf-8"))
    assert rendered == expected
