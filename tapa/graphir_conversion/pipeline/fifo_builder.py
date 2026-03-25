"""FIFO-specific GraphIR builders."""

from __future__ import annotations

from typing import TYPE_CHECKING

from tapa.graphir.types import (
    AnyModuleDefinition,
    Expression,
    HierarchicalName,
    ModuleConnection,
    ModuleInstantiation,
    Range,
    Token,
)
from tapa.graphir_conversion.utils import get_stream_port_name
from tapa.task_codegen.fifos import get_connection_to as get_connection_to_codegen
from tapa.verilog.util import sanitize_array_name
from tapa.verilog.xilinx.const import STREAM_DATA_SUFFIXES

if TYPE_CHECKING:
    from collections.abc import Mapping

    from tapa.task import Task

_FIFO_MODULE_NAME = "fifo"


def _mc(name: str, expr: Expression) -> ModuleConnection:
    return ModuleConnection(
        name=name, hierarchical_name=HierarchicalName.get_name(name), expr=expr
    )


def infer_fifo_data_range(
    fifo_name: str,
    fifo: dict,
    leaf_ir_defs: Mapping[str, AnyModuleDefinition],
    slot: Task,
    infer_port_name_from_tapa_module: bool = True,
) -> Range | None:
    """Infer the range of a fifo data."""
    consumer = fifo["consumed_by"][0]
    producer = fifo["produced_by"][0]
    assert isinstance(consumer, str)
    assert isinstance(producer, str)
    assert consumer in slot.tasks
    assert producer in slot.tasks
    producer_task_name, _, producer_fifo = get_connection_to_codegen(
        slot, fifo_name, "produced_by"
    )
    consumer_task_name, _, _consumer_fifo = get_connection_to_codegen(
        slot, fifo_name, "consumed_by"
    )

    subtasks = {inst.task.name: inst.task for inst in slot.instances}
    assert producer_task_name in subtasks
    assert consumer_task_name in subtasks

    if infer_port_name_from_tapa_module:
        producer_data_port = (
            subtasks[producer_task_name]
            .module.get_port_of(producer_fifo, STREAM_DATA_SUFFIXES[1])
            .name
        )
    else:
        producer_data_port = get_stream_port_name(
            producer_fifo, STREAM_DATA_SUFFIXES[1]
        )

    return leaf_ir_defs[producer_task_name].get_port(producer_data_port).range


def _get_fifo_data_width(fifo_range: Range) -> Expression:
    return Expression(
        (
            Token.new_lit("("),
            *fifo_range.left.root,
            Token.new_lit(")"),
            Token.new_lit("-"),
            Token.new_lit("("),
            *fifo_range.right.root,
            Token.new_lit(")"),
            Token.new_lit("+"),
            Token.new_lit("1"),
        )
    )


def _get_fifo_connections(
    fifo_name_no_bracket: str,
    is_top: bool,
) -> tuple[ModuleConnection, ...]:
    n = fifo_name_no_bracket
    reset_expr = (
        Expression((Token.new_id("rst"),))
        if is_top
        else Expression((Token.new_lit("~"), Token.new_id("ap_rst_n")))
    )
    one = Expression((Token.new_lit("1'b1"),))
    return (
        _mc("clk", Expression((Token.new_id("ap_clk"),))),
        _mc("reset", reset_expr),
        *(
            _mc(port, Expression((Token.new_id(sig),)))
            for port, sig in (
                ("if_dout", f"{n}_dout"),
                ("if_empty_n", f"{n}_empty_n"),
                ("if_read", f"{n}_read"),
                ("if_din", f"{n}_din"),
                ("if_full_n", f"{n}_full_n"),
                ("if_write", f"{n}_write"),
            )
        ),
        _mc("if_read_ce", one),
        _mc("if_write_ce", one),
    )


def get_fifo_inst(  # noqa: PLR0913
    upper_task: Task,
    fifo_name: str,
    fifo: dict,
    submodule_ir_defs: Mapping[str, AnyModuleDefinition],
    *,
    is_top: bool = False,
    floorplan_region: str | None = None,
) -> ModuleInstantiation:
    """Get slot fifo module instantiation."""
    depth = int(fifo["depth"])
    addr_width = max(1, (depth - 1).bit_length())
    fifo_range = infer_fifo_data_range(
        fifo_name, fifo, submodule_ir_defs, upper_task, not is_top
    )
    assert fifo_range is not None

    def _lit(val: str) -> Expression:
        return Expression((Token.new_lit(val),))

    fifo_name_no_bracket = sanitize_array_name(fifo_name)
    return ModuleInstantiation(
        name=fifo_name_no_bracket,
        hierarchical_name=HierarchicalName.get_name(fifo_name_no_bracket),
        module=_FIFO_MODULE_NAME,
        connections=_get_fifo_connections(fifo_name_no_bracket, is_top),
        parameters=(
            _mc("DEPTH", _lit(str(depth))),
            _mc("ADDR_WIDTH", _lit(str(addr_width))),
            _mc("DATA_WIDTH", _get_fifo_data_width(fifo_range)),
        ),
        floorplan_region=floorplan_region,
        area=None,
    )
