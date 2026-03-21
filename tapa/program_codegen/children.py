"""Child-instantiation helpers extracted from program orchestration."""

from __future__ import annotations

from dataclasses import dataclass
from typing import TYPE_CHECKING, Protocol

from pyverilog.ast_code_generator.codegen import ASTCodeGenerator
from pyverilog.vparser.ast import Identifier, IntConst, NonblockingSubstitution, PortArg

from tapa.util import get_module_name
from tapa.verilog.ast.ioport import IOPort
from tapa.verilog.ast.logic import Always, Assign
from tapa.verilog.ast.signal import Wire
from tapa.verilog.ast.width import Width
from tapa.verilog.ast_utils import make_block, make_if_with_block, make_port_arg
from tapa.verilog.util import Pipeline
from tapa.verilog.xilinx import generate_handshake_ports
from tapa.verilog.xilinx.async_mmap import (
    ASYNC_MMAP_SUFFIXES,
    generate_async_mmap_ioports,
    generate_async_mmap_ports,
    generate_async_mmap_signals,
)
from tapa.verilog.xilinx.const import (
    CLK_SENS_LIST,
    FALSE,
    HANDSHAKE_INPUT_PORTS,
    HANDSHAKE_OUTPUT_PORTS,
    RST,
    RST_N,
    TRUE,
)
from tapa.verilog.xilinx.module import generate_m_axi_ports

if TYPE_CHECKING:
    from tapa.instance import Instance
    from tapa.task import Task


class _ProgramLike(Protocol):
    files: dict[str, str]
    start_q: Pipeline
    done_q: Pipeline


_CODEGEN = ASTCodeGenerator()

STATE00 = IntConst("2'b00")
STATE01 = IntConst("2'b01")
STATE11 = IntConst("2'b11")
STATE10 = IntConst("2'b10")


@dataclass
class _ChildState:
    program: _ProgramLike
    task: Task
    width_table: dict[str, int]
    is_done_signals: list[Pipeline]
    arg_table: dict[str, Pipeline]
    async_mmap_args: dict[Instance.Arg, list[str]]
    fsm_upstream_portargs: list[PortArg]
    fsm_upstream_module_ports: dict[str, IOPort]
    fsm_downstream_portargs: list[PortArg]
    fsm_downstream_module_ports: list[IOPort]


def _new_state(
    program: _ProgramLike, task: Task, width_table: dict[str, int]
) -> _ChildState:
    return _ChildState(
        program=program,
        task=task,
        width_table=width_table,
        is_done_signals=[],
        arg_table={},
        async_mmap_args={},
        fsm_upstream_portargs=[
            make_port_arg(x, x) for x in HANDSHAKE_INPUT_PORTS + HANDSHAKE_OUTPUT_PORTS
        ],
        fsm_upstream_module_ports={},
        fsm_downstream_portargs=[],
        fsm_downstream_module_ports=[],
    )


def _resolve_arg_width(width_table: dict[str, int], arg: Instance.Arg) -> int:
    width = width_table.get(arg.name, 0)
    if arg.cat.is_scalar and width == 0:
        return int(arg.name.split("'d")[0])
    return 64 if width == 0 else width


def _collect_async_mmap_tags(
    instance: Instance,
    arg: Instance.Arg,
    upper_name: str,
    offset_name: str,
    child_port_set: set[str],
) -> list[str]:
    tags: list[str] = []
    for tag in ASYNC_MMAP_SUFFIXES:
        port_names = {
            x.portname
            for x in generate_async_mmap_ports(
                tag=tag,
                port=arg.port,
                arg=upper_name,
                offset_name=offset_name,
                instance=instance,
            )
        }
        if port_names & child_port_set:
            tags.append(tag)
    return tags


def _declare_arg_signal(
    state: _ChildState,
    instance: Instance,
    arg: Instance.Arg,
    upper_name: str,
    width: int,
) -> Pipeline:
    id_name = "64'd0" if arg.chan_count is not None else upper_name
    q = Pipeline(name=instance.get_instance_arg(id_name), width=width)
    state.arg_table[arg.name] = q
    if "'d" not in q.name:
        state.task.module.add_signals([Wire(q[-1].name, Width.create(width))])
        state.task.fsm_module.add_pipeline(q, init=Identifier(id_name))
        state.fsm_upstream_module_ports.setdefault(
            upper_name, IOPort("input", upper_name, Width.create(width))
        )
        state.fsm_downstream_module_ports.append(
            IOPort("output", q[-1].name, Width.create(width))
        )
        state.fsm_downstream_portargs.append(make_port_arg(q[-1].name, q[-1].name))
    return q


