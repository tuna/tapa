"""Utility functions for TAPA Verilog code generation."""

import re
from collections.abc import Iterator

from pyverilog.vparser.ast import Identifier

from tapa.verilog.ast.signal import Reg, Wire
from tapa.verilog.ast.width import Width

__all__ = [
    "Pipeline",
    "array_name",
    "async_mmap_instance_name",
    "match_array_name",
    "sanitize_array_name",
    "wire_name",
]


class Pipeline:
    """Pipeline signals and their widths."""

    def __init__(self, name: str, width: int | None = None) -> None:
        self.name = name
        # If `name` is a constant literal (like `32'd0`), identifiers are just
        # the constant literal.
        self._ids = (Identifier(name if "'d" in name else (f"{name}__q0")),)
        self._width = Width.create(width)

    def __getitem__(self, idx: int) -> Identifier:
        return self._ids[idx]

    def __iter__(self) -> Iterator[Identifier]:
        return iter(self._ids)

    @property
    def signals(self) -> Iterator[Reg | Wire]:
        yield Wire(name=self[0].name, width=self._width)


def match_array_name(name: str) -> tuple[str, int] | None:
    """Deprecated: use tapa.common.base._match_array_name for non-verilog code."""
    match = re.fullmatch(r"(\w+)\[(\d+)\]", name)
    return (match[1], int(match[2])) if match is not None else None


def array_name(name: str, idx: int) -> str:
    """Deprecated: use tapa.common.base._array_name for non-verilog code."""
    return f"{name}[{idx}]"


def sanitize_array_name(name: str) -> str:
    match = match_array_name(name)
    return f"{match[0]}_{match[1]}" if match is not None else name


def wire_name(fifo: str, suffix: str) -> str:
    fifo = sanitize_array_name(fifo)
    if suffix.startswith("_"):
        suffix = suffix[1:]
    return f"{fifo}__{suffix}"


def async_mmap_instance_name(variable_name: str) -> str:
    return f"{variable_name}__m_axi"
