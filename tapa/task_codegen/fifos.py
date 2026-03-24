"""FIFO wiring helpers for tasks."""

from __future__ import annotations

from typing import TYPE_CHECKING

from pyverilog.vparser.ast import IntConst, Plus

from tapa.verilog.ast.logic import Assign
from tapa.verilog.ast.signal import Wire
from tapa.verilog.util import wire_name
from tapa.verilog.xilinx.axis import (
    AXIS_CONSTANTS,
    STREAM_TO_AXIS,
    get_axis_port_width_int,
)
from tapa.verilog.xilinx.const import (
    ISTREAM_SUFFIXES,
    OSTREAM_SUFFIXES,
    RST,
    STREAM_PORT_DIRECTION,
    get_stream_width,
)

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
    return [
        direction
        for direction in ["consumed_by", "produced_by"]
        if direction in task.fifos[fifo_name]
    ]


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


def convert_axis_to_fifo(task: Task, axis_name: str) -> str:
    """Convert an AXIS port into a registered FIFO."""
    directions = get_fifo_directions(task, axis_name)
    assert len(directions) == 1, "axis interfaces should have one direction"
    direction_axis = {
        "consumed_by": "produced_by",
        "produced_by": "consumed_by",
    }[directions[0]]
    data_width = task.ports[axis_name].width

    fifo_name = "tapa_fifo_" + axis_name
    task.module.add_fifo_instance(
        name=fifo_name,
        rst=RST,
        width=Plus(IntConst(data_width), IntConst(1)),
        depth=2,
    )

    for suffix in STREAM_PORT_DIRECTION:
        w_name = wire_name(fifo_name, suffix)
        wire_width = get_stream_width(suffix, data_width)
        task.module.add_signals([Wire(name=w_name, width=wire_width)])

    if direction_axis == "consumed_by":
        for axis_suffix, bit in AXIS_CONSTANTS.items():
            port_name = task.module.find_port(axis_name, axis_suffix)
            assert port_name is not None
            width = get_axis_port_width_int(axis_suffix, data_width)
            task.module.add_logics(
                [
                    Assign(
                        lhs=port_name,
                        rhs=f"{width}'b{str(bit) * width}",
                    ),
                ],
            )

    for suffix in get_fifo_suffixes(direction_axis):
        w_name = wire_name(fifo_name, suffix)
        offset = 0
        for axis_suffix in STREAM_TO_AXIS[suffix]:
            port_name = task.module.find_port(axis_name, axis_suffix)
            assert port_name is not None
            width = get_axis_port_width_int(axis_suffix, data_width)
            if len(STREAM_TO_AXIS[suffix]) > 1:
                wire = f"{w_name}[{offset + width - 1}:{offset}]"
            else:
                wire = w_name
            _assign_directional(
                task,
                port_name,
                wire,
                STREAM_PORT_DIRECTION[suffix],
            )
            offset += width

    return fifo_name


def connect_fifo_externally(task: Task, internal_name: str, axis: bool) -> None:
    """Connect a FIFO either to external ports or to a FIFO wrapper."""
    directions = get_fifo_directions(task, internal_name)
    assert len(directions) == 1, "externally connected fifos should have one direction"
    direction = directions[0]
    external_name = convert_axis_to_fifo(task, internal_name) if axis else internal_name

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