def _bind_async_mmap_tag(
    state: _ChildState,
    instance: Instance,
    arg: Instance.Arg,
    upper_name: str,
    tag: str,
) -> None:
    if state.task.is_upper and instance.task.is_lower:
        state.task.module.add_signals(
            generate_async_mmap_signals(
                tag=tag,
                arg=arg.mmap_name,
                data_width=state.width_table[arg.name],
            ),
        )
    else:
        state.task.module.add_ports(
            generate_async_mmap_ioports(
                tag=tag,
                arg=upper_name,
                data_width=state.width_table[arg.name],
            ),
        )


def _declare_instance_inputs(
    state: _ChildState,
    instance: Instance,
    child_port_set: set[str],
) -> None:
    for arg in instance.args:
        if arg.cat.is_stream:
            continue

        upper_name = (
            f"{arg.name}_offset"
            if arg.cat.is_sync_mmap or arg.cat.is_async_mmap
            else arg.name
        )
        q = _declare_arg_signal(
            state=state,
            instance=instance,
            arg=arg,
            upper_name=upper_name,
            width=_resolve_arg_width(state.width_table, arg),
        )

        if arg.cat.is_async_mmap:
            tags = _collect_async_mmap_tags(
                instance=instance,
                arg=arg,
                upper_name=upper_name,
                offset_name=q[-1].name,
                child_port_set=child_port_set,
            )
            state.async_mmap_args.setdefault(arg, []).extend(tags)
            for tag in tags:
                _bind_async_mmap_tag(state, instance, arg, upper_name, tag)


def _declare_instance_start_logic(state: _ChildState, instance: Instance) -> None:
    start_q = Pipeline(f"{instance.start.name}_global")
    state.task.fsm_module.add_pipeline(start_q, state.program.start_q[0])
    if instance.is_autorun:
        state.task.fsm_module.add_logics(
            [
                Always(
                    sens_list=CLK_SENS_LIST,
                    statement=_CODEGEN.visit(
                        make_block(
                            make_if_with_block(
                                cond=RST,
                                true=NonblockingSubstitution(
                                    left=instance.start,
                                    right=FALSE,
                                ),
                                false=make_if_with_block(
                                    cond=start_q[-1],
                                    true=NonblockingSubstitution(
                                        left=instance.start,
                                        right=TRUE,
                                    ),
                                ),
                            ),
                        )
                    ),
                ),
            ],
        )
        return

    is_done_q = Pipeline(f"{instance.is_done.name}")
    done_q = Pipeline(f"{instance.done.name}_global")
    state.task.fsm_module.add_pipeline(is_done_q, instance.is_state(STATE10))
    state.task.fsm_module.add_pipeline(done_q, state.program.done_q[0])
    if_branch = instance.set_state(STATE00)
    else_branch = (
        make_if_with_block(
            cond=instance.is_state(STATE00),
            true=make_if_with_block(cond=start_q[-1], true=instance.set_state(STATE01)),
        ),
        make_if_with_block(
            cond=instance.is_state(STATE01),
            true=make_if_with_block(
                cond=instance.ready,
                true=make_if_with_block(
                    cond=instance.done,
                    true=instance.set_state(STATE10),
                    false=instance.set_state(STATE11),
                ),
            ),
        ),
        make_if_with_block(
            cond=instance.is_state(STATE11),
            true=make_if_with_block(
                cond=instance.done, true=instance.set_state(STATE10)
            ),
        ),
        make_if_with_block(
            cond=instance.is_state(STATE10),
            true=make_if_with_block(cond=done_q[-1], true=instance.set_state(STATE00)),
        ),
    )
    state.task.fsm_module.add_logics(
        [
            Always(
                sens_list=CLK_SENS_LIST,
                statement=_CODEGEN.visit(
                    make_block(
                        make_if_with_block(
                            cond=RST,
                            true=if_branch,
                            false=else_branch,
                        ),
                    )
                ),
            ),
            Assign(
                lhs=instance.start.name,
                rhs=_CODEGEN.visit(instance.is_state(STATE01)),
            ),
        ],
    )
    state.is_done_signals.append(is_done_q)


