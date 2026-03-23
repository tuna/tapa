"""AXI service and control-port code generation for Verilator TBs."""

from __future__ import annotations

from pathlib import Path
from typing import TYPE_CHECKING

from jinja2 import Environment, FileSystemLoader, StrictUndefined

if TYPE_CHECKING:
    from tapa.cosim.common import AXI

_env = Environment(
    loader=FileSystemLoader(str(Path(__file__).parent / "assets")),
    undefined=StrictUndefined,
    trim_blocks=True,
    lstrip_blocks=True,
)


def generate_axi_helpers(axi_list: list[AXI], mode: str) -> list[str]:
    ctx = [{"name": axi.name, "data_bytes": axi.data_width // 8} for axi in axi_list]
    rendered = _env.get_template("verilator_tb_axi.j2").render(axi_list=ctx, mode=mode)
    return rendered.split("\n")
