"""Tests for GraphIR project-builder helpers."""

from pathlib import Path
from types import SimpleNamespace
from typing import TYPE_CHECKING, Any, cast

from tapa.graphir_conversion.pipeline.project_builder import (
    add_pblock_ranges,
    get_island_to_pblock_range,
)

if TYPE_CHECKING:
    from tapa.graphir.types import Project

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


def test_pblock_range_mapping_matches_vadd_fixture() -> None:
    expected = {
        "SLOT_X0Y0_TO_SLOT_X0Y0": [_SLOT_X0Y0_ADD, _SLOT_X0Y0_REMOVE],
        "SLOT_X0Y1_TO_SLOT_X0Y1": [_SLOT_X0Y1_ADD, _SLOT_X0Y1_REMOVE],
    }
    device_config = Path("tests/functional/graphir/u55c_device_config.json")
    floorplan_path = Path("tests/functional/graphir/floorplan/vadd.json")

    assert get_island_to_pblock_range(device_config, floorplan_path) == expected

    project = cast("Project", SimpleNamespace())
    add_pblock_ranges(device_config, project, floorplan_path)

    assert project.island_to_pblock_range == expected


def test_project_builder_leaf_loop_does_not_mutate_task_module() -> None:
    """Verify project_builder does not reassign task.module.

    Calls get_project_from_floorplanned_program with a minimal
    top/slot/leaf hierarchy and preattached sentinel leaf.module.
    Verifies object identity is preserved after the call.
    """
    from unittest.mock import MagicMock, patch  # noqa: PLC0415

    from tapa.graphir_conversion.pipeline.project_builder import (  # noqa: PLC0415
        get_project_from_floorplanned_program,
    )

    sentinel = MagicMock(name="sentinel_module")
    sentinel.code = "// sentinel"

    fsm_mock = MagicMock(name="fsm")
    fsm_mock.name = "top_fsm"

    leaf = SimpleNamespace(
        name="leaf",
        is_slot=False,
        module=sentinel,
        fsm_module=None,
    )
    slot = SimpleNamespace(
        name="slot",
        is_slot=True,
        module=MagicMock(),
        fsm_module=fsm_mock,
        instances=(SimpleNamespace(task=leaf),),
        rtl_fsm_module=fsm_mock,
    )
    top = SimpleNamespace(
        name="top",
        is_slot=False,
        module=MagicMock(),
        fsm_module=fsm_mock,
        instances=(SimpleNamespace(task=slot),),
        ports={},
        rtl_module=MagicMock(),
        rtl_fsm_module=fsm_mock,
    )
    program = SimpleNamespace(
        top_task=top,
        top="top",
        slot_task_name_to_fp_region={"slot": "SLOT_X0Y0:SLOT_X0Y0"},
        get_rtl_path=lambda name: f"/fake/{name}.v",
        rtl_dir="/fake/rtl",
    )

    parsed = MagicMock(name="parsed_module")
    parsed.code = "// parsed"
    module_cls = MagicMock(return_value=parsed)
    project_mock = MagicMock()

    # Patch _get_ctrl_s_axi_definition and add_pblock_ranges to avoid file I/O
    with (
        patch(
            "tapa.graphir_conversion.pipeline.project_builder"
            "._get_ctrl_s_axi_definition",
            return_value="ctrl",
        ),
        patch(
            "tapa.graphir_conversion.pipeline.project_builder.add_pblock_ranges",
        ),
    ):
        get_project_from_floorplanned_program(
            program=cast("Any", program),
            device_config=Path("/fake/device.json"),
            floorplan_path=Path("/fake/floorplan.json"),
            get_verilog_module_from_leaf_task=MagicMock(return_value="leaf_ir"),
            get_slot_module_definition=MagicMock(return_value="slot_ir"),
            get_top_module_definition=MagicMock(return_value="top_ir"),
            get_ctrl_s_axi_def=MagicMock(return_value="ctrl"),
            get_fsm_def=MagicMock(return_value="fsm"),
            get_fifo_def=MagicMock(return_value="fifo"),
            get_reset_inverter_def=MagicMock(return_value="rst"),
            get_graphir_iface=MagicMock(return_value={}),
            module_cls=cast("Any", module_cls),
            modules_cls=cast("Any", MagicMock(return_value="modules")),
            project_cls=cast("Any", MagicMock(return_value=project_mock)),
        )

    # Leaf module identity must be unchanged — not replaced by module_cls()
    assert leaf.module is sentinel, "project_builder must not reassign task.module"