def _declare_instance_handshake_signals(state: _ChildState, instance: Instance) -> None:
    state.fsm_downstream_portargs.extend(
        make_port_arg(x.name, x.name) for x in instance.public_handshake_signals
    )
    state.task.module.add_signals(
        Wire(x.name, x.width) for x in instance.public_handshake_signals
    )
    state.task.fsm_module.add_signals(instance.all_handshake_signals)
    state.fsm_downstream_module_ports.extend(instance.public_handshake_ports)


def _build_instance_portargs(state: _ChildState, instance: Instance) -> list[PortArg]:
    portargs = list(generate_handshake_ports(instance, RST_N))
    for arg in instance.args:
        if arg.cat.is_scalar:
            portargs.append(
                PortArg(portname=arg.port, argname=state.arg_table[arg.name][-1]),
            )
        elif arg.cat.is_istream:
            portargs.extend(
                instance.task.module.generate_istream_ports(
                    port=arg.port,
                    arg=arg.name,
                    ignore_peek_fifos=((arg.port,) if instance.task.is_slot else ()),
                ),
            )
        elif arg.cat.is_ostream:
            portargs.extend(
                instance.task.module.generate_ostream_ports(
                    port=arg.port,
                    arg=arg.name,
                ),
            )
        elif arg.cat.is_sync_mmap:
            portargs.extend(
                generate_m_axi_ports(
                    module=instance.task.module,
                    port=arg.port,
                    arg=arg.mmap_name,
                    arg_reg=state.arg_table[arg.name][-1].name,
                ),
            )
        elif arg.cat.is_async_mmap:
            for tag in state.async_mmap_args[arg]:
                portargs.extend(
                    generate_async_mmap_ports(
                        tag=tag,
                        port=arg.port,
                        arg=arg.mmap_name,
                        offset_name=state.arg_table[arg.name][-1].name,
                        instance=instance,
                    ),
                )
    return portargs


def _declare_instance_ports(state: _ChildState, instance: Instance) -> None:
    state.task.module.add_instance(
        module_name=get_module_name(instance.task.name),
        instance_name=instance.name,
        ports=_build_instance_portargs(state, instance),
    )


def _process_instance(state: _ChildState, instance: Instance) -> None:
    child_port_set = set(instance.task.module.ports)
    _declare_instance_inputs(state, instance, child_port_set)
    _declare_instance_start_logic(state, instance)
    _declare_instance_handshake_signals(state, instance)
    _declare_instance_ports(state, instance)


def _finalize_state(state: _ChildState) -> list[Pipeline]:
    state.fsm_upstream_portargs.extend(
        [
            make_port_arg(x.name, x.name)
            for x in state.fsm_upstream_module_ports.values()
        ]
    )
    state.task.fsm_module.add_ports(state.fsm_upstream_module_ports.values())
    state.task.fsm_module.add_ports(state.fsm_downstream_module_ports)
    state.task.add_rs_pragmas_to_fsm()

    if state.task.is_upper:
        for arg, tag in state.async_mmap_args.items():
            state.task.module.add_async_mmap_instance(
                name=arg.mmap_name,
                tags=tag,
                rst=RST,
                data_width=state.width_table[arg.name],
                addr_width=64,
            )
        state.task.module.add_instance(
            module_name=state.task.fsm_module.name,
            instance_name="__tapa_fsm_unit",
            ports=state.fsm_upstream_portargs + state.fsm_downstream_portargs,
        )
    return state.is_done_signals


def instantiate_children_tasks(
    program: _ProgramLike,
    task: Task,
    width_table: dict[str, int],
) -> list[Pipeline]:
    state = _new_state(program, task, width_table)
    task.add_m_axi(width_table, program.files)
    for instance in task.instances:
        _process_instance(state, instance)
    return _finalize_state(state)
