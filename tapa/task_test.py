"""Unit tests for tapa.task."""

__copyright__ = """
Copyright (c) 2025 RapidStream Design Automation, Inc. and contributors.
All rights reserved. The contributor(s) of this file has/have agreed to the
RapidStream Contributor License Agreement.
"""

from tapa.codegen.task_rtl import TaskRtlState
from tapa.task import Task


def test_task_topology_only_before_rtl_state() -> None:
    """Program construction creates topology-only tasks (AC-2)."""
    task = Task(name="foo", code="", level="lower")
    assert task.module is None
    assert task.fsm_module is None
    assert task.name == "foo"
    assert task.is_lower


def test_lower_task_has_no_fsm_module() -> None:
    """Lower tasks keep fsm_module=None even after TaskRtlState (AC-2)."""
    task = Task(name="leaf", code="", level="lower")
    TaskRtlState(task)
    assert task.module is not None
    assert task.fsm_module is None  # lower tasks don't get FSM


def test_upper_task_gets_fsm_module() -> None:
    """Upper tasks get fsm_module from TaskRtlState (AC-2)."""
    task = Task(name="top", code="", level="upper")
    TaskRtlState(task)
    assert task.module is not None
    assert task.fsm_module is not None


def test_topology_dict_round_trip() -> None:
    """Task.to_topology_dict() produces a schema compatible with Program (AC-5)."""
    task = Task(
        name="my_task",
        code="void my_task() {}",
        level="lower",
        target_type="hls",
        ports=[{"cat": "scalar", "name": "n", "type": "int", "width": 32}],
    )
    d = task.to_topology_dict()
    assert d["name"] == "my_task"
    assert d["level"] == "lower"
    assert d["target"] == "hls"  # key matches Program.__init__ expectation
    assert "target_type" not in d  # old key should NOT be present
    assert isinstance(d["ports"], list)
    assert d["ports"][0]["name"] == "n"

    # Reconstruct a Task from the dict (simulating load_design path)
    task2 = Task(
        name=d["name"],
        code=d["code"],
        level=d["level"],
        target_type=d["target"],
        ports=d["ports"],
        is_slot=d["is_slot"],
    )
    assert task2.name == task.name
    assert task2.target_type == task.target_type
    assert task2.is_lower


def test_add_rs_pragmas_to_fsm_skips_unused_ports() -> None:
    task = Task(
        name="foo",
        code="",
        level="upper",
        ports=[
            {
                "cat": "scalar",
                "name": "n",
                "type": "int64_t",
                "width": 64,
            },
        ],
    )
    rtl_state = TaskRtlState(task)
    task.instances = ()

    rtl_state.add_rs_pragmas_to_fsm()

    assert task.fsm_module is not None
    ap_ctrl_pragma_lines = [
        line
        for line in task.fsm_module.code.splitlines()
        if line.strip().startswith("// pragma RS ap-ctrl ")
    ]
    assert len(ap_ctrl_pragma_lines) == 1
    assert "scalar" not in ap_ctrl_pragma_lines[0]
