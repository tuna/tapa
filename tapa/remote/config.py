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
    ssh_control_dir: str | None = None
    ssh_control_persist: str = "30m"
    ssh_multiplex: bool = True


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


def _load_remote_config_from_file() -> dict:
    """Load remote config from ~/.taparc."""
    if not os.path.isfile(TAPARC_PATH):
        return {}

    try:
        with open(TAPARC_PATH, encoding="utf-8") as f:
            taparc = yaml.safe_load(f)
    except (OSError, yaml.YAMLError):
        _logger.warning("Failed to parse %s", TAPARC_PATH, exc_info=True)
        return {}

    if not isinstance(taparc, dict) or "remote" not in taparc:
        return {}

    remote_config = dict(taparc["remote"])
    if remote_config.get("key_file"):
        remote_config["key_file"] = os.path.expanduser(remote_config["key_file"])
    return remote_config


def _parse_bool(value: object, default: bool = True) -> bool:
    """Parse bool-like config values."""
    if isinstance(value, bool):
        return value
    if isinstance(value, str):
        return value.strip().lower() not in {"0", "false", "no", "off"}
    if value is None:
        return default
    return bool(value)


def load_remote_config(cli_remote_host: str | None) -> RemoteConfig | None:
    """Load remote configuration from CLI and/or ~/.taparc.

    CLI --remote-host overrides ~/.taparc values.
    Returns None if remote execution is not configured.
    """
    file_config = _load_remote_config_from_file()

    if cli_remote_host is None and not file_config:
        return None

    merged: dict = {**file_config}
    if cli_remote_host is not None:
        merged.update(_parse_remote_host(cli_remote_host))

    if "host" not in merged:
        return None

    if "user" not in merged:
        merged["user"] = getpass.getuser()

    key_file = merged.get("key_file")
    if isinstance(key_file, str):
        key_file = os.path.expanduser(key_file)

    ssh_control_dir = merged.get("ssh_control_dir")
    if isinstance(ssh_control_dir, str):
        ssh_control_dir = os.path.expanduser(ssh_control_dir)

    return RemoteConfig(
        host=merged["host"],
        user=merged.get("user", getpass.getuser()),
        port=int(merged.get("port", 22)),
        key_file=key_file,
        xilinx_settings=merged.get("xilinx_settings"),
        work_dir=merged.get("work_dir", "/tmp/tapa-remote"),
        ssh_control_dir=ssh_control_dir,
        ssh_control_persist=str(merged.get("ssh_control_persist", "30m")),
        ssh_multiplex=_parse_bool(merged.get("ssh_multiplex"), default=True),
    )
