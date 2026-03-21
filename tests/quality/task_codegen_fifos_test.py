"""Focused tests for extracted Task FIFO helpers."""

from types import SimpleNamespace
from typing import Any, cast
from unittest.mock import Mock

from tapa.task_codegen.fifos import (
    connect_fifo_externally,
    convert_axis_to_fifo,
    get_fifo_suffixes,
)

EXPECTED_FIFO_DEPTH = 2
EXPECTED_SIGNAL_CALLS = 6


def test_convert_axis_to_fifo_registers_fifo_and_axis_wires() -> None:
    module = Mock()
    module.find_port.side_effect = lambda port, suffix: f"{port}.{suffix}"
    task = cast(
        "Any",
        SimpleNamespace(
            name="task",
            ports={"axis": SimpleNamespace(width=32)},
            fifos={"axis": {"consumed_by": ("child", 0)}},
            module=module,
            get_fifo_directions=Mock(return_value=["consumed_by"]),
        ),
    )

    fifo_name = convert_axis_to_fifo(task, "axis")

    assert fifo_name == "tapa_fifo_axis"
    module.add_fifo_instance.assert_called_once()
    assert module.add_fifo_instance.call_args.kwargs["name"] == "tapa_fifo_axis"
    assert module.add_fifo_instance.call_args.kwargs["depth"] == EXPECTED_FIFO_DEPTH
    assert module.add_signals.call_count == EXPECTED_SIGNAL_CALLS
    assert module.add_logics.call_count == 1


def test_connect_fifo_externally_reuses_module_ports() -> None:
    module = Mock()
    module.get_port_of.return_value = SimpleNamespace(name="fifo_a_port")
    task = cast(
        "Any",
        SimpleNamespace(
            name="task",
            fifos={"fifo_a": {"produced_by": ("child", 0)}},
            module=module,
            get_fifo_directions=Mock(return_value=["produced_by"]),
        ),
    )

    connect_fifo_externally(task, "fifo_a", False)

    assert module.get_port_of.call_count == len(get_fifo_suffixes("produced_by"))
    assert module.add_logics.call_count == len(get_fifo_suffixes("produced_by"))
