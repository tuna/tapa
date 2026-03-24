"""Jinja2-backed rendering helpers for TAPA fast cosim artifacts."""

from __future__ import annotations

import logging
import os
import sys
from pathlib import Path
from typing import TYPE_CHECKING, Literal

from jinja2 import Environment, FileSystemLoader
from pydantic import BaseModel, Field, computed_field

from tapa.cosim.common import AXI, MAX_AXI_BRAM_ADDR_WIDTH, Arg, output_data_path

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
    data_width: int
    max_addr_width: int = MAX_AXI_BRAM_ADDR_WIDTH

    @computed_field
    @property
    def upper_name(self) -> str:
        return self.name.upper()

    @classmethod
    def from_axi(cls, axi: AXI) -> AxiRamInstContext:
        return cls(name=axi.name, data_width=axi.data_width)


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
            output_data_path=output_data_path(input_data_path),
            c_array_size=c_array_size,
        )


class DutContext(BaseModel):
    """Context for rendering DUT connection snippets."""

    top_name: str | None = None
    args: list[Arg]
    scalar_to_val: dict[str, str] | None = None
    top_is_leaf_task: bool | None = None
    stream_widths: list[int] = Field(default_factory=list)


def _render_template(template_name: str, **kwargs: object) -> str:
    try:
        return _JINJA_ENV.get_template(template_name).render(**kwargs)
    except Exception as exc:
        msg = f"cosim render failed for {template_name}: {exc}"
        raise RuntimeError(msg) from exc


def render_axi_ram_inst(axi: AXI) -> str:
    return _render_template(
        "axi_ram_inst.j2",
        ctx=AxiRamInstContext.from_axi(axi),
    )


def render_testbench_begin() -> str:
    return _render_template("testbench_begin.j2")


def render_testbench_end() -> str:
    return _render_template("testbench_end.j2")


def render_vitis_test_signals(
    arg_to_reg_addrs: dict[str, list[str]],
    scalar_arg_to_val: dict[str, str],
    args: Sequence[Arg],
) -> str:
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
        ctx=ctx,
    )


def render_hls_test_signals(args: Sequence[Arg]) -> str:
    ctx = HlsTestSignalsContext(
        stream_args=[StreamArgContext.from_arg(arg) for arg in args if arg.is_stream],
        mmap_names=[arg.name for arg in args if arg.is_mmap],
    )
    return _render_template(
        "hls_test_signals.j2",
        ctx=ctx,
    )


def render_s_axi_control() -> str:
    return _render_template("s_axi_control.j2")


def _axis_stream_widths(args: Sequence[Arg]) -> list[int]:
    """Return sorted unique data widths (both w and w+1) across all stream args."""
    stream_args = [arg for arg in args if arg.is_stream]
    return sorted(
        {
            width
            for arg in stream_args
            for width in (arg.port.data_width, arg.port.data_width + 1)
        }
    )


def render_axis(args: Sequence[Arg]) -> str:
    ctx = DutContext(args=list(args), stream_widths=_axis_stream_widths(args))
    return _render_template("axis.j2", ctx=ctx)


def render_stream_typedef(args: Sequence[Arg]) -> str:
    ctx = DutContext(args=[], stream_widths=_axis_stream_widths(args))
    return _render_template("stream_typedef.j2", ctx=ctx)


def render_fifo(args: Sequence[Arg]) -> str:
    stream_args = [arg for arg in args if arg.is_stream]
    ctx = DutContext(
        args=list(args),
        stream_widths=sorted({arg.port.data_width + 1 for arg in stream_args}),
    )
    return _render_template("fifo.j2", ctx=ctx)


def render_m_axi_connections(arg_name: str) -> str:
    return _render_template("m_axi_connections.j2", arg_name=arg_name)


def render_vitis_dut(
    top_name: str,
    args: Sequence[Arg],
) -> str:
    ctx = DutContext(top_name=top_name, args=list(args))
    return _render_template("vitis_dut.j2", ctx=ctx)


def render_hls_dut(
    top_name: str,
    top_is_leaf_task: bool,
    args: Sequence[Arg],
    scalar_to_val: dict[str, str],
) -> str:
    ctx = DutContext(
        top_name=top_name,
        args=list(args),
        scalar_to_val=scalar_to_val,
        top_is_leaf_task=top_is_leaf_task,
    )
    return _render_template("hls_dut.j2", ctx=ctx)


def render_srl_fifo_template() -> str:
    return _render_template("srl_fifo_template.j2")


def render_axi_ram_module(axi: AXI, input_data_path: str, c_array_size: int) -> str:
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
