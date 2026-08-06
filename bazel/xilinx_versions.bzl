"""Shared numeric Vitis tool-version handling.

Tool-version compares must be numeric, not lexicographic: a string
compare orders "2024.10" before "2024.2".
"""

# Directory layout switch: Vitis replaced the Vitis_HLS install layout at 2024.2.
_VITIS_LAYOUT_SWITCH = (2024, 2)

def xilinx_tool_dir_candidates(root, tool, version):
    """Candidate install dirs for a Xilinx tool, in probe order.

    Classic layout first (`<root>/<Tool>/<version>`), then the AMD
    unified-installer layout introduced with the 2025 releases
    (`<root>/<version>/<Tool>`, e.g. `/opt/AMDDesignTools/2025.2/Vitis`).
    BUILD-phase Starlark cannot stat the filesystem, so callers must
    probe for existence at run/fetch time (wrapper scripts probe with
    `[ -d ]`; repository rules probe with `test -d`).

    Args:
      root: The configured `XILINX_TOOL_PATH`.
      tool: Tool directory name, e.g. "Vitis", "Vitis_HLS", "Vivado".
      version: Numeric "MAJOR.MINOR" tool version string.

    Returns:
      List of candidate directories, most specific layout first.
    """
    return [
        "{}/{}/{}".format(root, tool, version),
        "{}/{}/{}".format(root, version, tool),
    ]

def vitis_layout_subdir(version):
    """Return the Vitis install-layout subdirectory for the given tool version.

    Args:
      version: Numeric "MAJOR.MINOR" Vitis tool version string, e.g. "2024.2".

    Returns:
      "/Vitis/" for tool versions >= 2024.2, else "/Vitis_HLS/".
    """
    parts = version.split(".")
    if len(parts) != 2 or not parts[0].isdigit() or not parts[1].isdigit():
        fail("XILINX_TOOL_VERSION must be a numeric MAJOR.MINOR version, got " + repr(version))
    return "/Vitis/" if (int(parts[0]), int(parts[1])) >= _VITIS_LAYOUT_SWITCH else "/Vitis_HLS/"
