"""Custom RTL validation and replacement helpers."""

from __future__ import annotations

import logging
import shutil
from pathlib import Path
from typing import Any

from tapa.verilog.xilinx.module import Module

_logger = logging.getLogger().getChild(__name__)


def check_custom_rtl_format(
    rtl_paths: list[Path],
    templates_info: dict[str, list[str]],
    tasks: dict[str, Any],
) -> None:
    """Check if custom RTL files match expected template port signatures."""
    if rtl_paths:
        _logger.info("checking custom RTL files format")
    for rtl_path in rtl_paths:
        if rtl_path.suffix != ".v":
            _logger.warning(
                "Skip checking custom rtl format for non-verilog file: %s",
                rtl_path,
            )
            continue
        rtl_module = Module([rtl_path])
        if (task := tasks.get(rtl_module.name)) is None:
            continue
        if {str(port) for port in rtl_module.ports.values()} == set(
            templates_info[task.name]
        ):
            continue
        msg = [
            (
                f"Custom RTL file {rtl_path} for task {task.name}"
                " does not match the expected ports."
            ),
            "Task ports:",
            *(f"  {port}" for port in templates_info[task.name]),
            "Custom RTL ports:",
            *(f"  {port}" for port in rtl_module.ports.values()),
        ]
        _logger.warning("\n".join(msg))


def replace_custom_rtl(
    rtl_dir: str,
    custom_rtl: list[Path],
    templates_info: dict[str, list[str]],
    tasks: dict[str, Any],
) -> None:
    """Copy validated custom RTL files into the generated RTL directory."""
    rtl_path = Path(rtl_dir)
    assert rtl_path.exists()

    _logger.info("Adding custom RTL files to the project:")
    for file_path in custom_rtl:
        _logger.info("  %s", file_path)
    check_custom_rtl_format(custom_rtl, templates_info, tasks)

    for file_path in custom_rtl:
        assert file_path.is_file()
        dest_path = rtl_path / file_path.name
        if dest_path.exists():
            _logger.info("Replacing %s with custom RTL.", file_path.name)
        else:
            _logger.info("Adding custom RTL %s.", file_path.name)
        shutil.copy2(file_path, dest_path)
