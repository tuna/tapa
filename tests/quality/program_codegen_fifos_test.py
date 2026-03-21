"""Focused tests for extracted Program FIFO codegen helpers."""

from types import SimpleNamespace
from typing import Any, cast
from unittest.mock import Mock

from tapa.common.target import Target
from tapa.program_codegen.fifos import connect_fifos, instantiate_fifos
from tapa.verilog.ast.logic import Assign
from tapa.verilog.ast.signal import Wire
from tapa.verilog.util import wire_name
from tapa.verilog.xilinx.const import ISTREAM_SUFFIXES, RST


class _FakeModule:
    def __init__(self, widths: dict[tuple[str, str], str]) -> None:
        self._widths = widths
        self.signals: list[Wire] = []
        self.logics: list[Assign] = []

    def add_signals(self, signals: list[Wire]) -> None:
        self.signals.extend(signals)

    def add_logics(self, logics: list[Assign]) -> None:
        self.logics.extend(logics)

    def get_port_of(self, fifo: str, suffix: str) -> SimpleNamespace:
        return SimpleNamespace(
            name=f"{fifo}_{suffix}_port",
            width=self._widths[fifo, suffix],
        )


def test_instantiate_fifos_skips_entries_without_depth_and_casts_depth() -> None:
    module = Mock()
    task = cast(
        "Any",
        SimpleNamespace(
            name="parent",
            fifos={"fifo_a": {"depth": "16"}, "fifo_b": {"direction": "produced_by"}},
            module=module,
        ),
    )
    get_fifo_width = Mock(return_value="fifo-width")

    instantiate_fifos(task=task, get_fifo_width=get_fifo_width)

    get_fifo_width.assert_called_once_with(task, "fifo_a")
    module.add_fifo_instance.assert_called_once_with(
        name="fifo_a",
        rst=RST,
        width="fifo-width",
        depth=16,
    )


def test_connect_fifos_uses_real_metadata_for_wires_and_external_connections() -> None:
    parent_module = _FakeModule(
        {("fifo_ext", suffix): "external-width" for suffix in ISTREAM_SUFFIXES},
    )
    child_module = _FakeModule(
        {("child_fifo", suffix): "child-width" for suffix in ISTREAM_SUFFIXES},
    )
    parent_task = cast(
        "Any",
        SimpleNamespace(
            name="top_task",
            tasks={
                "child_task": [
                    {
                        "args": {
                            "child_fifo": {
                                "arg": "fifo_ext",
                                "cat": "istream",
                            },
                        },
                    },
                ],
            },
            fifos={
                "fifo_ext": {
                    "consumed_by": ("child_task", 0),
                },
            },
            module=parent_module,
        ),
    )
    child_task = cast("Any", SimpleNamespace(module=child_module))
    get_task = Mock(return_value=child_task)

    connect_fifos(
        task=parent_task,
        top="different_top",
        target=Target.XILINX_VITIS,
        get_task=get_task,
    )

    assert [signal.name for signal in parent_module.signals] == [
        wire_name("fifo_ext", suffix) for suffix in ISTREAM_SUFFIXES
    ]
    assert all(signal.width == "child-width" for signal in parent_module.signals)
    assert parent_module.logics == [
        Assign(
            lhs=wire_name("fifo_ext", suffix),
            rhs=f"fifo_ext_{suffix}_port",
        )
        for suffix in ISTREAM_SUFFIXES
    ]
    get_task.assert_called_once_with("child_task")
