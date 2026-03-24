"""Program orchestration helpers extracted from :mod:`tapa.core`."""

# ruff: noqa: SLF001, ANN401

from __future__ import annotations

import logging
from typing import TYPE_CHECKING, Any

from pyverilog.ast_code_generator.codegen import ASTCodeGenerator
from pyverilog.vparser.ast import (
    Constant,
    Eq,
    Identifier,
    IntConst,
    Minus,
    NonblockingSubstitution,
    Plus,
)

from tapa.common.target import Target
from tapa.program_codegen.children import (
    instantiate_children_tasks as _instantiate_children,
)
from tapa.program_codegen.custom_rtl import replace_custom_rtl as _replace_custom_rtl
from tapa.program_codegen.fifos import connect_fifos as _connect_fifos
from tapa.program_codegen.fifos import instantiate_fifos as _instantiate_fifos
from tapa.task_codegen.fifos import get_connection_to as get_connection_to_codegen
from tapa.verilog.ast.logic import Always, Assign
from tapa.verilog.ast.signal import Reg
from tapa.verilog.ast.width import Width
from tapa.verilog.ast_utils import make_block, make_case_with_block, make_if_with_block
from tapa.verilog.util import Pipeline, array_name, match_array_name
from tapa.verilog.xilinx.const import (
    CLK_SENS_LIST,
    HANDSHAKE_DONE,
    HANDSHAKE_IDLE,
    HANDSHAKE_READY,
    ISTREAM_SUFFIXES,
    OSTREAM_SUFFIXES,
    RST,
    START,
    STATE,
)
from tapa.verilog.xilinx.module import Module
from tapa.verilog.xilinx.module_ops.ports import get_streams_fifos

if TYPE_CHECKING:
    from collections.abc import Generator
    from pathlib import Path

    from tapa.task import Task

_logger = logging.getLogger().getChild(__name__)
_CODEGEN = ASTCodeGenerator()

STATE00 = IntConst("2'b00")
STATE01 = IntConst("2'b01")
STATE10 = IntConst("2'b10")


def _find_task_inst_hierarchy(
    program: Any,
    target_task: str,
    current_task: str,
    current_inst: str,
    current_hierarchy: tuple[str, ...],
) -> Generator[tuple[str, ...]]:
    if current_task == target_task:
        yield (*current_hierarchy, current_inst)
    for inst in program._tasks[current_task].instances:
        assert inst.name
        yield from _find_task_inst_hierarchy(
            program,
            target_task,
            inst.task.name,
            inst.name,
            (*current_hierarchy, current_inst),
        )


def get_rtl_templates_info(program: Any) -> dict[str, list[str]]:
    return {
        name: [str(port) for port in task.ports.values()]
        for name, task in program._tasks.items()
        if name in program.gen_templates
    }


def replace_custom_rtl(
    program: Any,
    rtl_paths: tuple[Path, ...],
    templates_info: dict[str, list[str]],
) -> None:
    _replace_custom_rtl(
        rtl_dir=program.rtl_dir,
        custom_rtl=program._get_custom_rtl_files(rtl_paths),
        templates_info=templates_info,
        tasks=program._tasks,
    )


def get_fifo_width(program: Any, task: Task, fifo: str) -> Plus:
    producer_task, _, fifo_port = get_connection_to_codegen(task, fifo, "produced_by")
    port = program.get_task(producer_task).module.get_port_of(
        fifo_port,
        OSTREAM_SUFFIXES[0],
    )
    assert port.width is not None
    return Plus(Minus(Constant(port.width.msb), Constant(port.width.lsb)), IntConst(1))


def connect_fifos(program: Any, task: Task) -> None:
    _connect_fifos(
        task=task, top=program.top, target=program.target, get_task=program.get_task
    )


def instantiate_fifos(program: Any, task: Task) -> None:
    _instantiate_fifos(task=task, get_fifo_width=program.get_fifo_width)


def instantiate_children_tasks(
    program: Any, task: Task, width_table: dict[str, int]
) -> list[Pipeline]:
    return _instantiate_children(program, task, width_table)


