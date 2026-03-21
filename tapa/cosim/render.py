"""Jinja2-backed rendering helpers for TAPA fast cosim artifacts."""

from __future__ import annotations

import logging
import os
import sys
from pathlib import Path
from typing import TYPE_CHECKING, Literal

from jinja2 import Environment, FileSystemLoader
from pydantic import BaseModel

from tapa.cosim.common import AXI, MAX_AXI_BRAM_ADDR_WIDTH, Arg

if TYPE_CHECKING:
    from collections.abc import Sequence

_logger = logging.getLogger().getChild(__name__)
_ASSETS_DIR = Path(__file__).with_name("assets")
_JINJA_ENV = Environment(
    loader=FileSystemLoader(_ASSETS_DIR),
    autoescape=False,
    keep_trailing_newline=True,
)


class AxiRamInstContext(BaseModel):
    """Context for rendering the AXI RAM instance snippet."""

    name: str
    upper_name: str
    data_width: int
    max_addr_width: int = MAX_AXI_BRAM_ADDR_WIDTH

    @classmethod
    def from_axi(cls, axi: AXI) -> AxiRamInstContext:
        return cls(
            name=axi.name,
            upper_name=axi.name.upper(),
            data_width=axi.data_width,
        )


class StreamArgContext(BaseModel):
    """Context for stream-oriented signal rendering."""

    name: str
    qualified_name: str
    data_width: int
    direction: Literal["istream", "ostream"]

    @classmethod
    def from_arg(cls, arg: Arg) -> StreamArgContext:
        if arg.port.is_istream:
            direction: Literal["istream", "ostream"] = "istream"
        elif arg.port.is_ostream:
            direction = "ostream"
        else:
            msg = f"unexpected arg.port.mode: {arg.port.mode}"
            raise ValueError(msg)
        return cls(
            name=arg.name,
            qualified_name=arg.qualified_name,
            data_width=arg.port.data_width,
            direction=direction,
        )


class VitisRegisterWriteContext(BaseModel):
    """A single scalar register write sequence for Vitis-mode cosim."""

    address: str
    value: str
    upper_word: bool = False


class VitisTestSignalsContext(BaseModel):
    """Context for rendering Vitis testbench signal logic."""

    stream_args: list[StreamArgContext]
    register_writes: list[VitisRegisterWriteContext]
    mmap_names: list[str]


class HlsTestSignalsContext(BaseModel):
    """Context for rendering HLS testbench signal logic."""

    stream_args: list[StreamArgContext]
    mmap_names: list[str]


class AxiRamModuleContext(BaseModel):
    """Context for rendering the AXI RAM behavioral module."""

    name: str
    data_width: int
    max_addr_width: int
    input_data_path: str
    output_data_path: str
    c_array_size: int

    @classmethod
    def from_axi(
        cls,
        axi: AXI,
        input_data_path: str,
        c_array_size: int,
    ) -> AxiRamModuleContext:
        return cls(
            name=axi.name,
            data_width=axi.data_width,
            max_addr_width=MAX_AXI_BRAM_ADDR_WIDTH,
            input_data_path=input_data_path,
            output_data_path=input_data_path.replace(".bin", "_out.bin"),
            c_array_size=c_array_size,
        )


def _render_template(template_name: str, **kwargs: object) -> str:
    return _JINJA_ENV.get_template(template_name).render(**kwargs)


def render_axi_ram_inst(axi: AXI) -> str:
    """Render the AXI RAM instance snippet."""
    return _render_template(
        "axi_ram_inst.j2",
        ctx=AxiRamInstContext.from_axi(axi),
    )


def render_testbench_begin() -> str:
    """Render the shared testbench prologue."""
    return _render_template("testbench_begin.j2")


def render_testbench_end() -> str:
    """Render the shared testbench epilogue."""
    return _render_template("testbench_end.j2")


def _get_axis_dpi_calls(stream_args: list[StreamArgContext]) -> list[str]:
    axis_dpi_calls = []
    for arg in stream_args:
        if arg.direction == "istream":
            axis_dpi_calls.append(
                f"""
    tapa::istream(
        axis_{arg.name}_tdata_unpacked_next,
        axis_{arg.name}_tvalid_next,
        axis_{arg.name}_tready,
        "{arg.qualified_name}"
    );

    axis_{arg.name}_tdata_unpacked <= axis_{arg.name}_tdata_unpacked_next;
    axis_{arg.name}_tvalid <= axis_{arg.name}_tvalid_next;
"""
            )
        else:
            axis_dpi_calls.append(
                f"""
    tapa::ostream(
        axis_{arg.name}_tdata_unpacked,
        axis_{arg.name}_tready_next,
        axis_{arg.name}_tvalid,
        "{arg.qualified_name}"
    );

    axis_{arg.name}_tready <= axis_{arg.name}_tready_next;
"""
            )
    return axis_dpi_calls


