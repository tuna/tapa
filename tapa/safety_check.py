"""Safety checks for TAPA tasks."""

__copyright__ = """
Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
All rights reserved. The contributor(s) of this file has/have agreed to the
RapidStream Contributor License Agreement.
"""

from tapa.task import Task

DISABLED_MMAP_NAME_LIST = {
    "begin",
    "end",
    "in",
    "input",
    "out",
    "output",
    "reg",
    "wire",
}


def check_mmap_arg_name(task_list: list[Task]) -> None:
    """Mmap arguments cannot be named "in" or "out".

    Otherwise HLS will have inconsistent naming convention in the generated AXI
    interface.
    """
    for task in task_list:
        if task.is_upper:
            for port_name in task.ports:
                if port_name in DISABLED_MMAP_NAME_LIST:
                    msg = (
                        f"Task argument '{port_name}' is a reserved keyword: "
                        f"{DISABLED_MMAP_NAME_LIST}"
                    )
                    raise ValueError(msg)
