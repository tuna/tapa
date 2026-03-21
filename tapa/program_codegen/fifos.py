"""FIFO generation helpers for upper-level task modules."""

from __future__ import annotations

import logging
from typing import TYPE_CHECKING

from tapa.common.target import Target
from tapa.util import as_type
from tapa.verilog.ast.signal import Wire
from tapa.verilog.util import wire_name
from tapa.verilog.xilinx.const import RST

if TYPE_CHECKING:
    from collections.abc import Callable

    from pyverilog.vparser.ast import Plus

    from tapa.task import Task

_logger = logging.getLogger().getChild(__name__)


def connect_fifos(
    task: Task,
    top: str,
    target: Target,
    get_task: Callable[[str], Task],
) -> None:
    """Declare FIFO wires between child tasks and connect external FIFOs."""
    _logger.debug("  connecting %s's children tasks", task.name)
    for fifo_name in task.fifos:
        for direction in task.get_fifo_directions(fifo_name):
            task_name, _, fifo_port = task.get_connection_to(fifo_name, direction)

            for suffix in task.get_fifo_suffixes(direction):
                wire = Wire(
                    wire_name(fifo_name, suffix),
                    get_task(task_name).module.get_port_of(fifo_port, suffix).width,
                )
                task.module.add_signals([wire])

        if task.is_fifo_external(fifo_name):
            task.connect_fifo_externally(
                fifo_name,
                task.name == top and target == Target.XILINX_VITIS,
            )


def instantiate_fifos(
    task: Task,
    get_fifo_width: Callable[[Task, str], Plus],
) -> None:
    """Instantiate declared FIFO channels on the given task module."""
    _logger.debug("  instantiating FIFOs in %s", task.name)
    fifos = {name: fifo for name, fifo in task.fifos.items() if "depth" in fifo}
    if not fifos:
        return

    for fifo_name, fifo in fifos.items():
        _logger.debug("    instantiating %s.%s", task.name, fifo_name)
        task.module.add_fifo_instance(
            name=fifo_name,
            rst=RST,
            width=get_fifo_width(task, fifo_name),
            depth=as_type(int, fifo["depth"]),
        )
