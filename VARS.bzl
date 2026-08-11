"""Variables for Xilinx tools to be configured by the developers."""

XILINX_TOOL_PATH = "/opt/tools/xilinx"
XILINX_TOOL_VERSION = "2024.2"
XILINX_TOOL_LEGACY_PATH = "/opt/tools/xilinx"
XILINX_TOOL_LEGACY_VERSION = "2022.2"
HAS_XRT = True
XILINX_XRT_SETUP = "/opt/xilinx/xrt/setup.sh"
XILINX_PLATFORM_REPO_PATHS = "/opt/xilinx/platforms"

# Device the test kernels target. XILINX_PART_NUM is what `tapa_xo` synthesizes
# for when a target names neither a part nor a platform; XILINX_HW_EMU_PLATFORM
# is what `vpp_xclbin` links the resulting `.xo` against. The two must name the
# same device -- `v++ --link` rejects a part/platform mismatch -- and the
# platform has to be one the installed Vitis still accepts, which rules out
# anything more than a year older than the toolchain. Override both in
# VARS.local.bzl when the local install ships a different device.
XILINX_PART_NUM = "xcu250-figd2104-2l-e"
XILINX_HW_EMU_PLATFORM = "xilinx_u250_gen3x16_xdma_4_1_202210_1"

# Remote SSH host for fetching vendor headers. Leave REMOTE_HOST empty to disable.
REMOTE_HOST = ""
REMOTE_USER = ""
REMOTE_PORT = "22"
REMOTE_KEY_FILE = ""
REMOTE_XILINX_TOOL_PATH = ""
REMOTE_XILINX_SETTINGS = ""
REMOTE_SSH_CONTROL_DIR = ""
REMOTE_SSH_CONTROL_PERSIST = "30m"
