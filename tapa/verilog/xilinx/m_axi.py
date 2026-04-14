"""Memory-mapped AXI interface utilities."""

from collections.abc import Iterable

from tapa.protocol import (
    M_AXI_ADDR_PORTS,
    M_AXI_PARAM_PREFIX,
    M_AXI_PARAM_SUFFIXES,
    M_AXI_PORT_WIDTHS,
    M_AXI_PORTS,
    M_AXI_PREFIX,
    M_AXI_SUFFIXES,
    M_AXI_SUFFIXES_BY_CHANNEL,
    M_AXI_SUFFIXES_COMPACT,
)
from tapa.verilog.ast.width import Width

__all__ = [
    "M_AXI_ADDR_PORTS",
    "M_AXI_PARAM_PREFIX",
    "M_AXI_PARAM_SUFFIXES",
    "M_AXI_PORTS",
    "M_AXI_PORT_WIDTHS",
    "M_AXI_PREFIX",
    "M_AXI_SUFFIXES",
    "M_AXI_SUFFIXES_BY_CHANNEL",
    "M_AXI_SUFFIXES_COMPACT",
    "get_m_axi_port_width",
]


def get_m_axi_port_width(
    port: str,
    data_width: int,
    addr_width: int = 64,
    id_width: int | None = None,
    vec_ports: Iterable[str] = ("ID",),
) -> Width | None:
    """Get the width of a memory-mapped AXI port."""
    width = M_AXI_PORT_WIDTHS[port]
    if width == 0:
        width = {"ADDR": addr_width, "DATA": data_width, "STRB": data_width // 8}[port]
    elif width == 1 and port not in vec_ports:
        return None
    if port == "ID" and id_width is not None:
        width = id_width
    return Width.create(width)
