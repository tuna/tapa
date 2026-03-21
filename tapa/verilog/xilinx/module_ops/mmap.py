"""Async mmap instantiation helpers for :mod:`tapa.verilog.xilinx.module`."""

from __future__ import annotations

from dataclasses import dataclass
from typing import TYPE_CHECKING

from pyverilog.vparser.ast import Constant, Node, ParamArg

from tapa.backend.xilinx import M_AXI_PREFIX
from tapa.verilog.ast_utils import make_port_arg
from tapa.verilog.util import async_mmap_instance_name
from tapa.verilog.xilinx.async_mmap import ASYNC_MMAP_SUFFIXES, async_mmap_arg_name
from tapa.verilog.xilinx.const import CLK
from tapa.verilog.xilinx.m_axi import M_AXI_PORTS

if TYPE_CHECKING:
    from tapa.verilog.xilinx.module import Module


@dataclass(frozen=True)
class _AsyncMmapContext:
    module: Module
    name: str
    tags: tuple[str, ...]
    rst: Node
    data_width: int
    addr_width: int = 64
    buffer_size: int | None = None
    max_wait_time: int = 3
    max_burst_len: int | None = None


def add_async_mmap_instance(context: _AsyncMmapContext) -> Module:
    module = context.module
    paramargs = [
        ParamArg(paramname="DataWidth", argname=Constant(context.data_width)),
        ParamArg(
            paramname="DataWidthBytesLog",
            argname=Constant((context.data_width // 8 - 1).bit_length()),
        ),
    ]
    portargs = [
        make_port_arg(port="clk", arg=CLK),
        make_port_arg(port="rst", arg=context.rst),
    ]
    paramargs.append(
        ParamArg(paramname="AddrWidth", argname=Constant(context.addr_width))
    )
    if context.buffer_size:
        paramargs.extend(
            (
                ParamArg(paramname="BufferSize", argname=Constant(context.buffer_size)),
                ParamArg(
                    paramname="BufferSizeLog",
                    argname=Constant((context.buffer_size - 1).bit_length()),
                ),
            ),
        )

    max_wait_time = max(1, context.max_wait_time)
    paramargs.extend(
        (
            ParamArg(
                paramname="WaitTimeWidth",
                argname=Constant(max_wait_time.bit_length()),
            ),
            ParamArg(
                paramname="MaxWaitTime",
                argname=Constant(max(1, max_wait_time)),
            ),
        ),
    )

    max_burst_len = context.max_burst_len
    if max_burst_len is None:
        # 1KB burst length
        max_burst_len = max(0, 8192 // context.data_width - 1)
    paramargs.extend(
        (
            ParamArg(paramname="BurstLenWidth", argname=Constant(9)),
            ParamArg(paramname="MaxBurstLen", argname=Constant(max_burst_len)),
        ),
    )

    for channel, ports in M_AXI_PORTS.items():
        for port, _direction in ports:
            portargs.append(
                make_port_arg(
                    port=f"{M_AXI_PREFIX}{channel}{port}",
                    arg=f"{M_AXI_PREFIX}{context.name}_{channel}{port}",
                ),
            )

    tags = set(context.tags)
    for tag in ASYNC_MMAP_SUFFIXES:
        for suffix in ASYNC_MMAP_SUFFIXES[tag]:
            if tag in tags:
                arg = async_mmap_arg_name(arg=context.name, tag=tag, suffix=suffix)
            elif suffix.endswith(("_read", "_write")):
                arg = "1'b0"
            elif suffix.endswith("_din"):
                arg = "'d0"
            else:
                arg = ""
            portargs.append(make_port_arg(port=tag + suffix, arg=arg))

    return module.add_instance(
        module_name="async_mmap",
        instance_name=async_mmap_instance_name(context.name),
        ports=portargs,
        params=paramargs,
    )
