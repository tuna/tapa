"""Wire-building helpers for GraphIR conversion."""

from __future__ import annotations

from typing import TYPE_CHECKING

from tapa.graphir.types import (
    AnyModuleDefinition,
    HierarchicalName,
    ModuleNet,
    ModulePort,
)
from tapa.graphir_conversion.pipeline.fifo_builder import infer_fifo_data_range
from tapa.graphir_conversion.utils import get_stream_port_name, get_task_arg_table
from tapa.protocol import (
    ISTREAM_SUFFIXES,
    OSTREAM_SUFFIXES,
    STREAM_DATA_SUFFIXES,
)
from tapa.task_codegen.fifos import is_fifo_external as is_fifo_external_codegen
from tapa.verilog.util import sanitize_array_name

if TYPE_CHECKING:
    from collections.abc import Mapping, Sequence

    from tapa.task import Task


def get_upper_task_ir_wires(
    upper_task: Task,
    submodule_ir_defs: Mapping[str, AnyModuleDefinition],
    upper_task_ir_ports: list[ModulePort],
    ctrl_s_axi_ir_ports: Sequence[ModulePort] = (),
    is_top: bool = False,
) -> list[ModuleNet]:
    """Get upper-task module wires."""
    connections = []
    for fifo_name, fifo in upper_task.fifos.items():
        if is_fifo_external_codegen(upper_task, fifo_name):
            continue
        fifo_name_no_bracket = sanitize_array_name(fifo_name)
        fifo_data_range = infer_fifo_data_range(
            fifo_name, fifo, submodule_ir_defs, upper_task, not is_top
        )
        for suffix in ISTREAM_SUFFIXES + OSTREAM_SUFFIXES:
            wire_name = get_stream_port_name(fifo_name_no_bracket, suffix)
            connections.append(
                ModuleNet(
                    name=wire_name,
                    hierarchical_name=HierarchicalName.get_name(wire_name),
                    range=fifo_data_range if suffix in STREAM_DATA_SUFFIXES else None,
                )
            )

    arg_table = get_task_arg_table(upper_task)
    port_range_mapping = {
        port.name: port.range for port in [*upper_task_ir_ports, *ctrl_s_axi_ir_ports]
    }
    for inst_arg_table in arg_table.values():
        for arg, q in inst_arg_table.items():
            port_range_key = arg if arg in port_range_mapping else f"{arg}_offset"
            wire_name = q[-1].name
            connections.append(
                ModuleNet(
                    name=wire_name,
                    hierarchical_name=HierarchicalName.get_name(wire_name),
                    range=port_range_mapping[port_range_key],
                )
            )

    for inst in upper_task.instances:
        for signal in ("ap_start", "ap_done", "ap_ready", "ap_idle"):
            wire_name = f"{inst.name}__{signal}"
            connections.append(
                ModuleNet(
                    name=wire_name,
                    hierarchical_name=HierarchicalName.get_name(wire_name),
                    range=None,
                )
            )
    return connections
