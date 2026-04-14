"""RTL state holder for Task codegen — creates and owns Module objects."""

from __future__ import annotations

from typing import TYPE_CHECKING

from tapa.protocol import (
    HANDSHAKE_CLK,
    HANDSHAKE_DONE,
    HANDSHAKE_IDLE,
    HANDSHAKE_READY,
    HANDSHAKE_RST_N,
    HANDSHAKE_START,
)
from tapa.task_codegen.fsm import add_rs_pragmas_to_fsm as _add_rs_pragmas_to_fsm
from tapa.task_codegen.m_axi import add_m_axi as _add_m_axi
from tapa.verilog.ast.ioport import IOPort
from tapa.verilog.xilinx.module import Module

if TYPE_CHECKING:
    from tapa.task import Task


class TaskRtlState:
    """Owns the codegen-specific setup for a Task's RTL modules.

    Creating this object creates the ``module`` and ``fsm_module``
    :class:`Module` objects on the task and sets up the FSM module with
    handshake ports for upper-level tasks.
    """

    def __init__(self, task: Task) -> None:
        self.task = task
        task.module = Module(name=task.name)
        task.fsm_module = Module(name=f"{task.name}_fsm")
        if task.is_upper:
            task.fsm_module.add_ports(
                [
                    IOPort("input", HANDSHAKE_CLK),
                    IOPort("input", HANDSHAKE_RST_N),
                    IOPort("input", HANDSHAKE_START),
                    IOPort("output", HANDSHAKE_READY),
                    IOPort("output", HANDSHAKE_DONE),
                    IOPort("output", HANDSHAKE_IDLE),
                ]
            )

    def add_m_axi(self, width_table: dict[str, int], files: dict[str, str]) -> None:
        """Add M-AXI ports and crossbar wiring for upper tasks."""
        _add_m_axi(self.task, width_table, files)

    def add_rs_pragmas_to_fsm(self) -> None:
        """Add RS pragmas to the FSM module."""
        _add_rs_pragmas_to_fsm(self.task)
