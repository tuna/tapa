"""Instantiation and wiring helpers for GraphIR conversion."""

from __future__ import annotations

from typing import TYPE_CHECKING

from tapa.graphir.types import (
    AnyModuleDefinition,
    Expression,
    HierarchicalName,
    ModuleConnection,
    ModuleInstantiation,
    Token,
)
from tapa.graphir_conversion.pipeline.fifo_builder import get_fifo_inst
from tapa.graphir_conversion.utils import (
    get_m_axi_port_name,
    get_stream_port_name,
    get_task_arg_table,
)
from tapa.instance import Instance
from tapa.task_codegen.fifos import is_fifo_external as is_fifo_external_codegen
from tapa.verilog.util import Pipeline, match_array_name
from tapa.verilog.xilinx.const import (
    ISTREAM_SUFFIXES,
    OSTREAM_SUFFIXES,
    STREAM_PORT_DIRECTION,
)
from tapa.verilog.xilinx.m_axi import M_AXI_SUFFIXES

if TYPE_CHECKING:
    from collections.abc import Callable, Mapping

    from tapa.task import Task


def _get_control_connections(instance_name: str) -> list[ModuleConnection]:
    connections = [
        ModuleConnection(
            name="ap_clk",
            hierarchical_name=HierarchicalName.get_name("ap_clk"),
            expr=Expression((Token.new_id("ap_clk"),)),
        ),
        ModuleConnection(
            name="ap_rst_n",
            hierarchical_name=HierarchicalName.get_name("ap_rst_n"),
            expr=Expression((Token.new_id("ap_rst_n"),)),
        ),
    ]
    ap_signals = ["ap_start", "ap_done", "ap_ready", "ap_idle"]
    connections.extend(
        ModuleConnection(
            name=signal,
            hierarchical_name=HierarchicalName.get_name(signal),
            expr=Expression((Token.new_id(f"{instance_name}__{signal}"),)),
        )
        for signal in ap_signals
    )
    return connections


def _get_task_inst_connections(  # noqa: C901
    instance_name: str,
    args: tuple[Instance.Arg, ...],
    arg_table: Mapping[str, Pipeline],
    get_stream_port: Callable[[str, str], str | None],
    has_port: Callable[[str], bool],
) -> list[ModuleConnection]:
    connections = []
    for arg in args:
        port_name = arg.port
        if arg.cat == Instance.Arg.Cat.SCALAR:
            expr = Expression.from_str_to_tokens(arg.name)
            if not expr.is_all_literals():
                expr = Expression((Token.new_id(arg_table[arg.name][-1].name),))
            connections.append(
                ModuleConnection(
                    name=port_name,
                    hierarchical_name=HierarchicalName.get_name(port_name),
                    expr=expr,
                )
            )
            continue

        if arg.cat == Instance.Arg.Cat.ISTREAM:
            for suffix in ISTREAM_SUFFIXES:
                leaf_port = get_stream_port(port_name, suffix)
                assert leaf_port is not None
                expr = Expression(
                    (Token.new_id(get_stream_port_name(arg.name, suffix)),)
                )
                connections.append(
                    ModuleConnection(
                        name=leaf_port,
                        hierarchical_name=HierarchicalName.get_name(leaf_port),
                        expr=expr,
                    )
                )
                if STREAM_PORT_DIRECTION[suffix] == "input":
                    peek_port = get_stream_port(port_name, f"_peek{suffix}")
                    if peek_port is not None:
                        connections.append(
                            ModuleConnection(
                                name=peek_port,
                                hierarchical_name=HierarchicalName.get_name(peek_port),
                                expr=expr,
                            )
                        )
            continue

        if arg.cat == Instance.Arg.Cat.OSTREAM:
            for suffix in OSTREAM_SUFFIXES:
                leaf_port = get_stream_port(port_name, suffix)
                assert leaf_port is not None
                connections.append(
                    ModuleConnection(
                        name=leaf_port,
                        hierarchical_name=HierarchicalName.get_name(leaf_port),
                        expr=Expression(
                            (Token.new_id(get_stream_port_name(arg.name, suffix)),)
                        ),
                    )
                )
            continue

        assert arg.cat == Instance.Arg.Cat.MMAP, arg.cat
        for suffix in M_AXI_SUFFIXES:
            full_port_name = get_m_axi_port_name(port_name, suffix)
            if not has_port(full_port_name):
                continue
            connections.append(
                ModuleConnection(
                    name=full_port_name,
                    hierarchical_name=HierarchicalName.get_name(full_port_name),
                    expr=Expression(
                        (Token.new_id(get_m_axi_port_name(arg.name, suffix)),)
                    ),
                )
            )
        offset_port_name = f"{port_name}_offset"
        connections.append(
            ModuleConnection(
                name=offset_port_name,
                hierarchical_name=HierarchicalName.get_name(offset_port_name),
                expr=Expression((Token.new_id(arg_table[arg.name][-1].name),)),
            )
        )

    connections.extend(_get_control_connections(instance_name))
    return connections


