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


def test_program_construction_is_topology_only() -> None:
    """Program creates topology-only tasks with module=None (AC-2).

    Both the upper task and the leaf task must be retained and have
    module/fsm_module=None immediately after construction.
    """
    from tapa.core import Program  # noqa: PLC0415

    graph = {
        "top": "top_task",
        "tasks": {
            "top_task": {
                "code": "",
                "level": "upper",
                "target": "hls",
                "tasks": {"leaf_task": [{"args": {}}]},
                "fifos": {},
                "ports": [],
            },
            "leaf_task": {
                "code": "",
                "level": "lower",
                "target": "hls",
                "tasks": {},
                "fifos": {},
                "ports": [],
            },
        },
    }
    program = Program(graph, target="xilinx-hls", work_dir="/tmp/test")
    # Both upper and lower tasks must be retained
    assert "top_task" in program._tasks  # noqa: SLF001
    assert "leaf_task" in program._tasks  # noqa: SLF001
    for task in program._tasks.values():  # noqa: SLF001
        assert task.module is None, f"task {task.name} should be topology-only"
        assert task.fsm_module is None, f"task {task.name} should be topology-only"


def test_design_json_round_trip_with_slots(tmp_path: object) -> None:
    """store_design + load via design.json round-trips including is_slot (AC-5)."""
    import click  # noqa: PLC0415

    work_dir = str(tmp_path)
    # store_design needs a click context with work-dir
    with click.Context(click.Command("test"), obj={"work-dir": work_dir}):
        _run_design_json_round_trip(work_dir)


def _run_design_json_round_trip(work_dir: str) -> None:
    import json  # noqa: PLC0415
    import os  # noqa: PLC0415

    from tapa.core import Program  # noqa: PLC0415
    from tapa.steps.common import store_design  # noqa: PLC0415

    graph = {
        "top": "top_task",
        "tasks": {
            "top_task": {
                "code": "void top() {}",
                "level": "upper",
                "target": "hls",
                "tasks": {"slot_task": [{"args": {}}]},
                "fifos": {},
                "ports": [],
            },
            "slot_task": {
                "code": "void slot() {}",
                "level": "lower",
                "target": "hls",
                "tasks": {},
                "fifos": {},
                "ports": [],
            },
        },
    }
    program = Program(
        graph,
        target="xilinx-hls",
        work_dir=work_dir,
        floorplan_slots=["slot_task"],
        slot_task_name_to_fp_region={"slot_task": "SLOT_X0Y0:SLOT_X0Y0"},
    )
    assert program._tasks["slot_task"].is_slot  # noqa: SLF001

    # Write design.json
    store_design(program)
    design_path = os.path.join(work_dir, "design.json")
    assert os.path.exists(design_path)

    with open(design_path, encoding="utf-8") as f:
        design = json.load(f)

    # Verify schema
    assert design["top"] == "top_task"
    assert design["target"] == "xilinx-hls"
    assert design["tasks"]["slot_task"]["is_slot"] is True
    assert design["tasks"]["top_task"]["is_slot"] is False
    assert design["slot_task_name_to_fp_region"] == {"slot_task": "SLOT_X0Y0:SLOT_X0Y0"}

    # Reconstruct floorplan_slots from design.json
    floorplan_slots = [
        name for name, t in design["tasks"].items() if t.get("is_slot", False)
    ]
    assert floorplan_slots == ["slot_task"]

    # Reconstruct Program from design.json
    program2 = Program(
        {"tasks": design["tasks"], "top": design["top"]},
        target=design["target"],
        work_dir=work_dir,
        floorplan_slots=floorplan_slots,
        slot_task_name_to_fp_region=design.get("slot_task_name_to_fp_region") or {},
    )
    assert program2._tasks["slot_task"].is_slot  # noqa: SLF001
    assert program2.slot_task_name_to_fp_region == {"slot_task": "SLOT_X0Y0:SLOT_X0Y0"}


def test_store_and_load_tapa_program_bridge(tmp_path: object) -> None:
    """store_tapa_program + load_tapa_program round-trips through design.json (AC-5).

    Store and load happen in SEPARATE Click contexts so no in-memory
    cache (obj["design"], obj["tapa-program"]) leaks between them.
    """
    import os  # noqa: PLC0415

    import click  # noqa: PLC0415

    from tapa.common.target import Target  # noqa: PLC0415
    from tapa.core import Program  # noqa: PLC0415
    from tapa.steps.common import load_tapa_program, store_tapa_program  # noqa: PLC0415

    work_dir = str(tmp_path)
    graph = {
        "top": "top_task",
        "tasks": {
            "top_task": {
                "code": "void top() {}",
                "level": "upper",
                "target": "hls",
                "tasks": {"slot_task": [{"args": {}}]},
                "fifos": {},
                "ports": [],
            },
            "slot_task": {
                "code": "void slot() {}",
                "level": "lower",
                "target": "hls",
                "tasks": {},
                "fifos": {},
                "ports": [],
            },
        },
    }

    # --- Store phase: first Click context ---
    with click.Context(click.Command("store"), obj={"work-dir": work_dir}):
        program = Program(
            graph,
            target="xilinx-hls",
            work_dir=work_dir,
            floorplan_slots=["slot_task"],
            slot_task_name_to_fp_region={"slot_task": "SLOT_X0Y0:SLOT_X0Y0"},
        )
        store_tapa_program(program)

    assert os.path.exists(os.path.join(work_dir, "design.json"))

    # --- Load phase: fresh Click context (no cached state) ---
    with click.Context(click.Command("load"), obj={"work-dir": work_dir}):
        loaded = load_tapa_program()

    assert loaded.top == "top_task"
    assert loaded.target == Target.XILINX_HLS
    assert loaded._tasks["slot_task"].is_slot  # noqa: SLF001
    assert loaded.slot_task_name_to_fp_region == {"slot_task": "SLOT_X0Y0:SLOT_X0Y0"}


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
