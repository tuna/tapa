"""Kernel metadata helpers for Xilinx backends."""

from __future__ import annotations

import enum
import xml.sax.saxutils
from typing import IO, TYPE_CHECKING, NamedTuple

if TYPE_CHECKING:
    from collections.abc import Iterable


class Cat(enum.Enum):
    SCALAR = 0
    MMAP = 1
    ISTREAM = 2
    OSTREAM = 3


class Arg(NamedTuple):
    cat: Cat
    name: str
    port: str
    ctype: str
    width: int


S_AXI_NAME = "s_axi_control"
M_AXI_PREFIX = "m_axi_"

KERNEL_XML_TEMPLATE = """
<?xml version="1.0" encoding="UTF-8"?>
<root versionMajor="1" versionMinor="6">
  <kernel name="{name}" \
          language="ip_c" \
          vlnv="tapa:xrtl:{name}:1.0" \
          attributes="" \
          preferredWorkGroupSizeMultiple="0" \
          workGroupSize="1" \
          interrupt="true" \
          hwControlProtocol="{hw_ctrl_protocol}">
    <ports>{ports}
    </ports>
    <args>{args}
    </args>
  </kernel>
</root>
"""

S_AXI_PORT = f"""
      <port name="{S_AXI_NAME}" \
            mode="slave" \
            range="0x1000" \
            dataWidth="32" \
            portType="addressable" \
            base="0x0"/>"""

M_AXI_PORT_TEMPLATE = f"""
      <port name="{M_AXI_PREFIX}{{name}}" \
            mode="master" \
            range="0xFFFFFFFFFFFFFFFF" \
            dataWidth="{{width}}" \
            portType="addressable" \
            base="0x0"/>"""

AXIS_PORT_TEMPLATE = """
      <port name="{name}" \
            mode="{mode}" \
            dataWidth="{width}" \
            portType="stream"/>"""

ARG_TEMPLATE = """
      <arg name="{name}" \
           addressQualifier="{addr_qualifier}" \
           id="{arg_id}" \
           port="{port_name}" \
           size="{size:#x}" \
           offset="{offset:#x}" \
           hostOffset="0x0" \
           hostSize="{host_size:#x}" \
           type="{c_type}"/>"""


def print_kernel_xml(name: str, args: Iterable[Arg], kernel_xml: IO[str]) -> None:
    """Generate kernel.xml file."""
    kernel_ports = ""
    kernel_args = ""
    offset = 0x10
    has_s_axi_control = False
    for arg_id, arg in enumerate(args):
        is_stream = False
        if arg.cat == Cat.SCALAR:
            has_s_axi_control = True
            addr_qualifier = 0
            host_size = arg.width // 8
            size = max(4, host_size)
            port_name = arg.port or S_AXI_NAME
        elif arg.cat == Cat.MMAP:
            has_s_axi_control = True
            addr_qualifier = 1
            size = host_size = 8
            port_name = M_AXI_PREFIX + (arg.port or arg.name)
            kernel_ports += M_AXI_PORT_TEMPLATE.format(
                name=arg.port or arg.name, width=arg.width
            )
        elif arg.cat in {Cat.ISTREAM, Cat.OSTREAM}:
            is_stream = True
            addr_qualifier = 4
            size = host_size = 8
            port_name = arg.port or arg.name
            mode = "read_only" if arg.cat == Cat.ISTREAM else "write_only"
            kernel_ports += AXIS_PORT_TEMPLATE.format(
                name=arg.name, mode=mode, width=arg.width
            )
        else:
            msg = f"unknown arg category: {arg.cat}"
            raise NotImplementedError(msg)
        kernel_args += ARG_TEMPLATE.format(
            name=arg.name,
            addr_qualifier=addr_qualifier,
            arg_id=arg_id,
            port_name=port_name,
            c_type=xml.sax.saxutils.escape(arg.ctype),
            size=size,
            offset=0 if is_stream else offset,
            host_size=host_size,
        )
        if not is_stream:
            offset += size + 4
    hw_ctrl_protocol = "ap_ctrl_none"
    if has_s_axi_control:
        hw_ctrl_protocol = "ap_ctrl_hs"
        kernel_ports += S_AXI_PORT
    kernel_xml.write(
        KERNEL_XML_TEMPLATE.format(
            name=name,
            ports=kernel_ports,
            args=kernel_args,
            hw_ctrl_protocol=hw_ctrl_protocol,
        )
    )
