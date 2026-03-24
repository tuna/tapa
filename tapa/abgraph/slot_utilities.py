"""Slot related utilities."""

__copyright__ = """
Copyright (c) 2025 RapidStream Design Automation, Inc. and contributors.
All rights reserved. The contributor(s) of this file has/have agreed to the
RapidStream Contributor License Agreement.
"""

import re

from tapa.abgraph.device.common import Coor

SLOT_PATTERN = r"SLOT_X(\d+)Y(\d+)_TO_SLOT_X(\d+)Y(\d+)"


def is_valid_slot(slot_name: str) -> bool:
    """Check if a slot name is valid."""
    return re.fullmatch(SLOT_PATTERN, slot_name) is not None


def get_coor_from_slot_name(slot_name: str) -> Coor:
    """Get the coordinate from a slot name."""
    match = re.fullmatch(SLOT_PATTERN, slot_name)
    if match is None:
        msg = f"Invalid slot name {slot_name}"
        raise ValueError(msg)
    dl_x, dl_y, ur_x, ur_y = (int(x) for x in match.groups())
    return Coor(down_left_x=dl_x, down_left_y=dl_y, up_right_x=ur_x, up_right_y=ur_y)


def is_slot1_inside_slot2(slot1: str, slot2: str) -> bool:
    """Check if slot1 is inside slot2."""
    return get_coor_from_slot_name(slot1).is_inside(get_coor_from_slot_name(slot2))


def are_slots_adjacent(slot1: str, slot2: str) -> bool:
    """Check if two slots are adjacent."""
    return get_coor_from_slot_name(slot1).is_neighbor(get_coor_from_slot_name(slot2))


def are_horizontal_neighbors(slot1: str, slot2: str) -> bool:
    """Check if two slots are horizontal neighbors."""
    c1, c2 = get_coor_from_slot_name(slot1), get_coor_from_slot_name(slot2)
    return c1.is_east_neighbor_of(c2) or c1.is_west_neighbor_of(c2)


def are_vertical_neighbors(slot1: str, slot2: str) -> bool:
    """Check if two slots are vertical neighbors."""
    c1, c2 = get_coor_from_slot_name(slot1), get_coor_from_slot_name(slot2)
    return c1.is_north_neighbor_of(c2) or c1.is_south_neighbor_of(c2)


def convert_to_config_pattern(slot_name: str) -> str:
    """Convert the slot name to the configuration pattern."""
    c = get_coor_from_slot_name(slot_name)
    return f"SLOT_X{c.down_left_x}Y{c.down_left_y}:SLOT_X{c.up_right_x}Y{c.up_right_y}"
