"""TAPA Tasks."""

__copyright__ = """
Copyright (c) 2025 RapidStream Design Automation, Inc. and contributors.
All rights reserved. The contributor(s) of this file has/have agreed to the
RapidStream Contributor License Agreement.
"""

import collections
import decimal
import enum
import logging
from typing import NamedTuple

from tapa import __version__
from tapa.instance import Instance, Port
from tapa.task_codegen.fsm import add_rs_pragmas_to_fsm as add_rs_pragmas_to_fsm_codegen
from tapa.task_codegen.m_axi import add_m_axi as add_m_axi_codegen
from tapa.verilog.ast.ioport import IOPort
from tapa.verilog.xilinx.const import (
    HANDSHAKE_CLK,
    HANDSHAKE_DONE,
    HANDSHAKE_IDLE,
    HANDSHAKE_READY,
    HANDSHAKE_RST_N,
    HANDSHAKE_START,
)
from tapa.verilog.xilinx.module import Module

_logger = logging.getLogger().getChild(__name__)


class MMapConnection(NamedTuple):
    id_width: int
    thread_count: int
    args: tuple[Instance.Arg, ...]
    chan_count: int | None
    chan_size: int | None


class Task:
    """Describes a TAPA task.

    Attributes:
      level: Task.Level, upper or lower.
      name: str, name of the task, function name as defined in the source code.
      code: str, HLS C++ code of this task.
      tasks: A dict mapping child task names to json instance description objects.
      fifos: A dict mapping child fifo names to json FIFO description objects.
      ports: A dict mapping port names to Port objects for the current task.
      module: rtl.Module, should be attached after RTL code is generated.
      fsm_module: rtl.Module of the finite state machine (upper-level only).
      is_slot: bool, True if this task is a floorplan slot.

    Properties:
      is_upper: bool, True if this task is an upper-level task.
      is_lower: bool, True if this task is an lower-level task.

    Properties unique to upper tasks:
      instances: A tuple of Instance objects, children instances of this task.
      args: A dict mapping arg names to lists of Arg objects that belong to the
          children instances of this task.
      mmaps: A dict mapping mmap arg names to MMapConnection objects.
    """

    class Level(enum.Enum):
        LOWER = 0
        UPPER = 1

    def __init__(  # noqa: PLR0917,PLR0913
        self,
        name: str,
        code: str,
        level: "Task.Level | str",
        tasks: dict[str, list[dict[str, dict[str, dict[str, object]]]]] | None = None,
        fifos: dict[str, dict[str, tuple[str, int]]] | None = None,
        ports: list[dict[str, str | int]] | None = None,
        target_type: str | None = None,
        is_slot: bool = False,
    ) -> None:
        if isinstance(level, str):
            level = {"lower": Task.Level.LOWER, "upper": Task.Level.UPPER}.get(
                level, level
            )
        if not isinstance(level, Task.Level):
            raise TypeError("unexpected `level`: " + level)
        self.level = level
        self.name: str = name
        self.code: str = code
        self.tasks = {}
        self.fifos = {}
        self.target_type = target_type
        self.is_slot = is_slot
        port_dict = {i.name: i for i in map(Port, ports or [])}
        if self.is_upper:
            self.tasks = dict(sorted((tasks or {}).items()))
            self.fifos = dict(sorted((fifos or {}).items()))
            self.ports = port_dict
            self.fsm_module = Module(name=f"{self.name}_fsm")
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
        elif ports:
            # Nonsynthesizable tasks need ports to generate template
            self.ports = port_dict
        self.module = Module(name=self.name)
        self._instances: tuple[Instance, ...] | None = None
        self._args: dict[str, list[Instance.Arg]] | None = None
        self._mmaps: dict[str, MMapConnection] | None = None
        self._self_area = {}
        self._total_area = {}
        self._clock_period = decimal.Decimal(0)

    @property
    def is_upper(self) -> bool:
        return self.level == Task.Level.UPPER

    @property
    def is_lower(self) -> bool:
        return self.level == Task.Level.LOWER

    @property
    def instances(self) -> tuple[Instance, ...]:
        if self._instances is not None:
            return self._instances
        msg = f"children of task {self.name} not populated"
        raise ValueError(msg)

    @instances.setter
    def instances(self, instances: tuple[Instance, ...]) -> None:
        self._instances = instances
        self._args = collections.defaultdict(list)

        mmaps: dict[str, list[Instance.Arg]] = collections.defaultdict(list)
        for instance in instances:
            for arg in instance.args:
                self._args[arg.name].append(arg)
                if arg.cat.is_mmap:
                    mmaps[arg.name].append(arg)

        self._mmaps = {}
        for arg_name, args in mmaps.items():
            # width of the ID port is the sum of the widest slave port plus bits
            # required to multiplex the slaves
            id_width = max(
                arg.instance.task.get_id_width(arg.port) or 1 for arg in args
            )
            id_width += (len(args) - 1).bit_length()
            thread_count = sum(
                arg.instance.task.get_thread_count(arg.port) for arg in args
            )
            mmap = MMapConnection(
                id_width,
                thread_count,
                tuple(args),
                self.ports[args[0].name].chan_count,
                self.ports[args[0].name].chan_size,
            )
            assert all(self.ports[x.name].chan_count == mmap.chan_count for x in args)
            assert all(self.ports[x.name].chan_size == mmap.chan_size for x in args)
            self._mmaps[arg_name] = mmap

            for arg in args:
                arg.chan_count = mmap.chan_count
                arg.chan_size = mmap.chan_size

            if len(args) > 1:
                _logger.debug(
                    "mmap argument '%s.%s'"
                    " (id_width=%d, thread_count=%d, chan_size=%s, chan_count=%s)"
                    " is shared by %d ports:",
                    self.name,
                    arg_name,
                    mmap.id_width,
                    mmap.thread_count,
                    mmap.chan_count,
                    mmap.chan_size,
                    len(args),
                )
                for arg in args:
                    arg.shared = True
                    _logger.debug("  %s.%s", arg.instance.name, arg.port)
            else:
                _logger.debug(
                    "mmap argument '%s.%s'"
                    " (id_width=%d, thread_count=%d, chan_size=%s, chan_count=%s)"
                    " is connected to port '%s.%s'",
                    self.name,
                    arg_name,
                    mmap.id_width,
                    mmap.thread_count,
                    mmap.chan_count,
                    mmap.chan_size,
                    args[0].instance.name,
                    args[0].port,
                )

    @property
    def args(self) -> dict[str, list[Instance.Arg]]:
        if self._args is not None:
            return self._args
        msg = f"children of task {self.name} not populated"
        raise ValueError(msg)

    @property
    def mmaps(self) -> dict[str, MMapConnection]:
        if self._mmaps is not None:
            return self._mmaps
        msg = f"children of task {self.name} not populated"
        raise ValueError(msg)

    @property
    def self_area(self) -> dict[str, int]:
        if self._self_area:
            return self._self_area
        msg = f"area of task {self.name} not populated"
        raise ValueError(msg)

    @self_area.setter
    def self_area(self, area: dict[str, int]) -> None:
        if self._self_area:
            msg = f"area of task {self.name} already populated"
            raise ValueError(msg)
        self._self_area = area

    @property
    def total_area(self) -> dict[str, int]:
        if self._total_area:
            return self._total_area

        area = dict(self.self_area)
        for instance in self.instances:
            for key in area:
                area[key] += instance.task.total_area[key]
        return area

    @total_area.setter
    def total_area(self, area: dict[str, int]) -> None:
        if self._total_area:
            msg = f"total area of task {self.name} already populated"
            raise ValueError(msg)
        self._total_area = area

    @property
    def clock_period(self) -> decimal.Decimal:
        if self.is_upper:
            return max(
                self._clock_period,
                *(x.clock_period for x in {y.task for y in self.instances}),
            )
        if self._clock_period:
            return self._clock_period
        msg = f"clock period of task {self.name} not populated"
        _logger.warning(msg)
        return decimal.Decimal(0)

    @clock_period.setter
    def clock_period(self, clock_period: decimal.Decimal) -> None:
        if self._clock_period:
            msg = f"clock period of task {self.name} already populated"
            raise ValueError(msg)
        self._clock_period = clock_period

    @property
    def report(self) -> dict[str, str | dict]:
        performance: dict[str, str | dict] = {
            "source": "hls",
            "clock_period": str(self.clock_period),
        }

        area = {
            "source": "synth" if self._total_area else "hls",
            "total": self.total_area,
        }

        if self.is_upper:
            performance["critical_path"] = {}
            area["breakdown"] = {}
            for instance in self.instances:
                task_report = instance.task.report

                if self.clock_period == instance.task.clock_period:
                    performance["critical_path"].setdefault(
                        instance.task.name,
                        task_report["performance"],
                    )

                area["breakdown"].setdefault(
                    instance.task.name,
                    {"count": 0, "area": task_report["area"]},
                )["count"] += 1

        return {
            "schema": __version__,
            "name": self.name,
            "performance": performance,
            "area": area,
        }

    def get_id_width(self, port: str) -> int | None:
        if port in self.mmaps:
            return self.mmaps[port].id_width or None
        return None

    def get_thread_count(self, port: str) -> int:
        if port in self.mmaps:
            return self.mmaps[port].thread_count
        return 1

    def add_m_axi(self, width_table: dict[str, int], files: dict[str, str]) -> None:
        """Add M-AXI ports and crossbar wiring for upper tasks."""
        add_m_axi_codegen(self, width_table, files)

    def add_rs_pragmas_to_fsm(self) -> None:
        """Add RS pragmas to the FSM module."""
        add_rs_pragmas_to_fsm_codegen(self)
