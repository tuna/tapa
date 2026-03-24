"""Device configuration helpers for Xilinx backends."""

from __future__ import annotations

import glob
import os
import zipfile
from typing import TYPE_CHECKING, NoReturn
from xml.etree import ElementTree as ET

if TYPE_CHECKING:
    from collections.abc import Callable

_XILINX_XML_NS = {"xd": "http://www.xilinx.com/xd"}
_XD_NS = "{http://www.xilinx.com/xd}"


def get_device_info(platform_path: str) -> dict[str, str]:
    """Extract device part number and target frequency from SDAccel platform."""
    device_name = os.path.basename(platform_path)
    try:
        platform_file = next(
            glob.iglob(os.path.join(glob.escape(platform_path), "hw", "*.[xd]sa"))
        )
    except StopIteration as e:
        msg = f"cannot find platform file for {device_name}"
        raise ValueError(msg) from e
    with (
        zipfile.ZipFile(platform_file) as platform,
        platform.open(os.path.basename(platform_file)[:-4] + ".hpfm") as metadata,
    ):
        platform_info = ET.parse(metadata).find(
            "./xd:component/xd:platformInfo", _XILINX_XML_NS
        )
        if platform_info is None:
            msg = "cannot parse platform"
            raise ValueError(msg)
        clock_period = platform_info.find(
            "./xd:systemClocks/xd:clock/[@xd:id='0']", _XILINX_XML_NS
        )
        if clock_period is None:
            msg = "cannot find clock period in platform"
            raise ValueError(msg)
        part_num = platform_info.find("xd:deviceInfo", _XILINX_XML_NS)
        if part_num is None:
            msg = "cannot find part number in platform"
            raise ValueError(msg)
        return {
            "clock_period": clock_period.attrib[f"{_XD_NS}period"],
            "part_num": part_num.attrib[f"{_XD_NS}name"],
        }


def parse_device_info(
    platform_and_argname: tuple[str | None, str],
    part_num_and_argname: tuple[str | None, str],
    clock_period_and_argname: tuple[float | str | None, str],
    on_error: Callable[[str], NoReturn],
) -> dict[str, str]:
    platform, platform_argname = platform_and_argname
    part_num, part_num_argname = part_num_and_argname
    clock_period, clock_period_argname = clock_period_and_argname
    original_platform = platform

    if platform is not None:
        platform = os.path.join(
            os.path.dirname(platform),
            os.path.basename(platform).replace(":", "_").replace(".", "_"),
        )
        for platform_dir in (
            os.path.join("/", "opt", "xilinx"),
            os.environ.get("XILINX_VITIS"),
            os.environ.get("XILINX_SDX"),
        ):
            if not os.path.isdir(platform) and platform_dir is not None:
                platform = os.path.join(platform_dir, "platforms", platform)
        if not os.path.isdir(platform):
            on_error(
                f"cannot find the specified platform '{original_platform}'; "
                "are you sure it has been installed, "
                "e.g., in '/opt/xilinx/platforms'?"
            )
    if platform is None or not os.path.isdir(platform):
        if clock_period is None:
            on_error(
                "cannot determine the target clock period; "
                f"please either specify '{platform_argname}' "
                "so the target clock period can be extracted from it, or "
                f"specify '{clock_period_argname}' directly"
            )
        if part_num is None:
            on_error(
                "cannot determine the target part number; "
                f"please either specify '{platform_argname}' "
                "so the target part number can be extracted from it, or "
                f"specify '{part_num_argname}' directly"
            )
        return {
            "clock_period": str(clock_period),
            "part_num": part_num,
        }
    device_info = get_device_info(platform)
    if clock_period is not None:
        device_info["clock_period"] = str(clock_period)
    if part_num is not None:
        device_info["part_num"] = part_num
    return device_info
