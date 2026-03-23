"""AXI service and control-port code generation for Verilator TBs."""

from __future__ import annotations

from pathlib import Path
from typing import TYPE_CHECKING

from jinja2 import Environment, FileSystemLoader, StrictUndefined

if TYPE_CHECKING:
    from collections.abc import Sequence

    from tapa.cosim.common import AXI, Arg

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


def generate_ctrl_writes(
    args: Sequence[Arg],
    config: dict,
    reg_addrs: dict[str, list[str]],
) -> list[str]:
    lines: list[str] = [
        "    ctrl_write(0x04, 1);  // GIE",
        "    ctrl_write(0x08, 1);  // IER",
        "",
    ]
    for arg in args:
        mmap_reg_name = arg.name
        if arg.is_mmap and mmap_reg_name not in reg_addrs:
            mmap_reg_name = f"{arg.name}_offset"
        if arg.is_mmap and mmap_reg_name in reg_addrs:
            addrs = reg_addrs[mmap_reg_name]
            base = f"{arg.name}_BASE"
            for i, addr in enumerate(addrs[:2]):
                a = addr.replace("'h", "0x")
                val = f"(uint32_t){base}" if i == 0 else f"(uint32_t)({base} >> 32)"
                lines.append(f"    ctrl_write({a}, {val});")

    scalar_to_val = config.get("scalar_to_val", {})
    for arg in args:
        if arg.is_scalar and arg.qualified_name in reg_addrs:
            addrs = reg_addrs[arg.qualified_name]
            val = scalar_to_val.get(arg.qualified_name, "'h0")
            hex_val = val.replace("'h", "0x")
            for i, addr in enumerate(addrs[:2]):
                a = addr.replace("'h", "0x")
                v = (
                    f"(uint32_t){hex_val}"
                    if i == 0
                    else f"(uint32_t)((uint64_t){hex_val} >> 32)"
                )
                lines.append(f"    ctrl_write({a}, {v});")

    lines.extend(["", "    ctrl_write(0x00, 1);"])
    return lines


def generate_hls_port_setup(
    args: Sequence[Arg],
    axi_list: list[AXI],
    config: dict,
) -> list[str]:
    lines: list[str] = []
    scalar_to_val = config.get("scalar_to_val", {})

    for arg in args:
        if arg.is_mmap:
            for idx, axi in enumerate(axi_list):
                if axi.name == arg.name:
                    base = f"0x{idx + 1}0000000ULL"
                    lines.append(f"    dut->{arg.name}_offset = {base};")
                    break

    for arg in args:
        if arg.is_scalar:
            val = scalar_to_val.get(arg.qualified_name, "'h0")
            hex_val = val.replace("'h", "0x")
            lines.append(f"    dut->{arg.name} = {hex_val};")

    lines.extend(
        [
            "",
            "    dut->ap_start = 1;",
            "    service_all_axi();",
            "    tick();",
        ]
    )
    return lines
