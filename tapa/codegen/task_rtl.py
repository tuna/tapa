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

if TYPE_CHECKING:
    from tapa.task import Task
    from tapa.verilog.xilinx.module import Module


class TaskRtlState:
    """Retained owner of a Task's RTL modules.

    ``module`` and ``fsm_module`` live on this state object.  They are
    also assigned to ``task.module`` / ``task.fsm_module`` so that
    existing read-through accessors (e.g. ``task.rtl_module``) continue
    to work without change.

    Heavy imports (Module, IOPort, task_codegen) are deferred to method
    bodies so that ``import tapa.codegen.task_rtl`` stays lightweight.
    """

    __slots__ = ("fsm_module", "module", "task")

    def __init__(self, task: Task) -> None:
        from tapa.verilog.ast.ioport import IOPort  # noqa: PLC0415
        from tapa.verilog.xilinx.module import Module  # noqa: PLC0415

        self.task = task
        self.module: Module = Module(name=task.name)
        self.fsm_module: Module | None = None
        if task.is_upper:
            self.fsm_module = Module(
                name=f"{task.name}_fsm",
                is_trimming_enabled=False,
            )
            self.fsm_module.add_ports(
                [
                    IOPort("input", HANDSHAKE_CLK),
                    IOPort("input", HANDSHAKE_RST_N),
                    IOPort("input", HANDSHAKE_START),
                    IOPort("output", HANDSHAKE_READY),
                    IOPort("output", HANDSHAKE_DONE),
                    IOPort("output", HANDSHAKE_IDLE),
                ]
            )
        # Sync to task for read-through accessors.
        task.module = self.module
        task.fsm_module = self.fsm_module

    def add_m_axi(self, width_table: dict[str, int], files: dict[str, str]) -> None:
        """Add M-AXI ports and crossbar wiring for upper tasks."""
        from tapa.task_codegen.m_axi import add_m_axi as _impl  # noqa: PLC0415

        _impl(self.task, width_table, files)

    def add_rs_pragmas_to_fsm(self) -> None:
        """Add RS pragmas to the FSM module."""
        from tapa.task_codegen.fsm import (  # noqa: PLC0415
            add_rs_pragmas_to_fsm as _impl,
        )

        _impl(self.task)
