"""Child-instantiation helpers extracted from program orchestration."""

# ruff: noqa: F401, C901, PLR0912, PLR0914, PLR0915, TC001, ANN401, RUF052

from __future__ import annotations

from typing import TYPE_CHECKING, Any

from pyverilog.ast_code_generator.codegen import ASTCodeGenerator
from pyverilog.vparser.ast import (
    Constant,
    Identifier,
    IntConst,
    NonblockingSubstitution,
    PortArg,
)

from tapa.instance import Instance
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
    ISTREAM_SUFFIXES,
    RST,
    RST_N,
    STATE,
    TRUE,
)
from tapa.verilog.xilinx.module import generate_m_axi_ports

if TYPE_CHECKING:
    from tapa.task import Task

_CODEGEN = ASTCodeGenerator()

STATE00 = IntConst("2'b00")
STATE01 = IntConst("2'b01")
STATE11 = IntConst("2'b11")
STATE10 = IntConst("2'b10")


def instantiate_children_tasks(
    program: Any,
    task: Task,
    width_table: dict[str, int],
) -> list[Pipeline]:
    _logger = getattr(program, "_logger", None)
    if _logger is not None:
        _logger.debug("  instantiating children tasks in %s", task.name)
    is_done_signals: list[Pipeline] = []
    arg_table: dict[str, Pipeline] = {}
    async_mmap_args: dict[Instance.Arg, list[str]] = {}

    task.add_m_axi(width_table, program.files)

    fsm_upstream_portargs: list[PortArg] = [
        make_port_arg(x, x) for x in HANDSHAKE_INPUT_PORTS + HANDSHAKE_OUTPUT_PORTS
    ]
    fsm_upstream_module_ports = {}
    fsm_downstream_portargs: list[PortArg] = []
    fsm_downstream_module_ports = []

    for instance in task.instances:
        child_port_set = set(instance.task.module.ports)
        for arg in instance.args:
            if arg.cat.is_stream:
                continue
            width = 64
            if arg.cat.is_scalar:
                width = width_table.get(arg.name, 0)
                if width == 0:
                    width = int(arg.name.split("'d")[0])
            upper_name = (
                f"{arg.name}_offset"
                if arg.cat.is_sync_mmap or arg.cat.is_async_mmap
                else arg.name
            )
            id_name = "64'd0" if arg.chan_count is not None else upper_name
            q = Pipeline(name=instance.get_instance_arg(id_name), width=width)
            arg_table[arg.name] = q
            if "'d" not in q.name:
                task.module.add_signals([Wire(q[-1].name, Width.create(width))])
                task.fsm_module.add_pipeline(q, init=Identifier(id_name))
                if _logger is not None:
                    _logger.debug("    pipelined signal: %s => %s", id_name, q.name)
                fsm_upstream_module_ports.setdefault(
                    upper_name, IOPort("input", upper_name, Width.create(width))
                )
                fsm_downstream_module_ports.append(
                    IOPort("output", q[-1].name, Width.create(width))
                )
                fsm_downstream_portargs.append(make_port_arg(q[-1].name, q[-1].name))
            if arg.cat.is_async_mmap:
                for tag in ASYNC_MMAP_SUFFIXES:
                    if {
                        x.portname
                        for x in generate_async_mmap_ports(
                            tag=tag,
                            port=arg.port,
                            arg=upper_name,
                            offset_name=arg_table[arg.name][-1].name,
                            instance=instance,
                        )
                    } & child_port_set:
                        async_mmap_args.setdefault(arg, []).append(tag)
            for tag in async_mmap_args.get(arg, []):
                if task.is_upper and instance.task.is_lower:
                    task.module.add_signals(
                        generate_async_mmap_signals(
                            tag=tag,
                            arg=arg.mmap_name,
                            data_width=width_table[arg.name],
                        ),
                    )
                else:
                    task.module.add_ports(
                        generate_async_mmap_ioports(
                            tag=tag,
                            arg=upper_name,
                            data_width=width_table[arg.name],
                        ),
                    )

        start_q = Pipeline(f"{instance.start.name}_global")
        task.fsm_module.add_pipeline(start_q, program.start_q[0])
        if instance.is_autorun:
            task.fsm_module.add_logics(
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
        else:
            is_done_q = Pipeline(f"{instance.is_done.name}")
            done_q = Pipeline(f"{instance.done.name}_global")
            task.fsm_module.add_pipeline(is_done_q, instance.is_state(STATE10))
            task.fsm_module.add_pipeline(done_q, program.done_q[0])
            if_branch = instance.set_state(STATE00)
            else_branch = (
                make_if_with_block(
                    cond=instance.is_state(STATE00),
                    true=make_if_with_block(
                        cond=start_q[-1], true=instance.set_state(STATE01)
                    ),
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
                    true=make_if_with_block(
                        cond=done_q[-1], true=instance.set_state(STATE00)
                    ),
                ),
            )
            task.fsm_module.add_logics(
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
            is_done_signals.append(is_done_q)

        fsm_downstream_portargs.extend(
            make_port_arg(x.name, x.name) for x in instance.public_handshake_signals
        )
        task.module.add_signals(
            Wire(x.name, x.width) for x in instance.public_handshake_signals
        )
        task.fsm_module.add_signals(instance.all_handshake_signals)
        fsm_downstream_module_ports.extend(instance.public_handshake_ports)

        portargs = list(generate_handshake_ports(instance, RST_N))
        for arg in instance.args:
            if arg.cat.is_scalar:
                portargs.append(
                    PortArg(portname=arg.port, argname=arg_table[arg.name][-1]),
                )
            elif arg.cat.is_istream:
                portargs.extend(
                    instance.task.module.generate_istream_ports(
                        port=arg.port,
                        arg=arg.name,
                        ignore_peek_fifos=(
                            (arg.port,) if instance.task.is_slot else ()
                        ),
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
                        arg_reg=arg_table[arg.name][-1].name,
                    ),
                )
            elif arg.cat.is_async_mmap:
                for tag in async_mmap_args[arg]:
                    portargs.extend(
                        generate_async_mmap_ports(
                            tag=tag,
                            port=arg.port,
                            arg=arg.mmap_name,
                            offset_name=arg_table[arg.name][-1].name,
                            instance=instance,
                        ),
                    )

        task.module.add_instance(
            module_name=get_module_name(instance.task.name),
            instance_name=instance.name,
            ports=portargs,
        )

    fsm_upstream_portargs.extend(
        [make_port_arg(x.name, x.name) for x in fsm_upstream_module_ports.values()]
    )
    task.fsm_module.add_ports(fsm_upstream_module_ports.values())
    task.fsm_module.add_ports(fsm_downstream_module_ports)
    task.add_rs_pragmas_to_fsm()

    addr_width = 64
    if _logger is not None:
        _logger.debug("Set the address width of async_mmap to %d", addr_width)
    if task.is_upper:
        for arg, tag in async_mmap_args.items():
            task.module.add_async_mmap_instance(
                name=arg.mmap_name,
                tags=tag,
                rst=RST,
                data_width=width_table[arg.name],
                addr_width=addr_width,
            )
        task.module.add_instance(
            module_name=task.fsm_module.name,
            instance_name="__tapa_fsm_unit",
            ports=fsm_upstream_portargs + fsm_downstream_portargs,
        )
    return is_done_signals
