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
    ApCtrlInterface,
    ClockInterface,
    FalsePathInterface,
    FeedForwardInterface,
    FeedForwardResetInterface,
    HandShakeInterface,
    Interfaces,
    Project,
)
from tapa.graphir_conversion.iface_roles import set_iface_role
from tapa.graphir_conversion.utils import get_m_axi_port_name
from tapa.task import Task
from tapa.verilog.util import sanitize_array_name
from tapa.verilog.xilinx.const import (
    HANDSHAKE_CLK,
    HANDSHAKE_DONE,
    HANDSHAKE_IDLE,
    HANDSHAKE_READY,
    HANDSHAKE_RST_N,
    HANDSHAKE_START,
    ISTREAM_ROLES,
    ISTREAM_SUFFIXES,
    OSTREAM_ROLES,
    OSTREAM_SUFFIXES,
)
from tapa.verilog.xilinx.m_axi import M_AXI_SUFFIXES_BY_CHANNEL

CTRL_S_AXI_FIXED_PORTS = (
    "ACLK",
    "ACLK_EN",
    "ARESET",
    "interrupt",
    "ARADDR",
    "ARREADY",
    "ARVALID",
    "AWADDR",
    "AWREADY",
    "AWVALID",
    "BREADY",
    "BRESP",
    "BVALID",
    "RDATA",
    "RREADY",
    "RRESP",
    "RVALID",
    "WDATA",
    "WREADY",
    "WSTRB",
    "WVALID",
    "ap_start",
    "ap_done",
    "ap_ready",
    "ap_idle",
)


def _make_handshake_iface(
    ports: tuple[str, ...],
    clk_port: str,
    rst_port: str,
    valid_port: str,
    ready_port: str,
) -> HandShakeInterface:
    return HandShakeInterface(
        ports=ports,
        clk_port=clk_port,
        rst_port=rst_port,
        valid_port=valid_port,
        ready_port=ready_port,
        origin_info="",
    )


def _get_fifo_ifaces() -> list[AnyInterface]:
    return [
        ClockInterface(ports=("clk",), origin_info=""),
        FeedForwardResetInterface(
            ports=("clk", "reset"),
            clk_port="clk",
            origin_info="",
        ),
        _make_handshake_iface(
            ports=("if_din", "if_full_n", "if_write", "clk", "reset"),
            clk_port="clk",
            rst_port="reset",
            valid_port="if_write",
            ready_port="if_full_n",
        ),
        _make_handshake_iface(
            ports=("if_dout", "if_empty_n", "if_read", "clk", "reset"),
            clk_port="clk",
            rst_port="reset",
            valid_port="if_empty_n",
            ready_port="if_read",
        ),
        FalsePathInterface(ports=("if_read_ce",), origin_info=""),
        FalsePathInterface(ports=("if_write_ce",), origin_info=""),
    ]


def _get_fsm_ifaces(
    project: Project,
    slot_tasks: Collection[Task],
    scalars: dict[str, list[str]],
) -> tuple[str, list[AnyInterface]]:
    fsm_name = f"{project.get_top_name()}_fsm"
    fsm_ir = project.get_module(fsm_name)
    fsm_ifaces: list[AnyInterface] = []
    for slot_name in scalars:
        ap_ctrl_ports = (
            HANDSHAKE_CLK,
            HANDSHAKE_RST_N,
            *tuple(
                port.name
                for port in fsm_ir.ports
                if port.name.startswith(f"{slot_name}_0")
            ),
        )
        fsm_ifaces.append(
            ApCtrlInterface(
                ports=ap_ctrl_ports,
                clk_port=HANDSHAKE_CLK,
                rst_port=HANDSHAKE_RST_N,
                ap_start_port=f"{slot_name}_0__{HANDSHAKE_START}",
                ap_done_port=f"{slot_name}_0__{HANDSHAKE_DONE}",
                ap_ready_port=f"{slot_name}_0__{HANDSHAKE_READY}",
                ap_idle_port=f"{slot_name}_0__{HANDSHAKE_IDLE}",
                ap_continue_port=None,
                origin_info="",
            )
        )

    slot_names = [slot_task.name for slot_task in slot_tasks]
    fsm_scalars = tuple(
        port.name
        for port in fsm_ir.ports
        if port.name not in {HANDSHAKE_CLK, HANDSHAKE_RST_N}
        and not any(port.name.startswith(slot_name) for slot_name in slot_names)
    )
    fsm_ifaces.append(
        ApCtrlInterface(
            ports=(
                *fsm_scalars,
                HANDSHAKE_CLK,
                HANDSHAKE_RST_N,
                HANDSHAKE_START,
                HANDSHAKE_DONE,
                HANDSHAKE_READY,
                HANDSHAKE_IDLE,
            ),
            clk_port="ap_clk",
            rst_port="ap_rst_n",
            ap_start_port=HANDSHAKE_START,
            ap_done_port=HANDSHAKE_DONE,
            ap_ready_port=HANDSHAKE_READY,
            ap_idle_port=HANDSHAKE_IDLE,
            ap_continue_port=None,
            origin_info="",
        )
    )
    return fsm_name, fsm_ifaces


