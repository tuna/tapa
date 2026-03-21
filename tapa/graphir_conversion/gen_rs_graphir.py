"""Generate a tapa graphir from a floorplanned TAPA program."""

__copyright__ = """
Copyright (c) 2025 RapidStream Design Automation, Inc. and contributors.
All rights reserved. The contributor(s) of this file has/have agreed to the
RapidStream Contributor License Agreement.
"""

import logging
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
    ModulePort,
    Modules,
    Project,
    Range,
    Token,
    VerilogModuleDefinition,
)
from tapa.graphir_conversion.add_iface import get_graphir_iface
from tapa.graphir_conversion.pipeline.fifo_builder import (
    get_fifo_inst as get_fifo_inst_builder,
)
from tapa.graphir_conversion.pipeline.fifo_builder import (
    infer_fifo_data_range as infer_fifo_data_range_builder,
)
from tapa.graphir_conversion.pipeline.instantiation_builder import (
    get_submodule_inst as get_submodule_inst_builder,
)
from tapa.graphir_conversion.pipeline.instantiation_builder import (
    get_top_ir_subinsts as get_top_ir_subinsts_builder,
)
from tapa.graphir_conversion.pipeline.instantiation_builder import (
    get_top_level_slot_inst as get_top_level_slot_inst_builder,
)
from tapa.graphir_conversion.pipeline.instantiation_builder import (
    get_upper_module_ir_subinsts as get_upper_module_ir_subinsts_builder,
)
from tapa.graphir_conversion.pipeline.ports_builder import (
    get_slot_module_definition as get_slot_module_definition_builder,
)
from tapa.graphir_conversion.pipeline.ports_builder import (
    get_slot_module_definition_parameters as get_slot_module_definition_parameters_b,
)
from tapa.graphir_conversion.pipeline.ports_builder import (
    get_slot_module_definition_ports as get_slot_module_definition_ports_b,
)
from tapa.graphir_conversion.pipeline.project_builder import (
    add_pblock_ranges as add_pblock_ranges_builder,
)
from tapa.graphir_conversion.pipeline.project_builder import (
    get_project_from_floorplanned_program as get_project_from_floorplanned_program_b,
)
from tapa.graphir_conversion.pipeline.wire_builder import (
    get_upper_task_ir_wires as get_upper_task_ir_wires_builder,
)
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
from tapa.instance import Instance
from tapa.task import Task
from tapa.verilog.util import Pipeline
from tapa.verilog.xilinx.module import Module

if TYPE_CHECKING:
    from tapa.core import Program

_logger = logging.getLogger().getChild(__name__)
_CTRL_S_AXI_PARAM_MAPPING = {
    "C_S_AXI_ADDR_WIDTH": "C_S_AXI_CONTROL_ADDR_WIDTH",
    "C_S_AXI_DATA_WIDTH": "C_S_AXI_CONTROL_DATA_WIDTH",
}
_CTRL_S_AXI_PORT_MAPPING = {
    "AWVALID": Expression((Token.new_id("s_axi_control_AWVALID"),)),
    "AWREADY": Expression((Token.new_id("s_axi_control_AWREADY"),)),
    "AWADDR": Expression((Token.new_id("s_axi_control_AWADDR"),)),
    "WVALID": Expression((Token.new_id("s_axi_control_WVALID"),)),
    "WREADY": Expression((Token.new_id("s_axi_control_WREADY"),)),
    "WDATA": Expression((Token.new_id("s_axi_control_WDATA"),)),
    "WSTRB": Expression((Token.new_id("s_axi_control_WSTRB"),)),
    "ARVALID": Expression((Token.new_id("s_axi_control_ARVALID"),)),
    "ARREADY": Expression((Token.new_id("s_axi_control_ARREADY"),)),
    "ARADDR": Expression((Token.new_id("s_axi_control_ARADDR"),)),
    "RVALID": Expression((Token.new_id("s_axi_control_RVALID"),)),
    "RREADY": Expression((Token.new_id("s_axi_control_RREADY"),)),
    "RDATA": Expression((Token.new_id("s_axi_control_RDATA"),)),
    "RRESP": Expression((Token.new_id("s_axi_control_RRESP"),)),
    "BVALID": Expression((Token.new_id("s_axi_control_BVALID"),)),
    "BREADY": Expression((Token.new_id("s_axi_control_BREADY"),)),
    "BRESP": Expression((Token.new_id("s_axi_control_BRESP"),)),
    "ACLK": Expression((Token.new_id("ap_clk"),)),
    "ARESET": Expression((Token.new_id("rst"),)),
    "ACLK_EN": Expression((Token.new_lit("1'b1"),)),
}


def get_verilog_module_from_leaf_task(
    task: Task, code: str | None = None
) -> VerilogModuleDefinition:
    """Get the verilog module from a task."""
    assert task.is_lower
    if not task.module:
        msg = "Task contains no module"
        raise ValueError(msg)

    return get_verilog_definition_from_tapa_module(task.module, code)


def get_slot_module_definition_parameters(
    leaf_modules: dict[str, VerilogModuleDefinition],
) -> list[ModuleParameter]:
    """Get slot module parameters."""
    return get_slot_module_definition_parameters_b(leaf_modules)


def get_slot_module_definition_ports(
    slot: Task,
    child_modules: dict[str, VerilogModuleDefinition],
) -> list[ModulePort]:
    """Get slot module ports.

    Args:
        slot: task of slot
        child_modules: graphir module definitions of its child modules. The key is the
            task name of the child module.

    Returns:
        List of the graphir ports of slot module definition.
    """
    return get_slot_module_definition_ports_b(slot, child_modules)


