"""C++ testbench generation for Verilator cosimulation."""

from __future__ import annotations

from pathlib import Path
from typing import TYPE_CHECKING

from jinja2 import Environment, FileSystemLoader, StrictUndefined

from tapa.cosim.common import output_data_path as _output_data_path
from tapa.cosim.config_preprocess import CosimConfig

if TYPE_CHECKING:
    from collections.abc import Sequence

    from tapa.cosim.common import AXI, Arg

_env = Environment(
    loader=FileSystemLoader(str(Path(__file__).parent / "assets")),
    undefined=StrictUndefined,
    trim_blocks=True,
    lstrip_blocks=True,
)

_WIDE_RDATA_WIDTH = 64


def _build_axi_ctx(
    axi_list: list[AXI],
    axi_to_data: dict,
    axi_to_size: dict,
) -> list[dict]:
    ctx = []
    for idx, axi in enumerate(axi_list):
        data_path = axi_to_data.get(axi.name, "")
        c_size = axi_to_size.get(axi.name, "0")
        ctx.append(
            {
                "name": axi.name,
                "data_width": axi.data_width,
                "data_bytes": axi.data_width // 8,
                "base_addr": f"0x{idx + 1}0000000ULL",
                "byte_size": f"{c_size} * {axi.data_width // 8}",
                "data_path": data_path,
                "out_data_path": _output_data_path(data_path) if data_path else "",
            }
        )
    return ctx


def _build_stream_args_ctx(args: Sequence[Arg], axis_to_data: dict) -> list[dict]:
    ctx = []
    for arg in args:
        if not arg.is_stream:
            continue
        data_path = axis_to_data.get(arg.qualified_name, "")
        ctx.append(
            {
                "qualified_name": arg.qualified_name,
                "width_bytes": (arg.port.data_width + 7) // 8,
                "is_istream": arg.port.is_istream,
                "is_ostream": arg.port.is_ostream,
                "data_path": data_path,
                "out_data_path": _output_data_path(data_path) if data_path else "",
            }
        )
    return ctx


def _build_vitis_ctrl_writes(
    args: Sequence[Arg],
    reg_addrs: dict[str, list[str]],
    scalar_to_val: dict,
) -> list[dict]:
    writes = []
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
                writes.append({"addr": a, "val": val})
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
                writes.append({"addr": a, "val": v})
    return writes


def _build_hls_port_args(
    args: Sequence[Arg],
    axi_list: list[AXI],
    scalar_to_val: dict,
) -> tuple[list[dict], list[dict]]:
    mmap_args = []
    for arg in args:
        if arg.is_mmap:
            for idx, axi in enumerate(axi_list):
                if axi.name == arg.name:
                    mmap_args.append(
                        {"name": arg.name, "base_addr": f"0x{idx + 1}0000000ULL"}
                    )
                    break
    scalar_args = [
        {
            "name": arg.name,
            "hex_val": scalar_to_val.get(arg.qualified_name, "'h0").replace("'h", "0x"),
        }
        for arg in args
        if arg.is_scalar
    ]
    return mmap_args, scalar_args


def generate_cpp_testbench(  # noqa: PLR0913, PLR0917
    top_name: str,
    axi_list: list[AXI],
    args: Sequence[Arg],
    config: CosimConfig | dict,
    reg_addrs: dict[str, list[str]],
    mode: str,
) -> str:
    if not isinstance(config, CosimConfig):
        config = CosimConfig.model_validate(config)
    scalar_to_val = config.scalar_to_val
    axi_ctx = _build_axi_ctx(
        axi_list,
        config.axi_to_data_file,
        config.axi_to_c_array_size,
    )
    stream_args_ctx = _build_stream_args_ctx(args, config.axis_to_data_file)
    ctrl_writes = (
        _build_vitis_ctrl_writes(args, reg_addrs, scalar_to_val)
        if mode == "vitis"
        else []
    )
    mmap_args, scalar_args = (
        _build_hls_port_args(args, axi_list, scalar_to_val)
        if mode != "vitis"
        else ([], [])
    )
    return _env.get_template("verilator_tb_core.j2").render(
        top_name=top_name,
        axi_list=axi_ctx,
        stream_args=stream_args_ctx,
        mode=mode,
        wide_rdata_width=_WIDE_RDATA_WIDTH,
        ctrl_writes=ctrl_writes,
        mmap_args=mmap_args,
        scalar_args=scalar_args,
    )
