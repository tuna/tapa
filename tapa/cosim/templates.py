__copyright__ = """
Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
All rights reserved. The contributor(s) of this file has/have agreed to the
RapidStream Contributor License Agreement.
"""

import logging
from collections.abc import Sequence

from tapa.cosim.common import AXI, Arg
from tapa.cosim.render import (
    render_axi_ram_inst,
    render_axi_ram_module,
    render_axis,
    render_fifo,
    render_hls_dut,
    render_hls_test_signals,
    render_m_axi_connections,
    render_s_axi_control,
    render_srl_fifo_template,
    render_stream_typedef,
    render_testbench_begin,
    render_testbench_end,
    render_vitis_dut,
    render_vitis_test_signals,
)

_logger = logging.getLogger().getChild(__name__)


def get_axi_ram_inst(axi_obj: AXI) -> str:
    # FIXME: test if using addr_width = 64 will cause problem in simulation
    return render_axi_ram_inst(axi_obj)


def get_s_axi_control() -> str:
    return render_s_axi_control()


def get_axis(args: Sequence[Arg]) -> str:
    return render_axis(args)


def get_fifo(args: Sequence[Arg]) -> str:
    return render_fifo(args)


def get_stream_typedef(args: Sequence[Arg]) -> str:
    return render_stream_typedef(args)


def get_m_axi_connections(arg_name: str) -> str:
    return render_m_axi_connections(arg_name)


def get_vitis_dut(top_name: str, args: Sequence[Arg]) -> str:
    return render_vitis_dut(top_name, args)


def get_hls_dut(
    top_name: str,
    top_is_leaf_task: bool,
    args: Sequence[Arg],
    scalar_to_val: dict[str, str],
) -> str:
    return render_hls_dut(top_name, top_is_leaf_task, args, scalar_to_val)


def get_vitis_test_signals(
    arg_to_reg_addrs: dict[str, list[str]],
    scalar_arg_to_val: dict[str, str],
    args: Sequence[Arg],
) -> str:
    return render_vitis_test_signals(arg_to_reg_addrs, scalar_arg_to_val, args)


def get_hls_test_signals(args: Sequence[Arg]) -> str:
    return render_hls_test_signals(args)


def get_begin() -> str:
    return render_testbench_begin()


def get_end() -> str:
    return render_testbench_end()


def get_axi_ram_module(axi: AXI, input_data_path: str, c_array_size: int) -> str:
    """Generate the AXI RAM module for cosimulation."""
    return render_axi_ram_module(axi, input_data_path, c_array_size)


def get_srl_fifo_template() -> str:
    return render_srl_fifo_template()
