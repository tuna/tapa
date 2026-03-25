"""RTL code generation helpers for TAPA programs."""

# ruff: noqa: SLF001

from __future__ import annotations

import logging
import os
import shutil
import tarfile
from pathlib import Path
from typing import TYPE_CHECKING

from tapa.instance import Instance
from tapa.verilog.xilinx.module import Module

if TYPE_CHECKING:
    from tapa.core import Program

_logger = logging.getLogger().getChild(__name__)


def extract_task_rtl(program: Program) -> None:
    """Extract tarballs per task and copy asset .v files to rtl_dir."""
    _logger.info("extracting RTL files")
    for task in program._tasks.values():
        with tarfile.open(program.get_tar_path(task.name), "r") as tarfileobj:
            tarfileobj.extractall(path=program.work_dir)

    assets_dir = os.path.join(os.path.dirname(__file__), "..", "assets", "verilog")
    for file_name in (
        "arbiter.v",
        "async_mmap.v",
        "axi_pipeline.v",
        "axi_crossbar_addr.v",
        "axi_crossbar_rd.v",
        "axi_crossbar_wr.v",
        "axi_crossbar.v",
        "axi_register_rd.v",
        "axi_register_wr.v",
        "detect_burst.v",
        "fifo.v",
        "fifo_bram.v",
        "fifo_fwd.v",
        "fifo_srl.v",
        "generate_last.v",
        "priority_encoder.v",
        "relay_station.v",
        "a_axi_write_broadcastor_1_to_3.v",
        "a_axi_write_broadcastor_1_to_4.v",
    ):
        shutil.copy(os.path.join(assets_dir, file_name), program.rtl_dir)


def parse_task_rtl(program: Program) -> None:
    """Parse RTL files and populate task instances."""
    _logger.info("parsing RTL files and populating tasks")
    for task in program._tasks.values():
        _logger.debug("parsing %s", task.name)
        task.module = Module(
            files=[Path(program.get_rtl_path(task.name))],
            is_trimming_enabled=task.is_lower,
        )
        task.self_area = program.get_area(task.name)
        task.clock_period = program.get_clock_period(task.name)

        _logger.debug("populating %s", task.name)
        task.instances = tuple(
            Instance(program.get_task(name), instance_id=idx, **obj)
            for name, objs in task.tasks.items()
            for idx, obj in enumerate(objs)
        )


def instrument_upper_task_rtl(program: Program) -> None:
    """Instrument upper-level RTL (except top-level)."""
    _logger.info("instrumenting upper-level RTL")
    for task in program._tasks.values():
        if task.is_upper and task.name != program.top:
            program._instrument_upper_and_template_task(task)
        elif not task.is_upper and task.name in program.gen_templates:
            assert task.ports
            program._instrument_upper_and_template_task(task)