def get_submodule_inst(
    subtasks: dict[str, Task],
    inst: Instance,
    arg_table: dict[str, dict[str, Pipeline]],
    floorplan_region: str | None = None,
) -> ModuleInstantiation:
    """Get submodule instantiation."""
    return get_submodule_inst_builder(subtasks, inst, arg_table, floorplan_region)


def get_fifo_inst(  # noqa: PLR0917, PLR0913
    upper_task: Task,
    fifo_name: str,
    fifo: dict,
    submodule_ir_defs: Mapping[str, AnyModuleDefinition],
    is_top: bool = False,
    floorplan_region: str | None = None,
) -> ModuleInstantiation:
    """Get slot fifo module instantiation."""
    return get_fifo_inst_builder(
        upper_task,
        fifo_name,
        fifo,
        submodule_ir_defs,
        is_top,
        floorplan_region,
    )


def get_upper_module_ir_subinsts(
    upper_task: Task,
    submodule_ir_defs: Mapping[str, AnyModuleDefinition],
    floorplan_region: str | None = None,
) -> list[ModuleInstantiation]:
    """Get leaf module instantiations of slot module."""
    return get_upper_module_ir_subinsts_builder(
        upper_task,
        submodule_ir_defs,
        floorplan_region,
    )


def infer_fifo_data_range(
    fifo_name: str,
    fifo: dict,
    leaf_ir_defs: Mapping[str, AnyModuleDefinition],
    slot: Task,
    infer_port_name_from_tapa_module: bool = True,
) -> Range | None:
    """Infer the range of a fifo data."""
    return infer_fifo_data_range_builder(
        fifo_name,
        fifo,
        leaf_ir_defs,
        slot,
        infer_port_name_from_tapa_module,
    )


def get_upper_task_ir_wires(
    upper_task: Task,
    submodule_ir_defs: Mapping[str, AnyModuleDefinition],
    upper_task_ir_ports: list[ModulePort],
    ctrl_s_axi_ir_ports: list[ModulePort] = [],
    is_top: bool = False,
) -> list[ModuleNet]:
    """Get upper_task module wires."""
    return get_upper_task_ir_wires_builder(
        upper_task,
        submodule_ir_defs,
        upper_task_ir_ports,
        ctrl_s_axi_ir_ports,
        is_top,
    )


def get_slot_module_definition(
    slot: Task,
    leaf_ir_defs: dict[str, VerilogModuleDefinition],
    floorplan_region: str,
) -> GroupedModuleDefinition:
    """Get slot module definition."""
    return get_slot_module_definition_builder(
        slot,
        leaf_ir_defs,
        floorplan_region,
        get_subinsts=get_upper_module_ir_subinsts,
        get_wires=get_upper_task_ir_wires,
    )


def get_top_ctrl_s_axi_inst(
    top: Task,
    top_ir_param: list[ModuleParameter],
    ctrl_s_axi_ir: VerilogModuleDefinition,
    floorplan_region: str,
) -> ModuleInstantiation:
    """Get top ctrl_s_axi instantiation."""
    connections = []
    for port in ctrl_s_axi_ir.ports:
        if port.name in _CTRL_S_AXI_PORT_MAPPING:
            expr = _CTRL_S_AXI_PORT_MAPPING[port.name]
        else:
            expr = Expression((Token.new_id(port.name),))
        connections.append(
            ModuleConnection(
                name=port.name,
                hierarchical_name=HierarchicalName.get_name(port.name),
                expr=expr,
            )
        )
    parameters = []
    for param, value in _CTRL_S_AXI_PARAM_MAPPING.items():
        # find id in top def and replace to make parameter constant
        tokens = None
        for top_param in top_ir_param:
            if top_param.name == value:
                tokens = top_param.expr.root
                break
        assert tokens
        parameters.append(
            ModuleConnection(
                name=param,
                hierarchical_name=HierarchicalName.get_name(param),
                expr=Expression(tokens),
            )
        )
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
    ctrl_s_axi__ir: VerilogModuleDefinition,
) -> Generator[ModuleNet]:
    """Get wires between control_s_axi and fsm."""
    for port in ctrl_s_axi__ir.ports:
        if port.name not in _CTRL_S_AXI_PORT_MAPPING:
            yield ModuleNet(
                name=port.name,
                hierarchical_name=HierarchicalName.get_name(port.name),
                range=port.range,
            )


def get_top_level_slot_inst(
    slot_def: AnyModuleDefinition,
    slot_inst: Instance,
    arg_table: dict[str, Pipeline],
    floorplan_task_name_region_mapping: dict[str, str],
) -> ModuleInstantiation:
    """Get top level slot instantiation."""
    return get_top_level_slot_inst_builder(
        slot_def,
        slot_inst,
        arg_table,
        floorplan_task_name_region_mapping,
    )


def get_top_ir_subinsts(
    top_task: Task,
    slot_defs: Mapping[str, AnyModuleDefinition],
    floorplan_task_name_region_mapping: dict[str, str],
    fsm_floorplan_region: str,
) -> list[ModuleInstantiation]:
    """Get leaf module instantiations of slot module."""
    return get_top_ir_subinsts_builder(
        top_task,
        slot_defs,
        floorplan_task_name_region_mapping,
        fsm_floorplan_region,
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
    return get_project_from_floorplanned_program_b(
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


def add_pblock_ranges(
    device_config: Path,
    project: Project,
    floorplan_path: Path,
) -> None:
    """Get the pblock range for the TAPA program."""
    add_pblock_ranges_builder(device_config, project, floorplan_path)
