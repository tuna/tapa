"""Tests for Jinja2-backed cosim renderers."""

from pathlib import Path

import tapa.cosim.render as render_module
from tapa.cosim.common import AXI, Arg, Port
from tapa.cosim.render import (
    render_axi_ram_inst,
    render_axi_ram_module,
    render_hls_dut,
    render_hls_test_signals,
    render_m_axi_connections,
    render_stream_typedef,
    render_testbench_begin,
    render_testbench_end,
    render_vitis_dut,
    render_vitis_test_signals,
)
from tapa.cosim.templates import (
    get_axi_ram_inst,
    get_axi_ram_module,
    get_begin,
    get_end,
    get_hls_dut,
    get_hls_test_signals,
    get_m_axi_connections,
    get_stream_typedef,
    get_vitis_dut,
    get_vitis_test_signals,
)

_FIXTURES = Path(__file__).with_name("testdata")


def _sample_args() -> tuple[Arg, ...]:
    return (
        Arg("a", 4, 0, Port("a", "read_only", 32)),
        Arg("b", 4, 1, Port("b", "write_only", 64)),
        Arg("mem", 1, 2, Port("mem", "read_only", 128)),
    )


def _read_fixture(*parts: str) -> str:
    return _FIXTURES.joinpath(*parts).read_text(encoding="utf-8")


def test_axi_ram_inst_renderer_smoke() -> None:
    axi = AXI("gmem", 64, 32)

    rendered = render_axi_ram_inst(axi)

    assert "parameter AXI_RAM_GMEM_DATA_WIDTH = 64;" in get_axi_ram_inst(axi)
    assert "parameter AXI_RAM_GMEM_DATA_WIDTH = 64;" in rendered
    assert "axi_ram_gmem_unit" in rendered


def test_axi_ram_module_renderer_smoke(tmp_path: Path) -> None:
    axi = AXI("mem", 32, 32)
    input_path = tmp_path / "mem.bin"
    input_path.write_bytes(b"\x00" * 16)

    rendered = render_axi_ram_module(axi, str(input_path), 4)

    assert "module axi_ram_mem #" in get_axi_ram_module(axi, str(input_path), 4)
    assert "module axi_ram_mem #" in rendered
    assert f'$fopen("{input_path}", "rb");' in rendered
    assert f'$fopen("{input_path.with_name("mem_out.bin")}", "wb");' in rendered


def test_vitis_test_signals_matches_fixture() -> None:
    args = _sample_args()

    rendered = render_vitis_test_signals(
        {"scalar": ["'h10", "'h14"]},
        {"scalar": "123456789"},
        args,
    )
    expected = _read_fixture("render", "vitis_test_signals.txt")

    assert rendered == expected
    assert (
        get_vitis_test_signals(
            {"scalar": ["'h10", "'h14"]},
            {"scalar": "123456789"},
            args,
        )
        == expected
    )


def test_hls_test_signals_matches_fixture() -> None:
    args = _sample_args()

    rendered = render_hls_test_signals(args)
    expected = _read_fixture("render", "hls_test_signals.txt")

    assert rendered == expected
    assert get_hls_test_signals(args) == expected


def test_testbench_frame_matches_fixture() -> None:
    expected = _read_fixture("render", "testbench_begin.txt")

    assert render_testbench_begin() == expected
    assert get_begin() == expected
    assert render_testbench_end() == _read_fixture("render", "testbench_end.txt")
    assert get_end() == render_testbench_end()


def test_stream_typedef_matches_fixture() -> None:
    args = _sample_args()
    expected = _read_fixture("render", "stream_typedef.txt").rstrip("\n")

    assert render_stream_typedef(args).rstrip("\n") == expected
    assert get_stream_typedef(args).rstrip("\n") == expected


def test_m_axi_connections_matches_fixture() -> None:
    expected = _read_fixture("render", "m_axi_connections.txt")

    assert render_m_axi_connections("mem") == expected
    assert get_m_axi_connections("mem") == expected


def test_vitis_dut_renderer_generates_mmap_connections() -> None:
    args = (
        Arg("a", 1, 0, Port("a", "read_write", 512)),
        Arg("b", 1, 1, Port("b", "read_write", 512)),
        Arg("c", 1, 2, Port("c", "read_write", 512)),
        Arg("n", 0, 3, Port("n", "read_only", 64)),
    )

    rendered = render_vitis_dut("VecAdd", args)

    assert "m_axi_a_ARADDR" in rendered
    assert "m_axi_b_ARADDR" in rendered
    assert "m_axi_c_ARADDR" in rendered
    assert rendered == get_vitis_dut("VecAdd", args)


def test_hls_dut_renderer_generates_mmap_connections() -> None:
    args = (
        Arg("a", 1, 0, Port("a", "read_write", 512)),
        Arg("b", 1, 1, Port("b", "read_write", 512)),
        Arg("c", 1, 2, Port("c", "read_write", 512)),
        Arg("n", 0, 3, Port("n", "read_only", 64)),
    )

    rendered = render_hls_dut("VecAdd", False, args, {"n": "'h3e8"})

    assert "m_axi_a_ARADDR" in rendered
    assert "m_axi_b_ARADDR" in rendered
    assert "m_axi_c_ARADDR" in rendered
    assert rendered == get_hls_dut("VecAdd", False, args, {"n": "'h3e8"})


def test_render_template_failure_is_wrapped() -> None:
    render_template = getattr(render_module, "_render_template")

    message = None
    try:
        render_template("missing_template.j2")
    except RuntimeError as exc:
        message = str(exc)
    assert message is not None
    assert "cosim render failed for missing_template.j2" in message