def get_submodule_inst(
    subtasks: dict[str, Task],
    inst: Instance,
    arg_table: dict[str, dict[str, Pipeline]],
    floorplan_region: str | None = None,
) -> ModuleInstantiation:
    """Get submodule instantiation."""
    task_name = inst.task.name
    subtask_module = subtasks[task_name].module

    def get_stream_port(port_name: str, suffix: str) -> str | None:
        if suffix.startswith("_peek"):
            match = match_array_name(port_name)
            if match is not None:
                return f"{match[0]}_peek_{match[1]}{suffix[5:]}"
        return subtask_module.get_port_of(port_name, suffix).name

    return ModuleInstantiation(
        name=inst.name,
        hierarchical_name=HierarchicalName.get_name(inst.name),
        module=task_name,
        connections=tuple(
            _get_task_inst_connections(
                instance_name=inst.name,
                args=inst.args,
                arg_table=arg_table[inst.name],
                get_stream_port=get_stream_port,
                has_port=lambda name: name in subtask_module.ports,
            )
        ),
        parameters=(),
        floorplan_region=floorplan_region,
        area=None,
    )


def get_upper_module_ir_subinsts(
    upper_task: Task,
    submodule_ir_defs: Mapping[str, AnyModuleDefinition],
    floorplan_region: str | None = None,
) -> list[ModuleInstantiation]:
    """Get leaf module instantiations of slot module."""
    subtasks = {inst.task.name: inst.task for inst in upper_task.instances}
    ir_insts = [
        get_submodule_inst(
            subtasks,
            inst,
            get_task_arg_table(upper_task),
            floorplan_region,
        )
        for inst in upper_task.instances
    ]
    fsm_module = upper_task.fsm_module
    ir_insts.append(
        ModuleInstantiation(
            name=f"{fsm_module.name}_0",
            hierarchical_name=HierarchicalName.get_name(f"{fsm_module.name}_0"),
            module=fsm_module.name,
            connections=tuple(
                ModuleConnection(
                    name=port,
                    hierarchical_name=HierarchicalName.get_name(port),
                    expr=Expression((Token.new_id(port),)),
                )
                for port in fsm_module.ports
            ),
            parameters=(),
            floorplan_region=floorplan_region,
            area=None,
        )
    )
    for fifo_name, fifo in upper_task.fifos.items():
        if is_fifo_external_codegen(upper_task, fifo_name):
            continue
        ir_insts.append(
            get_fifo_inst(
                upper_task,
                fifo_name,
                fifo,
                submodule_ir_defs,
                floorplan_region=floorplan_region,
            )
        )
    return ir_insts


def get_top_level_slot_inst(
    slot_def: AnyModuleDefinition,
    slot_inst: Instance,
    arg_table: dict[str, Pipeline],
    floorplan_task_name_region_mapping: dict[str, str],
) -> ModuleInstantiation:
    """Get top level slot instantiation."""
    slot_def_port_names = {port.name for port in slot_def.ports}

    def get_stream_port(port_name: str, suffix: str) -> str | None:
        full_name = f"{port_name}{suffix}"
        return full_name if full_name in slot_def_port_names else None

    return ModuleInstantiation(
        name=slot_inst.name,
        hierarchical_name=HierarchicalName.get_name(slot_inst.name),
        module=slot_inst.task.name,
        connections=tuple(
            _get_task_inst_connections(
                instance_name=slot_inst.name,
                args=slot_inst.args,
                arg_table=arg_table,
                get_stream_port=get_stream_port,
                has_port=lambda name: name in slot_def_port_names,
            )
        ),
        parameters=(),
        floorplan_region=floorplan_task_name_region_mapping[slot_inst.task.name],
        area=None,
    )


def get_top_ir_subinsts(
    top_task: Task,
    slot_defs: Mapping[str, AnyModuleDefinition],
    floorplan_task_name_region_mapping: dict[str, str],
    fsm_floorplan_region: str,
) -> list[ModuleInstantiation]:
    """Get slot, FSM, and FIFO instantiations for the top module."""
    ir_insts = [
        get_top_level_slot_inst(
            slot_defs[inst.task.name],
            inst,
            get_task_arg_table(top_task)[inst.name],
            floorplan_task_name_region_mapping,
        )
        for inst in top_task.instances
    ]
    fsm_module = top_task.fsm_module
    ir_insts.append(
        ModuleInstantiation(
            name=f"{fsm_module.name}_0",
            hierarchical_name=HierarchicalName.get_name(f"{fsm_module.name}_0"),
            module=fsm_module.name,
            connections=tuple(
                ModuleConnection(
                    name=port,
                    hierarchical_name=HierarchicalName.get_name(port),
                    expr=Expression((Token.new_id(port),)),
                )
                for port in fsm_module.ports
            ),
            parameters=(),
            floorplan_region=fsm_floorplan_region,
            area=None,
        )
    )
    for fifo_name, fifo in top_task.fifos.items():
        if is_fifo_external_codegen(top_task, fifo_name):
            continue
        ir_insts.append(
            get_fifo_inst(
                top_task,
                fifo_name,
                fifo,
                slot_defs,
                True,
                floorplan_task_name_region_mapping[fifo["consumed_by"][0]],
            )
        )
    return ir_insts
