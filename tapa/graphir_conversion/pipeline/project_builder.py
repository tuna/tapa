"""Project assembly helpers for GraphIR conversion."""

from __future__ import annotations

import json
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from pathlib import Path

    from tapa.graphir.types import Project


def get_island_to_pblock_range(
    device_config: Path,
    floorplan_path: Path,
) -> dict[str, list[str]]:
    """Load the floorplan-aware pblock mapping for a project."""
    with open(device_config, encoding="utf-8") as f:
        device = json.load(f)
    with open(floorplan_path, encoding="utf-8") as f:
        floorplan = json.load(f)

    slots = set(floorplan.values())
    slot_to_pblock_ranges = {}
    for slot in device["slots"]:
        x = slot["x"]
        y = slot["y"]
        slot_name = f"SLOT_X{x}Y{y}:SLOT_X{x}Y{y}"
        if slot_name not in slots:
            continue
        slot_to_pblock_ranges[slot_name.replace(":", "_TO_")] = slot["pblock_ranges"]
    return slot_to_pblock_ranges


def add_pblock_ranges(
    device_config: Path,
    project: Project,
    floorplan_path: Path,
) -> None:
    """Attach pblock ranges to a GraphIR project."""
    project.island_to_pblock_range = get_island_to_pblock_range(
        device_config=device_config,
        floorplan_path=floorplan_path,
    )