def instantiate_global_fsm(
    program: Any,
    module: Module,
    is_done_signals: list[Pipeline],
) -> None:
    def is_state(state: IntConst) -> Eq:
        return Eq(left=STATE, right=state)

    def set_state(state: IntConst) -> NonblockingSubstitution:
        return NonblockingSubstitution(left=STATE, right=state)

    module.add_signals([Reg(STATE.name, width=Width.create(2))])

    state01_action = set_state(STATE10)
    if is_done_signals:
        state01_action = make_if_with_block(
            cond=Identifier(" && ".join(str(x[-1]) for x in is_done_signals)),
            true=state01_action,
        )

    global_fsm = make_case_with_block(
        comp=STATE,
        cases=[
            (
                STATE00,
                make_if_with_block(cond=program.start_q[-1], true=set_state(STATE01)),
            ),
            (STATE01, state01_action),
            (STATE10, [set_state(STATE00)]),
        ],
    )

    module.add_logics(
        [
            Always(
                sens_list=CLK_SENS_LIST,
                statement=_CODEGEN.visit(
                    make_block(
                        make_if_with_block(
                            cond=RST, true=set_state(STATE00), false=global_fsm
                        )
                    )
                ),
            ),
            Assign(lhs=HANDSHAKE_IDLE, rhs=_CODEGEN.visit(is_state(STATE00))),
            Assign(lhs=HANDSHAKE_DONE, rhs=program.done_q[-1].name),
            Assign(lhs=HANDSHAKE_READY, rhs=program.done_q[0].name),
        ],
    )

    module.add_pipeline(program.start_q, init=START)
    module.add_pipeline(program.done_q, init=is_state(STATE10))


def instrument_upper_and_template_task(program: Any, task: Task) -> None:
    task.module.cleanup()
    if task.name == program.top and program.target == Target.XILINX_VITIS:
        task.module.add_rs_pragmas()

    if task.name == program.top_task.name:
        _logger.debug("remove top peek ports")
        for port_name, port in task.ports.items():
            if port.cat.is_istream:
                fifos = [port_name]
            elif port.is_istreams:
                fifos = get_streams_fifos(task.module, port_name)
            else:
                continue
            for fifo in fifos:
                for suffix in ISTREAM_SUFFIXES:
                    match = match_array_name(fifo)
                    peek_port = (
                        f"{fifo}_peek"
                        if match is None
                        else array_name(f"{match[0]}_peek", match[1])
                    )
                    try:
                        peek = task.module.get_port_of(peek_port, suffix)
                    except Module.NoMatchingPortError:
                        continue
                    _logger.debug("  remove %s", peek.name)
                    task.module.del_port(peek.name)

    if task.name in program.gen_templates:
        _logger.info("skip instrumenting template task %s", task.name)
        with open(
            program.get_rtl_template_path(task.name), "w", encoding="utf-8"
        ) as rtl_code:
            rtl_code.write(task.module.get_template_code())
    else:
        instantiate_fifos(program, task)
        connect_fifos(program, task)
        width_table = {port.name: port.width for port in task.ports.values()}
        is_done_signals = instantiate_children_tasks(program, task, width_table)
        instantiate_global_fsm(program, task.fsm_module, is_done_signals)
        with open(
            program.get_rtl_path(task.fsm_module.name), "w", encoding="utf-8"
        ) as rtl_code:
            rtl_code.write(task.fsm_module.code)
    with open(program.get_rtl_path(task.name), "w", encoding="utf-8") as rtl_code:
        rtl_code.write(task.module.code)


def get_grouping_constraints(
    program: Any, nonpipeline_fifos: list[str] | None = None
) -> list[list[str]]:
    _logger.info("Resolving grouping constraints from non-pipeline FIFOs")
    if not nonpipeline_fifos:
        return []
    grouping_constraints = []
    for task_fifo_name in nonpipeline_fifos:
        task_name, fifo_name = task_fifo_name.split(".")
        found_hierarchies = _find_task_inst_hierarchy(
            program, task_name, program.top, program.top, ()
        )
        fifo = program._tasks[task_name].fifos[fifo_name]
        consumer_task: str = fifo["consumed_by"][0]
        producer_task: str = fifo["produced_by"][0]
        for hierarchy in found_hierarchies:
            producer_inst = program.get_inst_by_port_arg_name(
                producer_task, program._tasks[task_name], fifo_name
            ).name
            consumer_inst = program.get_inst_by_port_arg_name(
                consumer_task, program._tasks[task_name], fifo_name
            ).name
            grouping_constraints.append(
                [
                    "/".join((*hierarchy, producer_inst)),
                    "/".join((*hierarchy, fifo_name)),
                    "/".join((*hierarchy, consumer_inst)),
                ]
            )
    return grouping_constraints
