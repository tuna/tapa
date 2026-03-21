"""AXI and RS pragma helpers for :mod:`tapa.verilog.xilinx.module`."""

from __future__ import annotations

from typing import TYPE_CHECKING

from tapa.backend.xilinx import M_AXI_PREFIX
from tapa.verilog.ast.ioport import IOPort
from tapa.verilog.ast.pragma import Pragma
from tapa.verilog.ast_utils import make_port_arg
from tapa.verilog.xilinx.axis import AXIS_PORTS
from tapa.verilog.xilinx.const import HANDSHAKE_CLK, HANDSHAKE_RST_N
from tapa.verilog.xilinx.m_axi import M_AXI_PORTS, M_AXI_SUFFIXES, get_m_axi_port_width

if TYPE_CHECKING:
    from collections.abc import Iterator

    from pyverilog.vparser.ast import PortArg

    from tapa.verilog.xilinx.module import Module


def _get_rs_port(port: str) -> str:
    if port in {"READY", "VALID"}:
        return port.lower()
    return "data"


def _get_rs_pragma(port_name: str) -> Pragma | None:
    if port_name == HANDSHAKE_CLK:
        return Pragma("RS_CLK")

    if port_name == HANDSHAKE_RST_N:
        return Pragma("RS_RST", "ff")

    if port_name == "interrupt":
        return Pragma("RS_FF", port_name)

    for channel, ports in M_AXI_PORTS.items():
        for port, _ in ports:
            if port_name.endswith(f"_{channel}{port}"):
                return Pragma(
                    "RS_HS",
                    f"{port_name[: -len(port)]}.{_get_rs_port(port)}",
                )

    for suffix, role in AXIS_PORTS.items():
        if port_name.endswith(suffix):
            return Pragma("RS_HS", f"{port_name[: -len(suffix)]}.{role}")

    return None


def build_m_axi_io_ports(
    _module: Module,
    name: str,
    data_width: int,
    addr_width: int = 64,
    id_width: int | None = None,
) -> list[IOPort]:
    io_ports = []
    for channel, ports in M_AXI_PORTS.items():
        for port, direction in ports:
            port_name = f"{M_AXI_PREFIX}{name}_{channel}{port}"
            io_ports.append(
                IOPort(
                    direction,
                    port_name,
                    get_m_axi_port_width(port, data_width, addr_width, id_width),
                    _get_rs_pragma(port_name),
                )
            )
    return io_ports


def add_m_axi(
    module: Module,
    name: str,
    data_width: int,
    addr_width: int = 64,
    id_width: int | None = None,
) -> Module:
    return module.add_ports(
        build_m_axi_io_ports(
            module,
            name,
            data_width,
            addr_width=addr_width,
            id_width=id_width,
        ),
    )


def generate_m_axi_ports(
    module: Module,
    port: str,
    arg: str,
    arg_reg: str = "",
) -> Iterator[PortArg]:
    """Generate AXI mmap ports that instantiate given module."""
    for suffix in M_AXI_SUFFIXES:
        yield make_port_arg(
            port=M_AXI_PREFIX + port + suffix,
            arg=M_AXI_PREFIX + arg + suffix,
        )
    for suffix in "_offset", "_data_V", "_V", "":
        if (port_name := port + suffix) in module.ports:
            yield make_port_arg(port=port_name, arg=arg_reg or arg)
            break
    else:
        msg = f"cannot find offset port for {port}"
        raise ValueError(msg)
