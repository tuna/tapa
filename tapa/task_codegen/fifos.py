"""FIFO wiring helpers for tasks."""

from __future__ import annotations

from typing import TYPE_CHECKING

from pyverilog.vparser.ast import IntConst, ParamArg

from tapa.protocol import (
    HANDSHAKE_CLK,
    ISTREAM_SUFFIXES,
    OSTREAM_SUFFIXES,
    STREAM_PORT_DIRECTION,
)
from tapa.verilog.ast.logic import Assign
from tapa.verilog.ast_utils import make_port_arg
from tapa.verilog.util import sanitize_array_name, wire_name
from tapa.verilog.xilinx.axis import (
    AXIS_CONSTANTS,
    get_axis_port_width_int,
)
from tapa.verilog.xilinx.const import RST

if TYPE_CHECKING:
    from tapa.task import Task

_DIR2CAT = {"produced_by": "ostream", "consumed_by": "istream"}


def get_connection_to(
    task: Task,
    fifo_name: str,
    direction: str,
) -> tuple[str, int, str]:
    """Get the port a FIFO is connected to in a task."""
    if direction not in _DIR2CAT:
        msg = f"invalid direction: {direction}"
        raise ValueError(msg)
    if direction not in task.fifos[fifo_name]:
        msg = f"{fifo_name} is not {direction} any task"
        raise ValueError(msg)
    task_name, task_idx = task.fifos[fifo_name][direction]
    for port, arg in task.tasks[task_name][task_idx]["args"].items():
        if arg["cat"] == _DIR2CAT[direction] and arg["arg"] == fifo_name:
            return task_name, task_idx, port
    msg = f"task {task.name} has inconsistent metadata"
    raise ValueError(msg)


def get_fifo_directions(task: Task, fifo_name: str) -> list[str]:
    """Return the directions recorded for a FIFO."""
    fifo = task.fifos[fifo_name]
    return [d for d in ("consumed_by", "produced_by") if d in fifo]


_DIR2SUFFIXES = {
    "consumed_by": ISTREAM_SUFFIXES,
    "produced_by": OSTREAM_SUFFIXES,
}


def get_fifo_suffixes(direction: str) -> list[str]:
    """Return the suffixes associated with a FIFO direction."""
    return list(_DIR2SUFFIXES[direction])


def is_fifo_external(task: Task, fifo_name: str) -> bool:
    """Return whether a FIFO is externally connected."""
    return "depth" not in task.fifos[fifo_name]


def _assign_directional(task: Task, a: str, b: str, a_direction: str) -> None:
    if a_direction == "input":
        task.module.add_logics([Assign(lhs=a, rhs=b)])
    elif a_direction == "output":
        task.module.add_logics([Assign(lhs=b, rhs=a)])


def _find_axis_port_name(task: Task, axis_name: str, suffix: str) -> str:
    port_name = task.module.find_port(axis_name, suffix)
    assert port_name is not None
    return port_name


def convert_axis_to_fifo(task: Task, axis_name: str) -> str:
    """Convert an AXIS port into a dedicated AXIS-stream adapter."""
    directions = get_fifo_directions(task, axis_name)
    assert len(directions) == 1, "axis interfaces should have one direction"
    direction = directions[0]
    data_width = task.ports[axis_name].width

    adapter_name = f"tapa_axis_{sanitize_array_name(axis_name)}"
    ports = [
        make_port_arg("clk", HANDSHAKE_CLK),
        make_port_arg("reset", RST),
    ]

    if direction == "consumed_by":
        task.module.add_instance(
            module_name="axis_to_stream_adapter",
            instance_name=adapter_name,
            params=(ParamArg("DATA_WIDTH", IntConst(str(data_width))),),
            ports=(
                *ports,
                make_port_arg(
                    "s_axis_tdata", _find_axis_port_name(task, axis_name, "TDATA")
                ),
                make_port_arg(
                    "s_axis_tvalid", _find_axis_port_name(task, axis_name, "TVALID")
                ),
                make_port_arg(
                    "s_axis_tready", _find_axis_port_name(task, axis_name, "TREADY")
                ),
                make_port_arg(
                    "s_axis_tlast", _find_axis_port_name(task, axis_name, "TLAST")
                ),
                make_port_arg("m_stream_dout", wire_name(axis_name, "_dout")),
                make_port_arg("m_stream_empty_n", wire_name(axis_name, "_empty_n")),
                make_port_arg("m_stream_read", wire_name(axis_name, "_read")),
            ),
        )
    else:
        task.module.add_instance(
            module_name="stream_to_axis_adapter",
            instance_name=adapter_name,
            params=(ParamArg("DATA_WIDTH", IntConst(str(data_width))),),
            ports=(
                *ports,
                make_port_arg("s_stream_din", wire_name(axis_name, "_din")),
                make_port_arg("s_stream_full_n", wire_name(axis_name, "_full_n")),
                make_port_arg("s_stream_write", wire_name(axis_name, "_write")),
                make_port_arg(
                    "m_axis_tdata", _find_axis_port_name(task, axis_name, "TDATA")
                ),
                make_port_arg(
                    "m_axis_tvalid", _find_axis_port_name(task, axis_name, "TVALID")
                ),
                make_port_arg(
                    "m_axis_tready", _find_axis_port_name(task, axis_name, "TREADY")
                ),
                make_port_arg(
                    "m_axis_tlast", _find_axis_port_name(task, axis_name, "TLAST")
                ),
            ),
        )

        for axis_suffix, bit in AXIS_CONSTANTS.items():
            port_name = _find_axis_port_name(task, axis_name, axis_suffix)
            width = get_axis_port_width_int(axis_suffix, data_width)
            task.module.add_logics(
                [
                    Assign(
                        lhs=port_name,
                        rhs=f"{width}'b{str(bit) * width}",
                    ),
                ],
            )

    return adapter_name


def connect_fifo_externally(task: Task, internal_name: str, axis: bool) -> None:
    """Connect a FIFO either to external ports or to a FIFO wrapper."""
    directions = get_fifo_directions(task, internal_name)
    assert len(directions) == 1, "externally connected fifos should have one direction"
    direction = directions[0]
    if axis:
        convert_axis_to_fifo(task, internal_name)
        return
    external_name = internal_name

    for suffix in get_fifo_suffixes(direction):
        if external_name == internal_name:
            rhs = task.module.get_port_of(external_name, suffix).name
        else:
            rhs = wire_name(external_name, suffix)
        _assign_directional(
            task,
            wire_name(internal_name, suffix),
            rhs,
            STREAM_PORT_DIRECTION[suffix],
        )
