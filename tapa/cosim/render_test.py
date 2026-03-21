"""Tests for Jinja2-backed cosim renderers."""

from pathlib import Path

import tapa.cosim.render as render_module
from tapa.cosim.common import AXI, Arg, Port
from tapa.cosim.render import (
    render_axi_ram_inst,
    render_axi_ram_module,
    render_hls_test_signals,
    render_m_axi_connections,
    render_stream_typedef,
    render_testbench_begin,
    render_testbench_end,
    render_vitis_test_signals,
)
from tapa.cosim.templates import (
    get_axi_ram_inst,
    get_axi_ram_module,
    get_begin,
    get_end,
    get_hls_test_signals,
    get_m_axi_connections,
    get_stream_typedef,
    get_vitis_test_signals,
)


def _sample_args() -> tuple[Arg, ...]:
    return (
        Arg("a", 4, 0, Port("a", "read_only", 32)),
        Arg("b", 4, 1, Port("b", "write_only", 64)),
        Arg("mem", 1, 2, Port("mem", "read_only", 128)),
    )


def test_axi_ram_inst_wrapper_matches_renderer() -> None:
    axi = AXI("gmem", 64, 32)

    rendered = render_axi_ram_inst(axi)

    assert get_axi_ram_inst(axi) == rendered
    assert "parameter AXI_RAM_GMEM_DATA_WIDTH = 64;" in rendered
    assert "axi_ram_gmem_unit" in rendered


def test_axi_ram_module_wrapper_matches_renderer(tmp_path: Path) -> None:
    axi = AXI("mem", 32, 32)
    input_path = tmp_path / "mem.bin"
    input_path.write_bytes(b"\x00" * 16)

    rendered = render_axi_ram_module(axi, str(input_path), 4)

    assert get_axi_ram_module(axi, str(input_path), 4) == rendered
    assert "module axi_ram_mem #" in rendered
    assert f'$fopen("{input_path}", "rb");' in rendered
    assert f'$fopen("{input_path.with_name("mem_out.bin")}", "wb");' in rendered


def test_vitis_test_signals_wrapper_matches_renderer() -> None:
    args = _sample_args()
    rendered = render_vitis_test_signals(
        {"scalar": ["'h10", "'h14"]},
        {"scalar": "123456789"},
        args,
    )

    assert (
        get_vitis_test_signals(
            {"scalar": ["'h10", "'h14"]},
            {"scalar": "123456789"},
            args,
        )
        == rendered
    )
    assert "tapa::istream(" in rendered
    assert "tapa::ostream(" in rendered
    assert "s_axi_aw_din <= 'h10;" in rendered
    assert "s_axi_aw_din <= 'h14;" in rendered
    assert "axi_ram_mem_dump_mem <= 1;" in rendered


def test_hls_test_signals_wrapper_matches_renderer() -> None:
    args = _sample_args()

    rendered = render_hls_test_signals(args)

    assert get_hls_test_signals(args) == rendered
    assert "wait(kernel_done);" in rendered
    assert "fifo_a_s_data_unpacked_next" in rendered
    assert "fifo_b_s_ready_next" in rendered
    assert "axi_ram_mem_dump_mem <= 1;" in rendered


def test_testbench_frame_wrappers_match_renderer() -> None:
    assert get_begin() == render_testbench_begin()
    assert get_end() == render_testbench_end()
    assert "module test();" in get_begin()
    assert "endmodule" in get_end()
    assert "wire [31:0] REG_MASK_32_BIT = {{32{{1'b1}}}};" in get_begin()


def test_stream_typedef_wrapper_matches_renderer() -> None:
    args = _sample_args()

    rendered = render_stream_typedef(args)

    assert get_stream_typedef(args) == rendered
    assert "typedef logic unpacked_uint32_t[31:0];" in rendered
    assert "typedef logic [64:0] packed_uint65_t;" in rendered


def test_m_axi_connections_wrapper_matches_renderer() -> None:
    rendered = render_m_axi_connections("mem")

    assert get_m_axi_connections("mem") == rendered
    assert ".m_axi_mem_ARADDR  (axi_mem_araddr )," in rendered
    assert ".m_axi_mem_WVALID  (axi_mem_wvalid )," in rendered


def test_render_template_failure_is_wrapped() -> None:
    render_template = getattr(render_module, "_render_template")

    message = None
    try:
        render_template("missing_template.j2")
    except RuntimeError as exc:
        message = str(exc)
    assert message is not None
    assert "cosim render failed for missing_template.j2" in message
