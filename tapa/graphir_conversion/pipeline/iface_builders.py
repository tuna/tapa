"""Interface-construction helpers for GraphIR conversion."""

from __future__ import annotations

from typing import TYPE_CHECKING

from tapa.graphir.types import (
    AnyInterface,
    ApCtrlInterface,
    ClockInterface,
    FalsePathInterface,
    FeedForwardInterface,
    FeedForwardResetInterface,
    HandShakeInterface,
    Project,
)
from tapa.protocol import (
    HANDSHAKE_CLK,
    HANDSHAKE_DONE,
    HANDSHAKE_IDLE,
    HANDSHAKE_READY,
    HANDSHAKE_RST_N,
    HANDSHAKE_START,
)

if TYPE_CHECKING:
    from collections.abc import Collection

    from tapa.task import Task

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


def make_handshake_iface(
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


def get_fifo_ifaces() -> list[AnyInterface]:
    return [
        ClockInterface(ports=("clk",), origin_info=""),
        FeedForwardResetInterface(
            ports=("clk", "reset"),
            clk_port="clk",
            origin_info="",
        ),
        make_handshake_iface(
            ports=("if_din", "if_full_n", "if_write", "clk", "reset"),
            clk_port="clk",
            rst_port="reset",
            valid_port="if_write",
            ready_port="if_full_n",
        ),
        make_handshake_iface(
            ports=("if_dout", "if_empty_n", "if_read", "clk", "reset"),
            clk_port="clk",
            rst_port="reset",
            valid_port="if_empty_n",
            ready_port="if_read",
        ),
        FalsePathInterface(ports=("if_read_ce",), origin_info=""),
        FalsePathInterface(ports=("if_write_ce",), origin_info=""),
    ]


def get_fsm_ifaces(
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
            *(
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


def get_ctrl_s_axi_ifaces(project: Project) -> tuple[str, list[AnyInterface]]:
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
        make_handshake_iface(
            ports=("ARADDR", "ARREADY", "ARVALID"),
            clk_port="ACLK",
            rst_port="ARESET",
            valid_port="ARVALID",
            ready_port="ARREADY",
        ),
        make_handshake_iface(
            ports=("AWADDR", "AWREADY", "AWVALID"),
            clk_port="ACLK",
            rst_port="ARESET",
            valid_port="AWVALID",
            ready_port="AWREADY",
        ),
        make_handshake_iface(
            ports=("BREADY", "BRESP", "BVALID"),
            clk_port="ACLK",
            rst_port="ARESET",
            valid_port="BVALID",
            ready_port="BREADY",
        ),
        make_handshake_iface(
            ports=("RDATA", "RREADY", "RRESP", "RVALID"),
            clk_port="ACLK",
            rst_port="ARESET",
            valid_port="RVALID",
            ready_port="RREADY",
        ),
        make_handshake_iface(
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


def get_reset_inverter_ifaces() -> list[AnyInterface]:
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


def get_top_task_ifaces() -> list[AnyInterface]:
    names = (
        (("ARADDR", "ARREADY", "ARVALID"), "ARVALID", "ARREADY"),
        (("AWADDR", "AWREADY", "AWVALID"), "AWVALID", "AWREADY"),
        (("BREADY", "BRESP", "BVALID"), "BVALID", "BREADY"),
        (("RDATA", "RREADY", "RRESP", "RVALID"), "RVALID", "RREADY"),
        (("WDATA", "WREADY", "WSTRB", "WVALID"), "WVALID", "WREADY"),
    )
    return [
        make_handshake_iface(
            ports=(
                *(f"s_axi_control_{name}" for name in channel_ports),
                HANDSHAKE_CLK,
                HANDSHAKE_RST_N,
            ),
            clk_port=HANDSHAKE_CLK,
            rst_port=HANDSHAKE_RST_N,
            valid_port=f"s_axi_control_{valid_port}",
            ready_port=f"s_axi_control_{ready_port}",
        )
        for channel_ports, valid_port, ready_port in names
    ]


def get_slot_task_ifaces(scalars: list[str]) -> list[AnyInterface]:
    return [
        ApCtrlInterface(
            ports=(
                *scalars,
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
