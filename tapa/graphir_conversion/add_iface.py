"""Add tapa graphir interface."""

__copyright__ = """
Copyright (c) 2025 RapidStream Design Automation, Inc. and contributors.
All rights reserved. The contributor(s) of this file has/have agreed to the
RapidStream Contributor License Agreement.
"""

from collections import defaultdict
from collections.abc import Collection

from tapa.graphir.types import (
    AnyInterface,
    Interfaces,
    Project,
)
from tapa.graphir_conversion.pipeline.iface_builders import (
    get_ctrl_s_axi_ifaces,
    get_fifo_ifaces,
    get_fsm_ifaces,
    get_reset_inverter_ifaces,
    get_slot_task_ifaces,
    get_top_task_ifaces,
    make_handshake_iface,
)
from tapa.graphir_conversion.pipeline.iface_roles import set_iface_role
from tapa.graphir_conversion.utils import get_m_axi_port_name
from tapa.task import Task
from tapa.verilog.util import sanitize_array_name
from tapa.verilog.xilinx.const import (
    HANDSHAKE_CLK,
    HANDSHAKE_RST_N,
    ISTREAM_SUFFIXES,
    OSTREAM_SUFFIXES,
)
from tapa.verilog.xilinx.m_axi import M_AXI_SUFFIXES_BY_CHANNEL


def _apply_iface_roles(
    project: Project,
    ifaces: defaultdict[str, list[AnyInterface]],
) -> None:
    for module in project.modules.module_definitions:
        if module.name in ifaces:
            ifaces[module.name] = [
                set_iface_role(module, iface) for iface in ifaces[module.name]
            ]


def _validate_top_submodule_ifaces(
    project: Project,
    ifaces: defaultdict[str, list[AnyInterface]],
) -> None:
    top_submodules = [
        project.get_module(inst.module) for inst in project.get_top_module().submodules
    ]
    for module in top_submodules:
        module_ifaces = ifaces[module.name]
        for port in module.ports:
            if not any(port.name in iface.ports for iface in module_ifaces):
                msg = (
                    f"Port {port.name} of module {module.name} not found in interfaces."
                )
                raise ValueError(msg)


def get_graphir_iface(
    project: Project,
    slot_tasks: Collection[Task],
    top_task: Task,
) -> Interfaces:
    """Add tapa graphir interface."""
    ifaces = defaultdict(list)
    scalars: dict[str, list[str]] = {}

    for slot_task in slot_tasks:
        slot_ifaces, slot_scalars = get_upper_task_ir_ifaces_and_scalars(
            project, slot_task, is_top=False
        )
        ifaces[slot_task.name] = slot_ifaces
        scalars[slot_task.name] = slot_scalars

    top_ifaces, _ = get_upper_task_ir_ifaces_and_scalars(project, top_task, is_top=True)
    ifaces[top_task.name] = top_ifaces

    project.get_module("fifo")
    ifaces["fifo"] = get_fifo_ifaces()
    fsm_name, ifaces[fsm_name] = get_fsm_ifaces(project, slot_tasks, scalars)
    ctrl_s_axi_name, ifaces[ctrl_s_axi_name] = get_ctrl_s_axi_ifaces(project)
    ifaces["reset_inverter"] = get_reset_inverter_ifaces()

    _apply_iface_roles(project, ifaces)
    _validate_top_submodule_ifaces(project, ifaces)

    return Interfaces(ifaces)


def _append_stream_iface(
    ifaces: list[AnyInterface],
    port_name: str,
    suffixes: tuple[str, ...],
    valid_port: str,
    ready_port: str,
) -> None:
    real_port_name = sanitize_array_name(port_name)
    ports = tuple(f"{real_port_name}{suffix}" for suffix in suffixes)
    ports += HANDSHAKE_CLK, HANDSHAKE_RST_N
    ifaces.append(
        make_handshake_iface(
            ports=ports,
            clk_port=HANDSHAKE_CLK,
            rst_port=HANDSHAKE_RST_N,
            valid_port=valid_port.format(port=real_port_name),
            ready_port=ready_port.format(port=real_port_name),
        )
    )


def _append_mmap_ifaces(
    ifaces: list[AnyInterface],
    scalars: list[str],
    port_name: str,
    ir_ports: Collection[str],
) -> None:
    scalars.append(f"{port_name}_offset")
    for channel in M_AXI_SUFFIXES_BY_CHANNEL.values():
        valid_port = get_m_axi_port_name(port_name, channel["valid"])
        ready_port = get_m_axi_port_name(port_name, channel["ready"])
        if valid_port not in ir_ports or ready_port not in ir_ports:
            continue
        channel_ports = [
            ir_port_name
            for suffix in channel["ports"]
            if (ir_port_name := get_m_axi_port_name(port_name, suffix)) in ir_ports
        ]
        channel_ports.extend([HANDSHAKE_CLK, HANDSHAKE_RST_N])
        ifaces.append(
            make_handshake_iface(
                ports=tuple(channel_ports),
                clk_port=HANDSHAKE_CLK,
                rst_port=HANDSHAKE_RST_N,
                valid_port=valid_port,
                ready_port=ready_port,
            )
        )


def _append_task_port_ifaces(
    ifaces: list[AnyInterface],
    scalars: list[str],
    task: Task,
    ir_ports: Collection[str],
) -> None:
    for port_name, port in task.ports.items():
        if port.cat.is_scalar:
            scalars.append(port_name)
        elif port.cat.is_istream or port.cat.is_istreams:
            _append_stream_iface(
                ifaces,
                port_name,
                ISTREAM_SUFFIXES,
                valid_port=f"{port}_empty_n",
                ready_port=f"{port}_read",
            )
        elif port.cat.is_ostream or port.cat.is_ostreams:
            _append_stream_iface(
                ifaces,
                port_name,
                OSTREAM_SUFFIXES,
                valid_port=f"{port}_write",
                ready_port=f"{port}_full_n",
            )
        elif port.cat.is_mmap:
            _append_mmap_ifaces(ifaces, scalars, port_name, ir_ports)
        else:
            msg = (
                f"Unsupported port category {port.cat} for port "
                f"{port_name} in task {task.name}"
            )
            raise ValueError(msg)


def get_upper_task_ir_ifaces_and_scalars(
    project: Project,
    task: Task,
    is_top: bool,
) -> tuple[list[AnyInterface], list[str]]:
    """Get the interface of the upper task IR."""
    ifaces: list[AnyInterface] = []
    scalars: list[str] = []
    task_ir = project.get_module(task.name)
    ir_ports = frozenset(port.name for port in task_ir.ports)
    _append_task_port_ifaces(ifaces, scalars, task, ir_ports)
    ifaces.extend(get_top_task_ifaces() if is_top else get_slot_task_ifaces(scalars))

    return ifaces, scalars
