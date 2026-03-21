"""M-AXI generation helpers for upper-level task modules."""

from __future__ import annotations

from dataclasses import dataclass
from typing import TYPE_CHECKING

from pyverilog.vparser.ast import Constant, ParamArg

from tapa.backend.xilinx import M_AXI_PREFIX
from tapa.util import get_addr_width, get_indexed_name, range_or_none
from tapa.verilog.ast.logic import Assign
from tapa.verilog.ast.signal import Wire
from tapa.verilog.ast.width import Width
from tapa.verilog.ast_utils import make_port_arg
from tapa.verilog.axi_xbar import generate as axi_xbar_generate
from tapa.verilog.xilinx.const import HANDSHAKE_CLK, HANDSHAKE_RST
from tapa.verilog.xilinx.m_axi import M_AXI_PORTS, get_m_axi_port_width

if TYPE_CHECKING:
    from tapa.instance import Instance
    from tapa.task import Task


@dataclass(frozen=True)
class _MMapContext:
    task: Task
    arg_name: str
    args: tuple[Instance.Arg, ...]
    chan_count: int | None
    chan_size: int | None
    data_width: int
    m_axi_id_width: int
    s_axi_id_width: int


def _add_upstream_portargs(
    context: _MMapContext,
    width_table: dict[str, int],
    portargs: list,
) -> None:
    for idx in range_or_none(context.chan_count):
        for axi_chan, axi_ports in M_AXI_PORTS.items():
            for axi_port, _direction in axi_ports:
                name = get_indexed_name(context.arg_name, idx)
                axi_arg_name = f"{M_AXI_PREFIX}{name}_{axi_chan}{axi_port}"
                axi_arg_name_raw = axi_arg_name
                if idx is not None and axi_port == "ADDR":
                    axi_arg_name_raw += "_raw"
                    context.task.module.add_signals(
                        [Wire(name=axi_arg_name_raw, width=Width.create(64))],
                    )
                    assert context.chan_size is not None
                    addr_width = get_addr_width(
                        context.chan_size, width_table[context.arg_name]
                    )
                    context.task.module.add_logics(
                        [
                            Assign(
                                lhs=axi_arg_name,
                                rhs=f"{{{name}_offset[63:{addr_width}], "
                                f"{axi_arg_name_raw}[{addr_width - 1}:0]}}",
                            ),
                        ],
                    )
                portargs.append(
                    make_port_arg(
                        port=(
                            f"m{idx or 0:02d}_axi_{axi_chan.lower()}{axi_port.lower()}"
                        ),
                        arg=axi_arg_name_raw,
                    ),
                )


def _add_downstream_portargs(
    context: _MMapContext,
    width_table: dict[str, int],
    portargs: list,
) -> None:
    for idx, arg in enumerate(context.args):
        wires = []
        id_width = arg.instance.task.get_id_width(arg.port)
        for axi_chan, axi_ports in M_AXI_PORTS.items():
            for axi_port, direction in axi_ports:
                signal_name = f"{M_AXI_PREFIX}{arg.mmap_name}_{axi_chan}{axi_port}"
                wires.append(
                    Wire(
                        name=signal_name,
                        width=get_m_axi_port_width(
                            port=axi_port,
                            data_width=width_table[context.arg_name],
                            id_width=id_width,
                        ),
                    ),
                )
                port_arg = signal_name
                if axi_port == "ID":
                    id_width = id_width or 1
                    if id_width != context.s_axi_id_width and direction == "output":
                        port_arg = (
                            f"{{{context.s_axi_id_width - id_width}'d0, {signal_name}}}"
                        )
                portargs.append(
                    make_port_arg(
                        port=f"s{idx:02d}_axi_{axi_chan.lower()}{axi_port.lower()}",
                        arg=port_arg,
                    ),
                )
        context.task.module.add_signals(wires)


def _build_paramargs(
    context: _MMapContext,
    width_table: dict[str, int],
) -> list[ParamArg]:
    paramargs = [
        ParamArg("DATA_WIDTH", Constant(context.data_width)),
        ParamArg("ADDR_WIDTH", Constant(64)),
        ParamArg("S_ID_WIDTH", Constant(context.s_axi_id_width)),
        ParamArg("M_ID_WIDTH", Constant(context.m_axi_id_width)),
    ]
    for idx in range(context.chan_count or 1):
        addr_width = get_addr_width(context.chan_size, width_table[context.arg_name])
        paramargs.extend(
            [
                ParamArg(f"M{idx:02d}_ADDR_WIDTH", Constant(addr_width)),
                ParamArg(f"M{idx:02d}_ISSUE", Constant(16)),
            ],
        )
    paramargs.extend(
        ParamArg(
            f"S{idx:02d}_THREADS",
            Constant(arg.instance.task.get_thread_count(arg.port)),
        )
        for idx, arg in enumerate(context.args)
    )
    return paramargs


def _ensure_crossbar_file(
    files: dict[str, str],
    module_name: str,
    context: _MMapContext,
) -> None:
    if f"{module_name}.v" not in files:
        files[f"{module_name}.v"] = axi_xbar_generate(
            ports=(len(context.args), context.chan_count or 1),
            name=module_name,
        )


def add_m_axi(task: Task, width_table: dict[str, int], files: dict[str, str]) -> None:
    """Add M-AXI ports and optional crossbar instances for upper tasks."""
    for arg_name, mmap in task.mmaps.items():
        m_axi_id_width, m_axi_thread_count, args, chan_count, chan_size = mmap
        for idx in range_or_none(chan_count):
            task.module.add_m_axi(
                name=get_indexed_name(arg_name, idx),
                data_width=width_table[arg_name],
                id_width=m_axi_id_width or None,
            )
        if len(args) == 1 and chan_count is None:
            continue

        assert m_axi_id_width is not None
        assert (m_axi_thread_count > 1) == (len(args) > 1)
        s_axi_id_width = max(
            arg.instance.task.get_id_width(arg.port) or 1 for arg in args
        )
        data_width = max(width_table[arg_name], 32)
        assert data_width in {32, 64, 128, 256, 512, 1024}
        context = _MMapContext(
            task=task,
            arg_name=arg_name,
            args=args,
            chan_count=chan_count,
            chan_size=chan_size,
            data_width=data_width,
            m_axi_id_width=m_axi_id_width,
            s_axi_id_width=s_axi_id_width,
        )

        portargs = [
            make_port_arg(port="clk", arg=HANDSHAKE_CLK),
            make_port_arg(port="rst", arg=HANDSHAKE_RST),
        ]
        _add_upstream_portargs(
            context=context,
            width_table=width_table,
            portargs=portargs,
        )
        _add_downstream_portargs(
            context=context,
            width_table=width_table,
            portargs=portargs,
        )

        module_name = f"axi_crossbar_{len(args)}x{chan_count or 1}"
        _ensure_crossbar_file(files=files, module_name=module_name, context=context)
        task.module.add_instance(
            module_name=module_name,
            instance_name=f"{module_name}__{arg_name}",
            ports=portargs,
            params=_build_paramargs(
                context=context,
                width_table=width_table,
            ),
        )
