"""Tests for GraphIR project-builder helpers."""

from pathlib import Path
from types import SimpleNamespace
from typing import TYPE_CHECKING, cast

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
