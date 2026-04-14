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
from tapa.protocol import (
    ISTREAM_SUFFIXES,
    M_AXI_SUFFIXES,
    OSTREAM_SUFFIXES,
    STREAM_PORT_DIRECTION,
)
from tapa.task_codegen.fifos import is_fifo_external as is_fifo_external_codegen
from tapa.verilog.util import Pipeline, match_array_name

if TYPE_CHECKING:
    from collections.abc import Callable, Mapping

    from tapa.task import Task
    from tapa.verilog.xilinx.module import Module


def _mc(name: str, expr: Expression) -> ModuleConnection:
    return ModuleConnection(
        name=name, hierarchical_name=HierarchicalName.get_name(name), expr=expr
    )


def _get_control_connections(instance_name: str) -> list[ModuleConnection]:
    return [
        _mc("ap_clk", Expression((Token.new_id("ap_clk"),))),
        _mc("ap_rst_n", Expression((Token.new_id("ap_rst_n"),))),
        *(
            _mc(sig, Expression((Token.new_id(f"{instance_name}__{sig}"),)))
            for sig in ("ap_start", "ap_done", "ap_ready", "ap_idle")
        ),
    ]


def _connect_scalar(
    arg: Instance.Arg,
    arg_table: Mapping[str, Pipeline],
) -> list[ModuleConnection]:
    expr = Expression.from_str_to_tokens(arg.name)
    if not expr.is_all_literals():
        expr = Expression((Token.new_id(arg_table[arg.name][-1].name),))
    return [_mc(arg.port, expr)]


def _connect_istream(
    arg: Instance.Arg,
    get_stream_port: Callable[[str, str], str | None],
) -> list[ModuleConnection]:
    connections = []
    for suffix in ISTREAM_SUFFIXES:
        leaf_port = get_stream_port(arg.port, suffix)
        assert leaf_port is not None
        expr = Expression((Token.new_id(get_stream_port_name(arg.name, suffix)),))
        connections.append(_mc(leaf_port, expr))
        if STREAM_PORT_DIRECTION[suffix] == "input":
            peek_port = get_stream_port(arg.port, f"_peek{suffix}")
            if peek_port is not None:
                connections.append(_mc(peek_port, expr))
    return connections


def _connect_ostream(
    arg: Instance.Arg,
    get_stream_port: Callable[[str, str], str | None],
) -> list[ModuleConnection]:
    connections = []
    for suffix in OSTREAM_SUFFIXES:
        leaf_port = get_stream_port(arg.port, suffix)
        assert leaf_port is not None
        connections.append(
            _mc(
                leaf_port,
                Expression((Token.new_id(get_stream_port_name(arg.name, suffix)),)),
            )
        )
    return connections


def _connect_mmap(
    arg: Instance.Arg,
    arg_table: Mapping[str, Pipeline],
    has_port: Callable[[str], bool],
) -> list[ModuleConnection]:
    connections = []
    for suffix in M_AXI_SUFFIXES:
        full_port_name = get_m_axi_port_name(arg.port, suffix)
        if not has_port(full_port_name):
            continue
        connections.append(
            _mc(
                full_port_name,
                Expression((Token.new_id(get_m_axi_port_name(arg.name, suffix)),)),
            )
        )
    offset_port_name = f"{arg.port}_offset"
    connections.append(
        _mc(
            offset_port_name,
            Expression((Token.new_id(arg_table[arg.name][-1].name),)),
        )
    )
    return connections


def _get_task_inst_connections(
    instance_name: str,
    args: tuple[Instance.Arg, ...],
    arg_table: Mapping[str, Pipeline],
    get_stream_port: Callable[[str, str], str | None],
    has_port: Callable[[str], bool],
) -> list[ModuleConnection]:
    connections = []
    for arg in args:
        if arg.cat == Instance.Arg.Cat.SCALAR:
            connections.extend(_connect_scalar(arg, arg_table))
        elif arg.cat == Instance.Arg.Cat.ISTREAM:
            connections.extend(_connect_istream(arg, get_stream_port))
        elif arg.cat == Instance.Arg.Cat.OSTREAM:
            connections.extend(_connect_ostream(arg, get_stream_port))
        else:
            assert arg.cat == Instance.Arg.Cat.MMAP, arg.cat
            connections.extend(_connect_mmap(arg, arg_table, has_port))

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


def _make_fsm_inst(
    fsm_module: Module,
    floorplan_region: str | None,
) -> ModuleInstantiation:
    """Build a self-connected FSM instantiation."""
    name = f"{fsm_module.name}_0"
    return ModuleInstantiation(
        name=name,
        hierarchical_name=HierarchicalName.get_name(name),
        module=fsm_module.name,
        connections=tuple(
            _mc(port, Expression((Token.new_id(port),))) for port in fsm_module.ports
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
    arg_table = get_task_arg_table(upper_task)
    ir_insts = [
        get_submodule_inst(subtasks, inst, arg_table, floorplan_region)
        for inst in upper_task.instances
    ]
    ir_insts.append(_make_fsm_inst(upper_task.fsm_module, floorplan_region))
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
    arg_table = get_task_arg_table(top_task)
    ir_insts = [
        get_top_level_slot_inst(
            slot_defs[inst.task.name],
            inst,
            arg_table[inst.name],
            floorplan_task_name_region_mapping,
        )
        for inst in top_task.instances
    ]
    ir_insts.append(_make_fsm_inst(top_task.fsm_module, fsm_floorplan_region))
    for fifo_name, fifo in top_task.fifos.items():
        if is_fifo_external_codegen(top_task, fifo_name):
            continue
        ir_insts.append(
            get_fifo_inst(
                top_task,
                fifo_name,
                fifo,
                slot_defs,
                is_top=True,
                floorplan_region=floorplan_task_name_region_mapping[
                    fifo["consumed_by"][0]
                ],
            )
        )
    return ir_insts
