"""Configuration for remote vendor tool execution."""

__copyright__ = """
Copyright (c) 2025 RapidStream Design Automation, Inc. and contributors.
All rights reserved. The contributor(s) of this file has/have agreed to the
RapidStream Contributor License Agreement.
"""

import getpass
import logging
import os
from dataclasses import dataclass

import yaml

_logger = logging.getLogger().getChild(__name__)

TAPARC_PATH = os.path.expanduser("~/.taparc")


@dataclass
class RemoteConfig:
    """Configuration for remote execution of vendor tools."""

    host: str
    user: str
    port: int = 22
    key_file: str | None = None
    xilinx_settings: str | None = None
    work_dir: str = "/tmp/tapa-remote"


_active_config: RemoteConfig | None = None


def set_remote_config(config: RemoteConfig | None) -> None:
    """Set the active remote configuration."""
    global _active_config  # noqa: PLW0603
    _active_config = config


def get_remote_config() -> RemoteConfig | None:
    """Get the active remote configuration."""
    return _active_config


def _parse_remote_host(remote_host: str) -> dict[str, str | int]:
    """Parse user@host[:port] into a dict."""
    result: dict[str, str | int] = {}
    if "@" in remote_host:
        result["user"], remote_host = remote_host.split("@", 1)
    if ":" in remote_host:
        host, port_str = remote_host.rsplit(":", 1)
        result["host"] = host
        result["port"] = int(port_str)
    else:
        result["host"] = remote_host
    return result


def load_remote_config(cli_remote_host: str | None) -> RemoteConfig | None:
    """Load remote configuration from CLI and/or ~/.taparc.

    CLI --remote-host overrides ~/.taparc values.
    Returns None if remote execution is not configured.
    """
    file_config: dict = {}

    if os.path.isfile(TAPARC_PATH):
        try:
            with open(TAPARC_PATH, encoding="utf-8") as f:
                taparc = yaml.safe_load(f)
            if isinstance(taparc, dict) and "remote" in taparc:
                file_config = taparc["remote"]
                if file_config.get("key_file"):
                    file_config["key_file"] = os.path.expanduser(
                        file_config["key_file"]
                    )
        except (OSError, yaml.YAMLError):
            _logger.warning("Failed to parse %s", TAPARC_PATH, exc_info=True)

    if cli_remote_host is None and not file_config:
        return None

    merged: dict = {**file_config}
    if cli_remote_host is not None:
        merged.update(_parse_remote_host(cli_remote_host))

    if "host" not in merged:
        return None

    if "user" not in merged:
        merged["user"] = getpass.getuser()

    return RemoteConfig(
        host=merged["host"],
        user=merged.get("user", getpass.getuser()),
        port=int(merged.get("port", 22)),
        key_file=merged.get("key_file"),
        xilinx_settings=merged.get("xilinx_settings"),
        work_dir=merged.get("work_dir", "/tmp/tapa-remote"),
    )
