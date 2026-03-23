"""Stream queue code generation for Verilator TBs."""

from __future__ import annotations

from pathlib import Path
from typing import TYPE_CHECKING

from jinja2 import Environment, FileSystemLoader, StrictUndefined

if TYPE_CHECKING:
    from collections.abc import Sequence

    from tapa.cosim.common import Arg

_env = Environment(
    loader=FileSystemLoader(str(Path(__file__).parent / "assets")),
    undefined=StrictUndefined,
    trim_blocks=True,
    lstrip_blocks=True,
)


def generate_stream_support(args: Sequence[Arg]) -> list[str]:
    stream_args = [arg for arg in args if arg.is_stream]
    if not stream_args:
        return []
    ctx = [
        {
            "qualified_name": arg.qualified_name,
            "width_bytes": (arg.port.data_width + 7) // 8,
        }
        for arg in stream_args
    ]
    rendered = _env.get_template("verilator_tb_stream.j2").render(stream_args=ctx)
    return rendered.split("\n")
