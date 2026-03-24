"""Variables for Xilinx tools to be configured by the developers."""

XILINX_TOOL_PATH = "/opt/tools/xilinx"
XILINX_TOOL_VERSION = "2024.2"
XILINX_TOOL_LEGACY_PATH = "/opt/tools/xilinx"
XILINX_TOOL_LEGACY_VERSION = "2022.2"
HAS_XRT = True
XILINX_XRT_SETUP = "/opt/tapa/software/xilinx/xrt/setup.sh"
XILINX_PLATFORM_REPO_PATHS = "/opt/tapa/software/xilinx/platforms"

# Remote SSH host for fetching vendor headers. Leave REMOTE_HOST empty to disable.
REMOTE_HOST = ""
REMOTE_USER = ""
REMOTE_PORT = "22"
REMOTE_KEY_FILE = ""
REMOTE_XILINX_TOOL_PATH = ""
REMOTE_XILINX_SETTINGS = ""
REMOTE_SSH_CONTROL_DIR = ""
REMOTE_SSH_CONTROL_PERSIST = "30m"
