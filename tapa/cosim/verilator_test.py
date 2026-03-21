"""Golden tests for Verilator cosim artifact generation."""

import re
from pathlib import Path
from tempfile import TemporaryDirectory, mkdtemp
from unittest.mock import patch

from tapa.cosim.verilator import generate_verilator_tb

_FIXTURES = Path(__file__).with_name("testdata").joinpath("verilator_empty")


def _read_fixture(name: str) -> str:
    return _FIXTURES.joinpath(name).read_text(encoding="utf-8")


def _canonicalize(text: str) -> str:
    return re.sub(r"\s+", "", text)


def test_generate_verilator_tb_matches_golden_outputs() -> None:
    config = {
        "top_name": "top",
        "args": [],
        "verilog_path": mkdtemp(),
        "mode": "hls",
        "scalar_to_val": {},
        "axi_to_data_file": {},
        "axi_to_c_array_size": {},
        "axis_to_data_file": {},
    }

    with TemporaryDirectory() as tb_dir, TemporaryDirectory() as verilog_dir:
        config["verilog_path"] = verilog_dir
        with patch(
            "tapa.cosim.verilator._find_verilator",
            return_value=("/verilator/bin/verilator", None),
        ):
            generate_verilator_tb(config, [], tb_dir)

        rendered_tb = _canonicalize(Path(tb_dir, "tb.cpp").read_text(encoding="utf-8"))
        expected_tb = _canonicalize(_read_fixture("tb.cpp"))
        assert rendered_tb == expected_tb
        rendered_dpi = _canonicalize(
            Path(tb_dir, "dpi_support.cpp").read_text(encoding="utf-8")
        )
        expected_dpi = _canonicalize(_read_fixture("dpi_support.cpp"))
        assert rendered_dpi == expected_dpi
        assert Path(tb_dir, "build.sh").read_text(encoding="utf-8") == _read_fixture(
            "build.sh"
        )
