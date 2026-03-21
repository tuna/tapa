"""Regression tests for Xilinx backend module split."""

from __future__ import annotations

import io
from unittest import mock

from tapa.backend import xilinx
from tapa.backend.kernel_metadata import M_AXI_PREFIX, Arg, Cat, print_kernel_xml
from tapa.backend.xilinx_tools import get_cmd_args


def test_xilinx_facade_exports_expected_symbols() -> None:
    assert xilinx.M_AXI_PREFIX == M_AXI_PREFIX
    assert xilinx.RunHls.__name__ == "RunHls"
    assert xilinx.RunAie.__name__ == "RunAie"
    assert xilinx.PackageXo.__name__ == "PackageXo"
    assert callable(xilinx.parse_device_info)


def test_print_kernel_xml_emits_m_axi_and_control_ports() -> None:
    args = [
        Arg(cat=Cat.MMAP, name="mem", port="", ctype="int*", width=512),
        Arg(cat=Cat.SCALAR, name="n", port="", ctype="int", width=32),
    ]
    out = io.StringIO()
    print_kernel_xml("top", args, out)
    payload = out.getvalue()

    assert 'name="m_axi_mem"' in payload
    assert 'hwControlProtocol="ap_ctrl_hs"' in payload
    assert 'name="s_axi_control"' in payload


def test_parse_device_info_uses_explicit_values_without_platform() -> None:
    parsed = xilinx.parse_device_info(
        platform_and_argname=(None, "--platform"),
        part_num_and_argname=("xcu250", "--part-num"),
        clock_period_and_argname=(3.33, "--clock-period"),
        on_error=lambda msg: (_ for _ in ()).throw(RuntimeError(msg)),
    )
    assert parsed["part_num"] == "xcu250"
    assert parsed["clock_period"] == "3.33"


def test_get_cmd_args_passthrough_without_toolchain_env() -> None:
    kwargs: dict[str, str | int | bool] = {}
    with (
        mock.patch("tapa.backend.xilinx_tools.get_remote_config", return_value=None),
        mock.patch.dict("os.environ", {}, clear=True),
    ):
        cmd = get_cmd_args(["vivado", "-version"], ["XILINX_VIVADO"], kwargs)
    assert cmd == ["vivado", "-version"]