def _get_ctrl_s_axi_ifaces(project: Project) -> tuple[str, list[AnyInterface]]:
    ctrl_s_axi_name = f"{project.get_top_name()}_control_s_axi"
    ctrl_s_axi_ir = project.get_module(ctrl_s_axi_name)
    ctrl_s_axi_scalars = [
        port.name
        for port in ctrl_s_axi_ir.ports
        if port.name not in CTRL_S_AXI_FIXED_PORTS
    ]
    return ctrl_s_axi_name, [
        ClockInterface(ports=("ACLK",), origin_info=""),
        FeedForwardResetInterface(
            ports=("ACLK", "ARESET"),
            clk_port="ACLK",
            origin_info="",
        ),
        FalsePathInterface(ports=("ACLK_EN",), origin_info=""),
        _make_handshake_iface(
            ports=("ARADDR", "ARREADY", "ARVALID"),
            clk_port="ACLK",
            rst_port="ARESET",
            valid_port="ARVALID",
            ready_port="ARREADY",
        ),
        _make_handshake_iface(
            ports=("AWADDR", "AWREADY", "AWVALID"),
            clk_port="ACLK",
            rst_port="ARESET",
            valid_port="AWVALID",
            ready_port="AWREADY",
        ),
        _make_handshake_iface(
            ports=("BREADY", "BRESP", "BVALID"),
            clk_port="ACLK",
            rst_port="ARESET",
            valid_port="BVALID",
            ready_port="BREADY",
        ),
        _make_handshake_iface(
            ports=("RDATA", "RREADY", "RRESP", "RVALID"),
            clk_port="ACLK",
            rst_port="ARESET",
            valid_port="RVALID",
            ready_port="RREADY",
        ),
        _make_handshake_iface(
            ports=("WDATA", "WREADY", "WSTRB", "WVALID"),
            clk_port="ACLK",
            rst_port="ARESET",
            valid_port="WVALID",
            ready_port="WREADY",
        ),
        FeedForwardInterface(
            ports=("ACLK", "ARESET", "interrupt"),
            clk_port="ACLK",
            rst_port="ARESET",
            origin_info="",
        ),
        ApCtrlInterface(
            ports=(
                *ctrl_s_axi_scalars,
                "ACLK",
                "ARESET",
                "ap_start",
                "ap_done",
                "ap_ready",
                "ap_idle",
            ),
            clk_port="ACLK",
            rst_port="ARESET",
            ap_start_port="ap_start",
            ap_done_port="ap_done",
            ap_ready_port="ap_ready",
            ap_idle_port="ap_idle",
            ap_continue_port=None,
            origin_info="",
        ),
    ]


def _get_reset_inverter_ifaces() -> list[AnyInterface]:
    return [
        ClockInterface(ports=("clk",), origin_info=""),
        FeedForwardResetInterface(
            ports=("clk", "rst_n"),
            clk_port="clk",
            origin_info="",
        ),
        FeedForwardResetInterface(
            ports=("clk", "rst"),
            clk_port="clk",
            origin_info="",
        ),
    ]


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
    ifaces["fifo"] = _get_fifo_ifaces()
    fsm_name, ifaces[fsm_name] = _get_fsm_ifaces(project, slot_tasks, scalars)
    ctrl_s_axi_name, ifaces[ctrl_s_axi_name] = _get_ctrl_s_axi_ifaces(project)
    ifaces["reset_inverter"] = _get_reset_inverter_ifaces()

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
        _make_handshake_iface(
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
    ir_ports: list[str],
) -> None:
    scalars.append(f"{port_name}_offset")
    for channel in M_AXI_SUFFIXES_BY_CHANNEL.values():
        channel_ports = [
            ir_port_name
            for suffix in channel["ports"]
            if (ir_port_name := get_m_axi_port_name(port_name, suffix)) in ir_ports
        ]
        channel_ports.extend([HANDSHAKE_CLK, HANDSHAKE_RST_N])
        ifaces.append(
            _make_handshake_iface(
                ports=tuple(channel_ports),
                clk_port=HANDSHAKE_CLK,
                rst_port=HANDSHAKE_RST_N,
                valid_port=get_m_axi_port_name(port_name, channel["valid"]),
                ready_port=get_m_axi_port_name(port_name, channel["ready"]),
            )
        )


