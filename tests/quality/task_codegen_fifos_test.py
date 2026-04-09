"""Focused tests for extracted Task FIFO helpers."""

from pathlib import Path
from types import SimpleNamespace
from typing import Any, cast
from unittest.mock import Mock

from pyverilog.vparser.ast import Identifier, IntConst, Node, PortArg, Unot

from tapa.task_codegen.fifos import (
    connect_fifo_externally,
    convert_axis_to_fifo,
    get_fifo_suffixes,
)


def _arg_text(node: Node | None) -> str:
    if node is None:
        return ""
    if isinstance(node, Identifier):
        return node.name
    if isinstance(node, IntConst):
        return node.value
    if isinstance(node, Unot):
        return f"~{_arg_text(node.right)}"
    return str(node)


def _port_map(ports: tuple[PortArg, ...]) -> dict[str, str]:
    return {port.portname: _arg_text(port.argname) for port in ports}


def test_convert_axis_to_fifo_uses_axis_to_stream_adapter_for_external_inputs() -> None:
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

    adapter_name = convert_axis_to_fifo(task, "axis")

    assert adapter_name == "tapa_axis_axis"
    module.add_instance.assert_called_once()
    kwargs = module.add_instance.call_args.kwargs
    assert kwargs["module_name"] == "axis_to_stream_adapter"
    assert kwargs["instance_name"] == "tapa_axis_axis"
    assert _port_map(tuple(kwargs["ports"])) == {
        "clk": "ap_clk",
        "reset": "~ap_rst_n",
        "s_axis_tdata": "axis.TDATA",
        "s_axis_tvalid": "axis.TVALID",
        "s_axis_tready": "axis.TREADY",
        "s_axis_tlast": "axis.TLAST",
        "m_stream_dout": "axis__dout",
        "m_stream_empty_n": "axis__empty_n",
        "m_stream_read": "axis__read",
    }
    assert next(iter(kwargs["params"])).paramname == "DATA_WIDTH"
    assert _arg_text(next(iter(kwargs["params"])).argname) == "32"
    module.add_fifo_instance.assert_not_called()
    module.add_signals.assert_not_called()
    module.add_logics.assert_not_called()


def test_convert_axis_to_fifo_uses_stream_to_axis_adapter_for_external_outputs() -> (
    None
):
    module = Mock()
    module.find_port.side_effect = lambda port, suffix: f"{port}.{suffix}"
    task = cast(
        "Any",
        SimpleNamespace(
            name="task",
            ports={"axis": SimpleNamespace(width=32)},
            fifos={"axis": {"produced_by": ("child", 0)}},
            module=module,
            get_fifo_directions=Mock(return_value=["produced_by"]),
        ),
    )

    adapter_name = convert_axis_to_fifo(task, "axis")

    assert adapter_name == "tapa_axis_axis"
    module.add_instance.assert_called_once()
    kwargs = module.add_instance.call_args.kwargs
    assert kwargs["module_name"] == "stream_to_axis_adapter"
    assert kwargs["instance_name"] == "tapa_axis_axis"
    assert _port_map(tuple(kwargs["ports"])) == {
        "clk": "ap_clk",
        "reset": "~ap_rst_n",
        "s_stream_din": "axis__din",
        "s_stream_full_n": "axis__full_n",
        "s_stream_write": "axis__write",
        "m_axis_tdata": "axis.TDATA",
        "m_axis_tvalid": "axis.TVALID",
        "m_axis_tready": "axis.TREADY",
        "m_axis_tlast": "axis.TLAST",
    }
    assert next(iter(kwargs["params"])).paramname == "DATA_WIDTH"
    assert _arg_text(next(iter(kwargs["params"])).argname) == "32"
    module.add_fifo_instance.assert_not_called()
    module.add_signals.assert_not_called()
    module.add_logics.assert_called_once()


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


def test_axis_adapter_asset_uses_explicit_handshake_state() -> None:
    text = Path("tapa/assets/verilog/axis_adapter.v").read_text(encoding="utf-8")

    assert "module axis_to_stream_adapter" in text
    assert "module stream_to_axis_adapter" in text
    assert "stage0_valid" in text
    assert "stage1_valid" in text
    assert "fifo #(" not in text
