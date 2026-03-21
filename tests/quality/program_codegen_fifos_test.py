"""Focused tests for extracted Program FIFO codegen helpers."""

from types import SimpleNamespace
from typing import Any, cast
from unittest.mock import Mock

from tapa.common.target import Target
from tapa.program_codegen.fifos import connect_fifos, instantiate_fifos
from tapa.verilog.util import wire_name
from tapa.verilog.xilinx.const import RST

EXPECTED_SIGNAL_CALLS = 2


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


def test_connect_fifos_adds_child_wires_and_connects_external_fifo() -> None:
    parent_module = Mock()
    parent_task = cast(
        "Any",
        SimpleNamespace(
            name="top_task",
            fifos={"fifo_a": {"depth": 8}},
            module=parent_module,
            get_fifo_directions=Mock(return_value=["consumed_by"]),
            get_connection_to=Mock(return_value=("child_task", "inst_0", "in_fifo")),
            get_fifo_suffixes=Mock(return_value=["dout", "empty_n"]),
            is_fifo_external=Mock(return_value=True),
            connect_fifo_externally=Mock(),
        ),
    )
    child_module = Mock()
    child_module.get_port_of.return_value = SimpleNamespace(width="child-width")
    child_task = cast("Any", SimpleNamespace(module=child_module))
    get_task = Mock(return_value=child_task)

    connect_fifos(
        task=parent_task,
        top="top_task",
        target=Target.XILINX_VITIS,
        get_task=get_task,
    )

    assert parent_module.add_signals.call_count == EXPECTED_SIGNAL_CALLS
    first_wire = parent_module.add_signals.call_args_list[0].args[0][0]
    second_wire = parent_module.add_signals.call_args_list[1].args[0][0]
    assert first_wire.name == wire_name("fifo_a", "dout")
    assert second_wire.name == wire_name("fifo_a", "empty_n")
    assert first_wire.width == "child-width"
    assert second_wire.width == "child-width"
    parent_task.connect_fifo_externally.assert_called_once_with("fifo_a", True)
