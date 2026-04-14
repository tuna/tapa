"""Port-building helpers for GraphIR conversion."""

from __future__ import annotations

import logging
from typing import TYPE_CHECKING

from tapa.graphir.types import (
    GroupedModuleDefinition,
    HierarchicalName,
    ModuleParameter,
    ModulePort,
    VerilogModuleDefinition,
)
from tapa.graphir_conversion.utils import get_child_port_connection_mapping
from tapa.protocol import HANDSHAKE_INPUT_PORTS, HANDSHAKE_OUTPUT_PORTS
from tapa.verilog.util import match_array_name

if TYPE_CHECKING:
    from collections.abc import Callable

    from tapa.graphir.types import ModuleInstantiation, ModuleNet
    from tapa.task import Task

_logger = logging.getLogger().getChild(__name__)


def get_slot_module_definition_parameters(
    leaf_modules: dict[str, VerilogModuleDefinition],
) -> list[ModuleParameter]:
    """Get slot module parameters."""
    parameters = {}
    for leaf_module in leaf_modules.values():
        for param in leaf_module.parameters:
            if param.name not in parameters:
                parameters[param.name] = param
            elif param.expr != parameters[param.name].expr:
                _logger.error(
                    "Parameter %s has different values in leaf modules",
                    param.name,
                )
    return list(parameters.values())


def _get_port_direction(name: str) -> ModulePort.Type:
    if name in HANDSHAKE_INPUT_PORTS:
        return ModulePort.Type.INPUT
    if name in HANDSHAKE_OUTPUT_PORTS:
        return ModulePort.Type.OUTPUT
    msg = f"Unknown handshake port direction for {name}"
    raise ValueError(msg)


def _find_port_child(slot: Task, port: str) -> tuple[str, str, int | None] | None:
    """Find (task_name, inst_port, inst_port_idx) for a slot port."""
    for inst in slot.instances:
        for arg in inst.args:
            if arg.name != port:
                continue
            match = match_array_name(arg.port)
            if match:
                return inst.task.name, match[0], match[1]
            return inst.task.name, arg.port, None
    return None


def get_slot_module_definition_ports(
    slot: Task,
    child_modules: dict[str, VerilogModuleDefinition],
) -> list[ModulePort]:
    """Get slot module ports."""
    ports = []
    child_module_tasks = {inst.task.name: inst.task for inst in slot.instances}
    for port in slot.ports:
        found = _find_port_child(slot, port)
        if not found:
            continue
        child_module_name, child_inst_port, child_inst_port_idx = found

        child_module_ir = child_modules[child_module_name]
        child_module_task = child_module_tasks[child_module_name]
        assert child_inst_port in child_module_task.ports
        task_port = child_module_task.ports[child_inst_port]
        port_map = get_child_port_connection_mapping(
            task_port, child_module_task.module, port, child_inst_port_idx
        )
        for child_port, slot_port in port_map.items():
            child_module_ir_port = child_module_ir.get_port(child_port)
            ports.append(
                ModulePort(
                    name=slot_port,
                    hierarchical_name=HierarchicalName.get_name(slot_port),
                    type=child_module_ir_port.type,
                    range=child_module_ir_port.range,
                )
            )

    ports.extend(
        ModulePort(
            name=name,
            hierarchical_name=HierarchicalName.get_name(name),
            type=_get_port_direction(name),
            range=None,
        )
        for name in (
            "ap_clk",
            "ap_rst_n",
            "ap_start",
            "ap_done",
            "ap_ready",
            "ap_idle",
        )
    )
    return ports


def get_slot_module_definition(
    slot: Task,
    leaf_ir_defs: dict[str, VerilogModuleDefinition],
    floorplan_region: str,
    get_subinsts: Callable[
        [Task, dict[str, VerilogModuleDefinition], str],
        list[ModuleInstantiation],
    ],
    get_wires: Callable[
        [Task, dict[str, VerilogModuleDefinition], list[ModulePort]],
        list[ModuleNet],
    ],
) -> GroupedModuleDefinition:
    """Build the slot grouped-module definition."""
    slot_ports = get_slot_module_definition_ports(slot, leaf_ir_defs)
    return GroupedModuleDefinition(
        name=slot.name,
        hierarchical_name=HierarchicalName.get_name(slot.name),
        parameters=tuple(get_slot_module_definition_parameters(leaf_ir_defs)),
        ports=tuple(slot_ports),
        submodules=tuple(get_subinsts(slot, leaf_ir_defs, floorplan_region)),
        wires=tuple(get_wires(slot, leaf_ir_defs, slot_ports)),
    )
