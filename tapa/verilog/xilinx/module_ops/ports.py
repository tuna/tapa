"""Port lookup helpers for :mod:`tapa.verilog.xilinx.module`."""

from __future__ import annotations

import logging
import re
from typing import TYPE_CHECKING

from tapa.verilog.ast_utils import make_port_arg
from tapa.verilog.util import (
    array_name,
    match_array_name,
    sanitize_array_name,
    wire_name,
)
from tapa.verilog.xilinx.const import (
    ISTREAM_SUFFIXES,
    OSTREAM_SUFFIXES,
    STREAM_PORT_DIRECTION,
)

if TYPE_CHECKING:
    from collections.abc import Iterable, Iterator

    from pyverilog.vparser.ast import PortArg

    from tapa.verilog.ast.ioport import IOPort
    from tapa.verilog.xilinx.module import Module

_logger = logging.getLogger().getChild(__name__)
_FIFO_INFIXES = ("_V", "_r", "_s", "")


def get_port_of(
    module: Module,
    fifo: str,
    suffix: str,
) -> IOPort:
    """Return the IOPort of the given fifo with the given suffix."""
    ports = module.ports
    sanitized_fifo = sanitize_array_name(fifo)
    for infix in _FIFO_INFIXES:
        port = ports.get(f"{sanitized_fifo}{infix}{suffix}")
        if port is not None:
            return port
    match = match_array_name(fifo)
    if match is not None and match[1] == 0:
        singleton_fifo = match[0]
        for infix in _FIFO_INFIXES:
            port_name = f"{singleton_fifo}{infix}{suffix}"
            port = ports.get(port_name)
            if port is not None:
                _logger.debug("assuming %s is a singleton array", port_name)
                return port

    msg = f"module {module.name} does not have port {fifo}.{suffix}"
    raise module.NoMatchingPortError(msg)


def generate_istream_ports(
    module: Module,
    port: str,
    arg: str,
    ignore_peek_fifos: Iterable[str] = (),
) -> Iterator[PortArg]:
    for suffix in ISTREAM_SUFFIXES:
        arg_name = wire_name(arg, suffix)
        yield make_port_arg(port=get_port_of(module, port, suffix).name, arg=arg_name)
        if STREAM_PORT_DIRECTION[suffix] == "input":
            if port in ignore_peek_fifos:
                continue
            match = match_array_name(port)
            peek_port = (
                f"{port}_peek"
                if match is None
                else array_name(f"{match[0]}_peek", match[1])
            )
            yield make_port_arg(
                port=get_port_of(module, peek_port, suffix).name,
                arg=arg_name,
            )


def generate_ostream_ports(module: Module, port: str, arg: str) -> Iterator[PortArg]:
    for suffix in OSTREAM_SUFFIXES:
        yield make_port_arg(
            port=get_port_of(module, port, suffix).name,
            arg=wire_name(arg, suffix),
        )


def find_port(module: Module, prefix: str, suffix: str) -> str | None:
    """Find an IO port with given prefix and suffix in this module."""
    for port_name in module.ports:
        if port_name.startswith(prefix) and port_name.endswith(suffix):
            return port_name
    return None


def get_streams_fifos(module: Module, streams_name: str) -> list[str]:
    """Get all FIFOs that are related to a streams."""
    pattern = re.compile(rf"{streams_name}_(\d+)_")
    fifos = set()

    for s in module.ports:
        match = pattern.match(s)
        if match:
            number = match.group(1)
            fifos.add(f"{streams_name}_{number}")

    if not fifos:
        for s in module.ports:
            for infix in _FIFO_INFIXES:
                if s.startswith(f"{streams_name}{infix}"):
                    return [streams_name]

    if not fifos:
        msg = f"no fifo found for {streams_name}"
        raise ValueError(msg)
    return list(fifos)
