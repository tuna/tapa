"""Generate a tapa graphir from a floorplanned TAPA program."""

__copyright__ = """
Copyright (c) 2025 RapidStream Design Automation, Inc. and contributors.
All rights reserved. The contributor(s) of this file has/have agreed to the
RapidStream Contributor License Agreement.
"""

from collections.abc import Generator, Mapping
from pathlib import Path
from typing import TYPE_CHECKING

from tapa.graphir.types import (
    AnyModuleDefinition,
    Expression,
    GroupedModuleDefinition,
    HierarchicalName,
    ModuleConnection,
    ModuleInstantiation,
    ModuleNet,
    ModuleParameter,
    Modules,
    Project,
    Token,
    VerilogModuleDefinition,
)
from tapa.graphir_conversion.add_iface import get_graphir_iface
from tapa.graphir_conversion.pipeline.instantiation_builder import (
    get_top_ir_subinsts,
    get_upper_module_ir_subinsts,
)
from tapa.graphir_conversion.pipeline.ports_builder import (
    get_slot_module_definition as _get_slot_module_definition,
)
from tapa.graphir_conversion.pipeline.project_builder import (
    get_project_from_floorplanned_program as _get_project_from_floorplanned_program,
)
from tapa.graphir_conversion.pipeline.wire_builder import get_upper_task_ir_wires
from tapa.graphir_conversion.utils import (
    get_ctrl_s_axi_def,
    get_fifo_def,
    get_fsm_def,
    get_reset_inverter_def,
    get_reset_inverter_inst,
    get_task_graphir_parameters,
    get_task_graphir_ports,
    get_verilog_definition_from_tapa_module,
)
from tapa.task import Task
from tapa.verilog.xilinx.module import Module

if TYPE_CHECKING:
    from tapa.core import Program

_CTRL_S_AXI_PARAM_MAPPING = {
    "C_S_AXI_ADDR_WIDTH": "C_S_AXI_CONTROL_ADDR_WIDTH",
    "C_S_AXI_DATA_WIDTH": "C_S_AXI_CONTROL_DATA_WIDTH",
}
_CTRL_S_AXI_PORT_MAPPING: dict[str, Expression] = {
    port: Expression((Token.new_id(f"s_axi_control_{port}"),))
    for port in (
        "AWVALID",
        "AWREADY",
        "AWADDR",
        "WVALID",
        "WREADY",
        "WDATA",
        "WSTRB",
        "ARVALID",
        "ARREADY",
        "ARADDR",
        "RVALID",
        "RREADY",
        "RDATA",
        "RRESP",
        "BVALID",
        "BREADY",
        "BRESP",
    )
}
_CTRL_S_AXI_PORT_MAPPING.update(
    {
        "ACLK": Expression((Token.new_id("ap_clk"),)),
        "ARESET": Expression((Token.new_id("rst"),)),
        "ACLK_EN": Expression((Token.new_lit("1'b1"),)),
    }
)


def get_verilog_module_from_leaf_task(
    task: Task, code: str | None = None
) -> VerilogModuleDefinition:
    """Get the verilog module from a task."""
    assert task.is_lower
    if not task.module:
        msg = "Task contains no module"
        raise ValueError(msg)

    return get_verilog_definition_from_tapa_module(task.module, code)


def get_top_ctrl_s_axi_inst(
    top: Task,
    top_ir_param: list[ModuleParameter],
    ctrl_s_axi_ir: VerilogModuleDefinition,
    floorplan_region: str,
) -> ModuleInstantiation:
    """Get top ctrl_s_axi instantiation."""
    connections = [
        ModuleConnection(
            name=port.name,
            hierarchical_name=HierarchicalName.get_name(port.name),
            expr=_CTRL_S_AXI_PORT_MAPPING.get(
                port.name, Expression((Token.new_id(port.name),))
            ),
        )
        for port in ctrl_s_axi_ir.ports
    ]
    top_param_by_name = {p.name: p for p in top_ir_param}
    parameters = [
        ModuleConnection(
            name=param,
            hierarchical_name=HierarchicalName.get_name(param),
            expr=Expression(top_param_by_name[value].expr.root),
        )
        for param, value in _CTRL_S_AXI_PARAM_MAPPING.items()
    ]
    return ModuleInstantiation(
        name="control_s_axi_U",
        hierarchical_name=HierarchicalName.get_name("control_s_axi_U"),
        module=f"{top.name}_control_s_axi",
        connections=tuple(connections),
        parameters=tuple(parameters),
        floorplan_region=floorplan_region,
        area=None,
    )


