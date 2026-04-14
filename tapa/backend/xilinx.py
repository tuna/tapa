"""Xilinx backend facade and compatibility exports."""

from tapa.backend.device_config import get_device_info, parse_device_info
from tapa.backend.kernel_metadata import (
    Arg,
    Cat,
    print_kernel_xml,
)
from tapa.backend.xilinx_hls import RunAie, RunHls
from tapa.backend.xilinx_tools import PackageXo, Vivado, VivadoHls
from tapa.protocol import M_AXI_PREFIX

__all__ = [
    "M_AXI_PREFIX",
    "Arg",
    "Cat",
    "PackageXo",
    "RunAie",
    "RunHls",
    "Vivado",
    "VivadoHls",
    "get_device_info",
    "parse_device_info",
    "print_kernel_xml",
]