def _get_axis_assignments(stream_args: list[StreamArgContext]) -> list[str]:
    assignments = []
    for arg in stream_args:
        if arg.direction == "istream":
            assignments.append(
                f"""
    assign {{axis_{arg.name}_tlast, axis_{arg.name}_tdata}} =
        packed_uint{arg.data_width + 1}_t'(axis_{arg.name}_tdata_unpacked);
"""
            )
        else:
            assignments.append(
                f"""
    assign axis_{arg.name}_tdata_unpacked =
        unpacked_uint{arg.data_width + 1}_t'
            ({{axis_{arg.name}_tlast, axis_{arg.name}_tdata}});
"""
            )
    return assignments


def _get_fifo_dpi_calls(stream_args: list[StreamArgContext]) -> list[str]:
    fifo_dpi_calls = []
    for arg in stream_args:
        if arg.direction == "istream":
            fifo_dpi_calls.append(
                f"""
    tapa::istream(
        fifo_{arg.qualified_name}_data_unpacked_next,
        fifo_{arg.qualified_name}_valid_next,
        fifo_{arg.qualified_name}_ready,
        "{arg.qualified_name}"
    );

    fifo_{arg.qualified_name}_data_unpacked <=
        fifo_{arg.qualified_name}_data_unpacked_next;
    fifo_{arg.qualified_name}_valid <= fifo_{arg.qualified_name}_valid_next;
"""
            )
        else:
            fifo_dpi_calls.append(
                f"""
    tapa::ostream(
        fifo_{arg.qualified_name}_data_unpacked,
        fifo_{arg.qualified_name}_ready_next,
        fifo_{arg.qualified_name}_valid,
        "{arg.qualified_name}"
    );

    fifo_{arg.qualified_name}_ready <= fifo_{arg.qualified_name}_ready_next;
"""
            )
    return fifo_dpi_calls


def _get_fifo_assignments(stream_args: list[StreamArgContext]) -> list[str]:
    assignments = []
    for arg in stream_args:
        if arg.direction == "istream":
            assignments.append(
                f"""
    assign fifo_{arg.qualified_name}_data =
        packed_uint{arg.data_width + 1}_t'(
            fifo_{arg.qualified_name}_data_unpacked);
"""
            )
        else:
            assignments.append(
                f"""
    assign fifo_{arg.qualified_name}_data_unpacked =
        unpacked_uint{arg.data_width + 1}_t'(fifo_{arg.qualified_name}_data);
"""
            )
    return assignments


def render_vitis_test_signals(
    arg_to_reg_addrs: dict[str, list[str]],
    scalar_arg_to_val: dict[str, str],
    args: list[Arg] | tuple[Arg, ...] | Sequence[Arg],
) -> str:
    """Render the Vitis test-signal block."""
    stream_args = [StreamArgContext.from_arg(arg) for arg in args if arg.is_stream]
    register_writes = []
    for arg_name, addrs in arg_to_reg_addrs.items():
        value = str(scalar_arg_to_val.get(arg_name, 0))
        register_writes.append(VitisRegisterWriteContext(address=addrs[0], value=value))
        if len(addrs) == 2:  # noqa: PLR2004
            register_writes.append(
                VitisRegisterWriteContext(
                    address=addrs[1],
                    value=value,
                    upper_word=True,
                )
            )
    ctx = VitisTestSignalsContext(
        stream_args=stream_args,
        register_writes=register_writes,
        mmap_names=[arg.name for arg in args if arg.is_mmap],
    )
    return _render_template(
        "vitis_test_signals.j2",
        axis_dpi_calls="\n".join(_get_axis_dpi_calls(ctx.stream_args)),
        axis_assignments="\n".join(_get_axis_assignments(ctx.stream_args)),
        register_writes=ctx.register_writes,
        dump_signals="\n".join(
            f"          axi_ram_{name}_dump_mem <= 1;" for name in ctx.mmap_names
        ),
    )


def render_hls_test_signals(args: list[Arg] | tuple[Arg, ...] | Sequence[Arg]) -> str:
    """Render the HLS test-signal block."""
    ctx = HlsTestSignalsContext(
        stream_args=[StreamArgContext.from_arg(arg) for arg in args if arg.is_stream],
        mmap_names=[arg.name for arg in args if arg.is_mmap],
    )
    return _render_template(
        "hls_test_signals.j2",
        fifo_dpi_calls="\n".join(_get_fifo_dpi_calls(ctx.stream_args)),
        fifo_assignments="\n".join(_get_fifo_assignments(ctx.stream_args)),
        dump_signals="\n".join(
            f"    axi_ram_{name}_dump_mem <= 1;" for name in ctx.mmap_names
        ),
    )


def render_axi_ram_module(axi: AXI, input_data_path: str, c_array_size: int) -> str:
    """Render the AXI RAM behavioral module."""
    if axi.data_width / 8 * c_array_size > 2**MAX_AXI_BRAM_ADDR_WIDTH:
        _logger.error(
            "The current cosim data size is larger than the template "
            "threshold (32-bit address). Please reduce cosim data size."
        )
        sys.exit(1)

    if input_data_path:
        assert os.path.exists(input_data_path)

    return _render_template(
        "axi_ram_module.j2",
        ctx=AxiRamModuleContext.from_axi(axi, input_data_path, c_array_size),
    )
