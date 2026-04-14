"""Program-level RTL codegen helpers extracted from Program class.

These free functions perform the same work as the corresponding
Program methods but do not require a Program instance — they accept
the minimal set of data they need.  Callers that currently use
``program.generate_task_rtl()`` etc. can continue to do so; the
methods on Program delegate here.
"""

from __future__ import annotations

import json
import logging
from typing import TYPE_CHECKING

import yaml

if TYPE_CHECKING:
    from tapa.codegen.task_rtl import TaskRtlState
    from tapa.core import Program
    from tapa.task import Task

_logger = logging.getLogger().getChild(__name__)


def generate_task_rtl(
    program: Program,
    rtl_states: dict[str, TaskRtlState],
) -> None:
    """Extract HDL files from tarballs generated from HLS."""
    from tapa.program.rtl_codegen import (  # noqa: PLC0415
        extract_task_rtl,
        instrument_upper_task_rtl,
        parse_task_rtl,
    )

    extract_task_rtl(program)
    parse_task_rtl(program, rtl_states)
    instrument_upper_task_rtl(program, rtl_states)


def generate_top_rtl(
    program: Program,
    rtl_states: dict[str, TaskRtlState],
    override_report_schema_version: str,
) -> None:
    """Instrument HDL files generated from HLS.

    Args:
        program: The TAPA Program instance.
        rtl_states: Map of task names to their RTL state holders.
        override_report_schema_version: Override the schema version with the
            given string, if non-empty.
    """
    import os  # noqa: PLC0415

    from tapa.program_codegen.program import (  # noqa: PLC0415
        instrument_upper_and_template_task as _instrument,
    )

    if program.top_task.name in program.gen_templates:
        msg = "top task cannot be a template"
        raise ValueError(msg)

    # instrument the top-level RTL if it is a upper-level task
    if program.top_task.is_upper:
        _instrument(program, program.top_task, rtl_states)

    _logger.info("generating report")
    task_report = program.top_task.report
    if override_report_schema_version:
        task_report["schema"] = override_report_schema_version
    with open(program.report_paths.yaml, "w", encoding="utf-8") as fp:
        yaml.dump(task_report, fp, default_flow_style=False, sort_keys=False)
    with open(program.report_paths.json, "w", encoding="utf-8") as fp:
        json.dump(task_report, fp, indent=2)

    # self.files won't be populated until all tasks are instrumented
    _logger.info("writing generated auxiliary RTL files")
    for name, content in program.files.items():
        with open(os.path.join(program.rtl_dir, name), "w", encoding="utf-8") as fp:
            fp.write(content)


def instrument_upper_and_template_task(
    program: Program,
    task: Task,
    rtl_states: dict[str, TaskRtlState],
) -> None:
    """Instrument a single upper-level or template task."""
    from tapa.program_codegen.program import (  # noqa: PLC0415
        instrument_upper_and_template_task as _impl,
    )

    _impl(program, task, rtl_states)
