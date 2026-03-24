"""Supported target flows of TAPA compiler."""

from enum import Enum, unique


@unique
class Target(Enum):
    """Supported target flows of TAPA compiler."""

    XILINX_AIE = "xilinx-aie"
    """Xilinx AIE target flow, which uses Xilinx AIE compiler to create .a files."""

    XILINX_HLS = "xilinx-hls"
    """Xilinx HLS target flow, which uses Xilinx Vitis HLS to generate RTL zips."""

    XILINX_VITIS = "xilinx-vitis"
    """Xilinx Vitis target flow, which uses the Vitis shell and generates XO files."""
