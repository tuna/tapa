"""Tests for full GraphIR project assembly."""

import importlib
import json
import shutil
from pathlib import Path
from typing import TYPE_CHECKING

import pytest

if TYPE_CHECKING:
    from tapa.core import Program

_TEST_FILES_DIR = Path(__file__).parent.absolute() / "top_conversion_test_files"
_WORK_DIR = Path(__file__).parent.absolute() / "work_dir_project"
_SLOT_TO_REGION = {
    "SLOT_X0Y2_SLOT_X0Y2": "SLOT_X0Y0:SLOT_X0Y0",
    "SLOT_X2Y3_SLOT_X2Y3": "SLOT_X0Y1:SLOT_X0Y1",
    "SLOT_X3Y3_SLOT_X3Y3": "SLOT_X0Y1:SLOT_X0Y1",
}
_SLOT_X0Y0_ADD = (
    "-add { SLICE_X206Y0:SLICE_X232Y59 SLICE_X176Y60:SLICE_X196Y239 "
    "SLICE_X117Y180:SLICE_X145Y239  DSP48E2_X25Y18:DSP48E2_X28Y89 "
    "DSP48E2_X16Y66:DSP48E2_X19Y89 DSP48E2_X30Y0:DSP48E2_X31Y17  "
    "LAGUNA_X24Y0:LAGUNA_X27Y119 LAGUNA_X16Y0:LAGUNA_X19Y119  "
    "RAMB18_X11Y24:RAMB18_X11Y95 RAMB18_X8Y72:RAMB18_X9Y95 "
    "RAMB18_X12Y0:RAMB18_X13Y23  RAMB36_X11Y12:RAMB36_X11Y47 "
    "RAMB36_X8Y36:RAMB36_X9Y47 RAMB36_X12Y0:RAMB36_X13Y11  "
    "URAM288_X4Y16:URAM288_X4Y63 URAM288_X2Y48:URAM288_X2Y63  "
    "CLOCKREGION_X5Y3:CLOCKREGION_X5Y3 CLOCKREGION_X0Y3:CLOCKREGION_X3Y3 "
    "CLOCKREGION_X0Y1:CLOCKREGION_X5Y2 CLOCKREGION_X0Y0:CLOCKREGION_X6Y0 }"
)
_SLOT_X0Y0_REMOVE = (
    "-remove { CLOCKREGION_X2Y0:CLOCKREGION_X3Y3  CLOCKREGION_X4Y0:CLOCKREGION_X7Y3 }"
)
_SLOT_X0Y1_ADD = (
    "-add { SLICE_X176Y240:SLICE_X196Y479  DSP48E2_X25Y90:DSP48E2_X28Y185  "
    "LAGUNA_X24Y120:LAGUNA_X27Y359  RAMB18_X11Y96:RAMB18_X11Y191  "
    "RAMB36_X11Y48:RAMB36_X11Y95  URAM288_X4Y64:URAM288_X4Y127  "
    "CLOCKREGION_X0Y4:CLOCKREGION_X5Y7 }"
)
_SLOT_X0Y1_REMOVE = (
    "-remove { CLOCKREGION_X2Y4:CLOCKREGION_X3Y7  CLOCKREGION_X4Y4:CLOCKREGION_X7Y7 }"
)


def _copy_fixture_rtl(work_dir: Path) -> None:
    hdl_dir = work_dir / "hdl"
    hdl_dir.mkdir(parents=True, exist_ok=True)
    for rtl in _TEST_FILES_DIR.glob("*.v"):
        shutil.copy2(rtl, hdl_dir / rtl.name)


def _build_program() -> "Program":
    pytest.importorskip("toposort")
    pytest.importorskip("intervaltree")
    program_cls = importlib.import_module("tapa.core").Program
    instance_cls = importlib.import_module("tapa.instance").Instance
    module_cls = importlib.import_module("tapa.verilog.xilinx.module").Module

    _WORK_DIR.mkdir(parents=True, exist_ok=True)
    _copy_fixture_rtl(_WORK_DIR)
    with open(_TEST_FILES_DIR / "graph.json", encoding="utf-8") as f:
        obj = json.load(f)

    program = program_cls(
        obj,
        work_dir=str(_WORK_DIR),
        target="xilinx-vitis",
        floorplan_slots=list(_SLOT_TO_REGION),
        slot_task_name_to_fp_region=_SLOT_TO_REGION,
    )
    for task in program._tasks.values():  # noqa: SLF001
        task.instances = tuple(
            instance_cls(program.get_task(name), instance_id=idx, **instance_obj)
            for name, objs in task.tasks.items()
            for idx, instance_obj in enumerate(objs)
        )

    for task in program._tasks.values():  # noqa: SLF001
        task.module = module_cls(
            name=task.name,
            files=[Path(program.get_rtl_path(task.name))],
            is_trimming_enabled=task.is_lower,
        )
        if task.is_upper:
            task.fsm_module = module_cls(
                name=f"{task.name}_fsm",
                files=[Path(program.get_rtl_path(f"{task.name}_fsm"))],
                is_trimming_enabled=True,
            )
    return program


def test_project_assembly_populates_modules_ifaces_and_pblocks() -> None:
    program = _build_program()
    get_project_from_floorplanned_program = importlib.import_module(
        "tapa.graphir_conversion.gen_rs_graphir"
    ).get_project_from_floorplanned_program

    project = get_project_from_floorplanned_program(
        program=program,
        device_config=Path("tests/functional/graphir/u55c_device_config.json"),
        floorplan_path=Path("tests/functional/graphir/floorplan/vadd.json"),
    )

    module_names = {module.name for module in project.modules.module_definitions}
    assert "VecAdd" in module_names
    assert "VecAdd_control_s_axi" in module_names
    assert "fifo" in module_names
    assert project.ifaces is not None
    assert project.ifaces.root["VecAdd"]
    assert project.island_to_pblock_range == {
        "SLOT_X0Y0_TO_SLOT_X0Y0": [_SLOT_X0Y0_ADD, _SLOT_X0Y0_REMOVE],
        "SLOT_X0Y1_TO_SLOT_X0Y1": [_SLOT_X0Y1_ADD, _SLOT_X0Y1_REMOVE],
    }
