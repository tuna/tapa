"""Tests for Verilator DPI-C support generation."""

import re
from pathlib import Path

from tapa.cosim.verilator_dpi import generate_dpi_support

_FIXTURES = Path(__file__).with_name("testdata").joinpath("verilator_empty")


def _canonicalize(text: str) -> str:
    return re.sub(r"\s+", "", text)


def test_generate_dpi_support_matches_golden_output() -> None:
    rendered = _canonicalize(generate_dpi_support())
    expected = _canonicalize(
        _FIXTURES.joinpath("dpi_support.cpp").read_text(encoding="utf-8")
    )
    assert rendered == expected
