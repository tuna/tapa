"""Shared numeric Vitis tool-version handling.

Tool-version compares must be numeric, not lexicographic: a string
compare orders "2024.10" before "2024.2".
"""

# Directory layout switch: Vitis replaced the Vitis_HLS install layout at 2024.2.
_VITIS_LAYOUT_SWITCH = (2024, 2)

def vitis_layout_subdir(version):
    """Return "/Vitis/" for tool versions >= 2024.2, else "/Vitis_HLS/"."""
    parts = version.split(".")
    if len(parts) != 2 or not parts[0].isdigit() or not parts[1].isdigit():
        fail("XILINX_TOOL_VERSION must be a numeric MAJOR.MINOR version, got " + repr(version))
    return "/Vitis/" if (int(parts[0]), int(parts[1])) >= _VITIS_LAYOUT_SWITCH else "/Vitis_HLS/"