def _append_task_port_ifaces(
    ifaces: list[AnyInterface],
    scalars: list[str],
    task: Task,
    ir_ports: list[str],
) -> None:
    for port_name, port in task.ports.items():
        if port.cat.is_scalar:
            scalars.append(port_name)
        elif port.cat.is_istream or port.cat.is_istreams:
            _append_stream_iface(
                ifaces,
                port_name,
                ISTREAM_SUFFIXES,
                valid_port=f"{{port}}{ISTREAM_ROLES['valid']}",
                ready_port=f"{{port}}{ISTREAM_ROLES['ready']}",
            )
        elif port.cat.is_ostream or port.cat.is_ostreams:
            _append_stream_iface(
                ifaces,
                port_name,
                OSTREAM_SUFFIXES,
                valid_port=f"{{port}}{OSTREAM_ROLES['valid']}",
                ready_port=f"{{port}}{OSTREAM_ROLES['ready']}",
            )
        elif port.cat.is_mmap:
            _append_mmap_ifaces(ifaces, scalars, port_name, ir_ports)
        else:
            msg = (
                f"Unsupported port category {port.cat} for port "
                f"{port_name} in task {task.name}"
            )
            raise ValueError(msg)


def _get_top_task_ifaces() -> list[AnyInterface]:
    names = (
        ("ARADDR", "ARREADY", "ARVALID"),
        ("AWADDR", "AWREADY", "AWVALID"),
        ("BREADY", "BRESP", "BVALID"),
        ("RDATA", "RREADY", "RRESP", "RVALID"),
        ("WDATA", "WREADY", "WSTRB", "WVALID"),
    )
    return [
        _make_handshake_iface(
            ports=(
                *(f"s_axi_control_{name}" for name in port_names),
                HANDSHAKE_CLK,
                HANDSHAKE_RST_N,
            ),
            clk_port=HANDSHAKE_CLK,
            rst_port=HANDSHAKE_RST_N,
            valid_port=f"s_axi_control_{port_names[-1]}",
            ready_port=f"s_axi_control_{port_names[-2]}",
        )
        for port_names in names
    ]


def _get_slot_task_ifaces(scalars: list[str]) -> list[AnyInterface]:
    return [
        ApCtrlInterface(
            ports=(
                *tuple(scalars),
                HANDSHAKE_CLK,
                HANDSHAKE_RST_N,
                HANDSHAKE_START,
                HANDSHAKE_DONE,
                HANDSHAKE_READY,
                HANDSHAKE_IDLE,
            ),
            clk_port=HANDSHAKE_CLK,
            rst_port=HANDSHAKE_RST_N,
            ap_start_port=HANDSHAKE_START,
            ap_done_port=HANDSHAKE_DONE,
            ap_ready_port=HANDSHAKE_READY,
            ap_idle_port=HANDSHAKE_IDLE,
            ap_continue_port=None,
            origin_info="",
        ),
        ClockInterface(ports=(HANDSHAKE_CLK,), origin_info=""),
        FeedForwardResetInterface(
            ports=(HANDSHAKE_CLK, HANDSHAKE_RST_N),
            clk_port=HANDSHAKE_CLK,
            origin_info="",
        ),
    ]


def get_upper_task_ir_ifaces_and_scalars(
    project: Project,
    task: Task,
    is_top: bool,
) -> tuple[list[AnyInterface], list[str]]:
    """Get the interface of the upper task IR."""
    ifaces: list[AnyInterface] = []
    scalars: list[str] = []
    task_ir = project.get_module(task.name)
    ir_ports = [port.name for port in task_ir.ports]
    _append_task_port_ifaces(ifaces, scalars, task, ir_ports)
    ifaces.extend(_get_top_task_ifaces() if is_top else _get_slot_task_ifaces(scalars))

    return ifaces, scalars
