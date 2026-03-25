"""Graphir conversion utilities."""

from pyverilog.vparser.ast import (
    And,
    Divide,
    Eq,
    GreaterEq,
    GreaterThan,
    Land,
    LessEq,
    LessThan,
    Lor,
    Minus,
    Mod,
    Node,
    NotEq,
    Or,
    Plus,
    Power,
    Sll,
    Sra,
    Srl,
    Times,
    Xnor,
    Xor,
)

from tapa.graphir.types import (
    Expression,
    HierarchicalName,
    ModuleParameter,
    ModulePort,
    Range,
    Token,
    VerilogModuleDefinition,
)
from tapa.instance import Port
from tapa.task import Task
from tapa.verilog.util import Pipeline
from tapa.verilog.xilinx.const import ISTREAM_SUFFIXES, OSTREAM_SUFFIXES
from tapa.verilog.xilinx.m_axi import M_AXI_PREFIX, M_AXI_SUFFIXES
from tapa.verilog.xilinx.module import Module

PORT_TYPE_MAPPING = {
    "input": ModulePort.Type.INPUT,
    "output": ModulePort.Type.OUTPUT,
    "inout": ModulePort.Type.INOUT,
}

_OPERATOR_SYMBOLS: dict[type[Node], str] = {
    Plus: "+",
    Minus: "-",
    Times: "*",
    Divide: "/",
    Mod: "%",
    Power: "**",
    Eq: "==",
    NotEq: "!=",
    GreaterThan: ">",
    LessThan: "<",
    GreaterEq: ">=",
    LessEq: "<=",
    Land: "&&",
    Lor: "||",
    And: "&",
    Or: "|",
    Xor: "^",
    Xnor: "~^",
    Sll: "<<",
    Srl: ">>",
    Sra: ">>>",
}


def get_operator_token(node: Node) -> Token:
    """Map AST node to an operator symbol token."""
    return Token.new_lit(_OPERATOR_SYMBOLS[type(node)])


def get_task_graphir_ports(task_module: Module) -> list[ModulePort]:
    """Get the graphir ports from a task."""
    assert task_module.ports
    result = []
    for name, port in task_module.ports.items():
        port_range = None
        if port.width:
            port_range = Range(
                left=Expression.from_str_to_tokens(port.width.msb),
                right=Expression.from_str_to_tokens(port.width.lsb),
            )
            assert port_range.left, type(port.width.msb)
        result.append(
            ModulePort(
                name=name,
                hierarchical_name=HierarchicalName.get_name(port.name),
                type=PORT_TYPE_MAPPING[port.direction],
                range=port_range,
            )
        )
    return result


def get_task_graphir_parameters(task_module: Module) -> list[ModuleParameter]:
    """Get the graphir parameters from a task."""
    return [
        ModuleParameter(
            name=name,
            hierarchical_name=HierarchicalName.get_name(param.name),
            expr=Expression.from_str_to_tokens(param.value),
            range=None,
        )
        for name, param in task_module.params.items()
    ]


def get_child_port_connection_mapping(
    task_port: Port,
    task_module: Module,
    arg: str,
    idx: int | None,
) -> dict[str, str]:
    """Get child task port and slot port mapping.

    Given a child task port and its arg, find all related ports in the task module based
    on cat. Return a mapping from the child module port name to the connected parent
    slot port name. This is for inferring the slot port range and direction from its
    connected child port.
    """
    if task_port.cat.is_scalar:
        return {task_port.name: arg}

    if task_port.cat.is_istream or task_port.cat.is_istreams:
        full_port_name = task_port.name if idx is None else f"{task_port.name}_{idx}"
        return {
            task_module.get_port_of(full_port_name, suffix).name: get_stream_port_name(
                arg, suffix
            )
            for suffix in ISTREAM_SUFFIXES
        }

    if task_port.cat.is_ostream or task_port.cat.is_ostreams:
        full_port_name = task_port.name if idx is None else f"{task_port.name}_{idx}"
        return {
            task_module.get_port_of(full_port_name, suffix).name: get_stream_port_name(
                arg, suffix
            )
            for suffix in OSTREAM_SUFFIXES
        }

    if task_port.cat.is_mmap:
        mapping = {f"{task_port.name}_offset": f"{arg}_offset"}
        for suffix in M_AXI_SUFFIXES:
            m_axi_port_name = get_m_axi_port_name(task_port.name, suffix)
            if m_axi_port_name in task_module.ports:
                mapping[m_axi_port_name] = get_m_axi_port_name(arg, suffix)
        return mapping

    msg = (
        f"Unknown port type for port {task_port.name}, "
        f"category {task_port.cat}, arg {arg}."
    )
    raise ValueError(msg)


def get_stream_port_name(task_port_name: str, suffix: str) -> str:
    """Get the stream port name from the task port name and suffix."""
    return f"{task_port_name}{suffix}"


def get_m_axi_port_name(task_port_name: str, suffix: str) -> str:
    """Get the m_axi port name from the task port name and suffix."""
    return f"{M_AXI_PREFIX}{task_port_name}{suffix}"


def get_task_arg_table(
    task: Task,
) -> dict[str, dict[str, Pipeline]]:
    """Build arg table for fsm pipeline signals.

    The upper key is instance name, the lower key is the arg name.
    """
    arg_table: dict[str, dict[str, Pipeline]] = {}
    for instance in task.instances:
        inst_table: dict[str, Pipeline] = {}
        for arg in instance.args:
            # Skip port connected to lit
            if Expression.from_str_to_tokens(arg.name).is_all_literals():
                continue

            if not arg.cat.is_stream:
                # For mmap ports, the scalar port is the offset.
                upper_name = (
                    f"{arg.name}_offset"
                    if arg.cat.is_sync_mmap or arg.cat.is_async_mmap
                    else arg.name
                )
                id_name = "64'd0" if arg.chan_count is not None else upper_name
                q = Pipeline(
                    name=instance.get_instance_arg(id_name),
                )
                inst_table[arg.name] = q
        arg_table[instance.name] = inst_table
    return arg_table


def get_verilog_definition_from_tapa_module(
    module: Module, code: str | None = None
) -> VerilogModuleDefinition:
    """Convert a Tapa Module to a VerilogModuleDefinition."""
    return VerilogModuleDefinition(
        name=module.name,
        hierarchical_name=HierarchicalName.get_name(module.name),
        parameters=tuple(get_task_graphir_parameters(module)),
        ports=tuple(get_task_graphir_ports(module)),
        verilog=code if code else module.code,
        submodules_module_names=(),
    )
