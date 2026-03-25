"""Verilator-based cosimulation backend for TAPA.

Generates a C++ testbench and builds/runs it with Verilator, providing an
open-source alternative to xsim that works on both Linux and macOS.
"""

from pathlib import Path
from typing import TYPE_CHECKING

from tapa.cosim.common import parse_register_addr
from tapa.cosim.config_preprocess import CosimConfig
from tapa.cosim.verilator_build import generate_build_script as _generate_build_script
from tapa.cosim.verilator_build import launch_verilator
from tapa.cosim.verilator_dpi import generate_dpi_support
from tapa.cosim.verilator_ips import detect_xilinx_ips
from tapa.cosim.verilator_tb_core import generate_cpp_testbench

if TYPE_CHECKING:
    from tapa.cosim.common import AXI

__all__ = ["launch_verilator"]


def generate_verilator_tb(
    config: CosimConfig | dict,
    axi_list: list["AXI"],
    tb_output_dir: str,
) -> None:
    """Generate C++ testbench and support files for Verilator simulation."""
    if not isinstance(config, CosimConfig):
        config = CosimConfig.model_validate(config)
    Path(tb_output_dir).mkdir(parents=True, exist_ok=True)

    top_name = config.top_name
    args = config.args
    verilog_path = config.verilog_path
    mode = config.mode

    rtl_dir = Path(tb_output_dir) / "rtl"
    rtl_dir.mkdir(parents=True, exist_ok=True)
    for ext in ("*.v", "*.sv", "*.tcl"):
        for src_file in Path(verilog_path).glob(ext):
            target = rtl_dir / src_file.name
            target.write_bytes(src_file.read_bytes())

    detect_xilinx_ips(rtl_dir)

    reg_addrs: dict[str, list[str]] = {}
    if mode == "vitis":
        ctrl_path = f"{verilog_path}/{top_name}_control_s_axi.v"
        reg_addrs = parse_register_addr(ctrl_path)

    tb_cpp = generate_cpp_testbench(
        top_name, axi_list, args, config, reg_addrs=reg_addrs, mode=mode
    )
    (Path(tb_output_dir) / "tb.cpp").write_text(tb_cpp, encoding="utf-8")

    dpi_c = generate_dpi_support()
    (Path(tb_output_dir) / "dpi_support.cpp").write_text(dpi_c, encoding="utf-8")

    build_sh = _generate_build_script(top_name)
    build_path = Path(tb_output_dir) / "build.sh"
    build_path.write_text(build_sh, encoding="utf-8")
    build_path.chmod(0o755)
