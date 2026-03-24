"""Variables for Xilinx tools to be configured by the developers."""

# Copyright (c) 2025 RapidStream Design Automation, Inc. and contributors.
# All rights reserved. The contributor(s) of this file has/have agreed to the
# RapidStream Contributor License Agreement.

XILINX_TOOL_PATH = "/opt/tools/xilinx"
XILINX_TOOL_VERSION = "2024.2"
XILINX_TOOL_LEGACY_PATH = "/opt/tools/xilinx"
XILINX_TOOL_LEGACY_VERSION = "2022.2"
HAS_XRT = True
XILINX_XRT_SETUP = "/opt/tapa/software/xilinx/xrt/setup.sh"
XILINX_PLATFORM_REPO_PATHS = "/opt/tapa/software/xilinx/platforms"

# Remote SSH host for fetching vendor headers (e.g., on macOS without local
# Xilinx tools). Leave REMOTE_HOST empty to disable remote fetching.
REMOTE_HOST = ""
REMOTE_USER = ""
REMOTE_PORT = "22"
REMOTE_KEY_FILE = ""
REMOTE_XILINX_TOOL_PATH = ""
REMOTE_XILINX_SETTINGS = ""
REMOTE_SSH_CONTROL_DIR = ""
REMOTE_SSH_CONTROL_PERSIST = "30m"
