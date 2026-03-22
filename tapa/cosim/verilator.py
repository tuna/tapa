"""Verilator-based cosimulation backend for TAPA.

Generates a C++ testbench and builds/runs it with Verilator, providing an
open-source alternative to xsim that works on both Linux and macOS.
"""

from pathlib import Path
from typing import TYPE_CHECKING

from tapa.cosim.common import parse_register_addr
from tapa.cosim.verilator_build import generate_build_script as _generate_build_script
from tapa.cosim.verilator_build import launch_verilator  # noqa: F401
from tapa.cosim.verilator_dpi import generate_dpi_support
from tapa.cosim.verilator_ips import detect_xilinx_ips
from tapa.cosim.verilator_tb_core import generate_cpp_testbench

if TYPE_CHECKING:
    from collections.abc import Sequence

    from tapa.cosim.common import AXI, Arg


def generate_verilator_tb(
    config: dict,
    axi_list: list["AXI"],
    tb_output_dir: str,
) -> None:
    """Generate C++ testbench and support files for Verilator simulation."""
    Path(tb_output_dir).mkdir(parents=True, exist_ok=True)

    top_name: str = config["top_name"]
    args: Sequence[Arg] = config["args"]
    verilog_path: str = config["verilog_path"]
    mode: str = config["mode"]

    # Copy RTL and TCL files to output directory for Verilator
    rtl_dir = Path(tb_output_dir) / "rtl"
    rtl_dir.mkdir(parents=True, exist_ok=True)
    for ext in ("*.v", "*.sv", "*.tcl"):
        for src_file in Path(verilog_path).glob(ext):
            target = rtl_dir / src_file.name
            target.write_bytes(src_file.read_bytes())

    # Detect and replace Xilinx IPs with behavioral models
    detect_xilinx_ips(rtl_dir)

    # Parse control register addresses (Vitis mode)
    reg_addrs: dict[str, list[str]] = {}
    if mode == "vitis":
        ctrl_path = f"{verilog_path}/{top_name}_control_s_axi.v"
        reg_addrs = parse_register_addr(ctrl_path)

    # Generate the C++ testbench
    tb_cpp = generate_cpp_testbench(top_name, axi_list, args, config, reg_addrs, mode)
    (Path(tb_output_dir) / "tb.cpp").write_text(tb_cpp, encoding="utf-8")

    # Generate the DPI-C FP32 support file
    dpi_c = generate_dpi_support()
    (Path(tb_output_dir) / "dpi_support.cpp").write_text(dpi_c, encoding="utf-8")

    # Generate build script
    build_sh = _generate_build_script(top_name)
    build_path = Path(tb_output_dir) / "build.sh"
    build_path.write_text(build_sh, encoding="utf-8")
    build_path.chmod(0o755)