def get_top_extra_wires(
    ctrl_s_axi_ir: VerilogModuleDefinition,
) -> Generator[ModuleNet]:
    """Get wires between control_s_axi and fsm."""
    for port in ctrl_s_axi_ir.ports:
        if port.name not in _CTRL_S_AXI_PORT_MAPPING:
            yield ModuleNet(
                name=port.name,
                hierarchical_name=HierarchicalName.get_name(port.name),
                range=port.range,
            )


def get_slot_module_definition(
    slot: Task,
    leaf_ir_defs: dict[str, VerilogModuleDefinition],
    floorplan_region: str,
) -> GroupedModuleDefinition:
    """Get slot module definition."""
    return _get_slot_module_definition(
        slot,
        leaf_ir_defs,
        floorplan_region,
        get_subinsts=get_upper_module_ir_subinsts,
        get_wires=get_upper_task_ir_wires,
    )


def get_top_module_definition(
    top: Task,
    slot_defs: Mapping[str, AnyModuleDefinition],
    ctrl_s_axi_ir: VerilogModuleDefinition,
    floorplan_task_name_region_mapping: dict[str, str],
) -> GroupedModuleDefinition:
    """Get top module definition."""
    top_ports = get_task_graphir_ports(top.module)
    top_param = get_task_graphir_parameters(top.module)

    # Assign a default region for fsm and ctrl_s_axi instantiation
    default_region = next(iter(floorplan_task_name_region_mapping.values()))

    top_subinsts = get_top_ir_subinsts(
        top,
        slot_defs,
        floorplan_task_name_region_mapping,
        default_region,
    )
    top_subinsts.append(
        get_top_ctrl_s_axi_inst(top, top_param, ctrl_s_axi_ir, default_region)
    )
    top_subinsts.append(get_reset_inverter_inst(default_region))

    top_wires = get_upper_task_ir_wires(
        top,
        slot_defs,
        top_ports,
        list(ctrl_s_axi_ir.ports),
        True,
    )
    top_wires.extend(get_top_extra_wires(ctrl_s_axi_ir))
    top_wires.append(
        ModuleNet(
            name="rst",
            hierarchical_name=HierarchicalName.get_name("rst"),
            range=None,
        )
    )

    return GroupedModuleDefinition(
        name=top.name,
        hierarchical_name=HierarchicalName.get_name(top.name),
        parameters=tuple(top_param),
        ports=tuple(top_ports),
        submodules=tuple(top_subinsts),
        wires=tuple(top_wires),
    )


def get_project_from_floorplanned_program(
    program: "Program", device_config: Path, floorplan_path: Path
) -> Project:
    """Get a graphir project from a floorplanned TAPA program."""
    return _get_project_from_floorplanned_program(
        program=program,
        device_config=device_config,
        floorplan_path=floorplan_path,
        get_verilog_module_from_leaf_task=get_verilog_module_from_leaf_task,
        get_slot_module_definition=get_slot_module_definition,
        get_top_module_definition=get_top_module_definition,
        get_ctrl_s_axi_def=get_ctrl_s_axi_def,
        get_fsm_def=get_fsm_def,
        get_fifo_def=get_fifo_def,
        get_reset_inverter_def=get_reset_inverter_def,
        get_graphir_iface=get_graphir_iface,
        module_cls=Module,
        modules_cls=Modules,
        project_cls=Project,
    )
