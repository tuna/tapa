"""Tests for GraphIR interface role inference helpers."""

from types import SimpleNamespace
from typing import cast

import pytest

from tapa.graphir.types import (
    AnyModuleDefinition,
    ApCtrlInterface,
    BaseInterface,
    FeedForwardInterface,
    HandShakeInterface,
    HierarchicalName,
    ModulePort,
)
from tapa.graphir_conversion.pipeline.iface_roles import set_iface_role


def _port(name: str, type_: ModulePort.Type) -> ModulePort:
    return ModulePort(
        name=name,
        hierarchical_name=HierarchicalName.get_name(name),
        type=type_,
        range=None,
    )


def test_set_iface_role_marks_handshake_source() -> None:
    module = cast(
        "AnyModuleDefinition",
        SimpleNamespace(
            name="leaf",
            ports=(
                _port("clk", ModulePort.Type.INPUT),
                _port("rst_n", ModulePort.Type.INPUT),
                _port("valid", ModulePort.Type.OUTPUT),
                _port("ready", ModulePort.Type.INPUT),
                _port("data", ModulePort.Type.OUTPUT),
            ),
        ),
    )
    iface = HandShakeInterface(
        ports=("data", "valid", "ready"),
        clk_port="clk",
        rst_port="rst_n",
        valid_port="valid",
        ready_port="ready",
        origin_info="",
    )

    updated = set_iface_role(module, iface)

    assert updated.role == BaseInterface.InterfaceRole.SOURCE


def test_set_iface_role_marks_ap_ctrl_sink() -> None:
    module = cast(
        "AnyModuleDefinition",
        SimpleNamespace(
            name="slot",
            ports=(
                _port("clk", ModulePort.Type.INPUT),
                _port("rst_n", ModulePort.Type.INPUT),
                _port("ap_start", ModulePort.Type.INPUT),
                _port("ap_ready", ModulePort.Type.OUTPUT),
                _port("ap_done", ModulePort.Type.OUTPUT),
                _port("ap_idle", ModulePort.Type.OUTPUT),
            ),
        ),
    )
    iface = ApCtrlInterface(
        ports=("ap_start", "ap_ready", "ap_done", "ap_idle"),
        clk_port="clk",
        rst_port="rst_n",
        ap_start_port="ap_start",
        ap_ready_port="ap_ready",
        ap_done_port="ap_done",
        ap_idle_port="ap_idle",
        ap_continue_port=None,
        origin_info="",
    )

    updated = set_iface_role(module, iface)

    assert updated.role == BaseInterface.InterfaceRole.SINK


def test_set_iface_role_rejects_mixed_feedforward_directions() -> None:
    module = cast(
        "AnyModuleDefinition",
        SimpleNamespace(
            name="mixed",
            ports=(
                _port("clk", ModulePort.Type.INPUT),
                _port("a", ModulePort.Type.INPUT),
                _port("b", ModulePort.Type.OUTPUT),
            ),
        ),
    )
    iface = FeedForwardInterface(
        ports=("a", "b"),
        clk_port="clk",
        rst_port=None,
        origin_info="",
    )

    with pytest.raises(ValueError, match="Incorrect feed_forward interface"):
        set_iface_role(module, iface)
