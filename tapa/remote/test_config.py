"""Tests for RemoteConfig Pydantic model and load_remote_config."""

from typing import Any

import pytest
from pydantic import ValidationError

from tapa.remote.config import RemoteConfig, load_remote_config

_DEFAULT_PORT = 22
_CUSTOM_PORT = 2222


def test_load_remote_config_from_cli_string() -> None:
    config = load_remote_config("user@host:2222")
    assert config is not None
    assert config.host == "host"
    assert config.user == "user"
    assert config.port == _CUSTOM_PORT


def test_load_remote_config_invalid_port_raises() -> None:
    with pytest.raises((ValidationError, ValueError)):
        RemoteConfig.model_validate({"host": "h", "user": "u", "port": "not-a-number"})


def test_load_remote_config_none_returns_none() -> None:
    config = load_remote_config(None)
    assert config is None


def test_remote_config_defaults() -> None:
    config = RemoteConfig(host="myhost", user="myuser")
    assert config.port == _DEFAULT_PORT
    assert config.work_dir == "/tmp/tapa-remote"
    assert config.ssh_multiplex is True
    assert config.ssh_control_persist == "30m"
    assert config.key_file is None
    assert config.ssh_control_dir is None


def test_remote_config_ssh_multiplex_string_true() -> None:
    data: dict[str, Any] = {"host": "h", "user": "u", "ssh_multiplex": "true"}
    config = RemoteConfig.model_validate(data)
    assert config.ssh_multiplex is True


def test_remote_config_ssh_multiplex_string_false() -> None:
    data: dict[str, Any] = {"host": "h", "user": "u", "ssh_multiplex": "false"}
    config = RemoteConfig.model_validate(data)
    assert config.ssh_multiplex is False


def test_remote_config_ssh_multiplex_string_yes() -> None:
    data: dict[str, Any] = {"host": "h", "user": "u", "ssh_multiplex": "yes"}
    config = RemoteConfig.model_validate(data)
    assert config.ssh_multiplex is True


def test_remote_config_ssh_multiplex_string_no() -> None:
    data: dict[str, Any] = {"host": "h", "user": "u", "ssh_multiplex": "no"}
    config = RemoteConfig.model_validate(data)
    assert config.ssh_multiplex is False


def test_remote_config_key_file_expanded() -> None:
    config = RemoteConfig(host="h", user="u", key_file="~/.ssh/id_rsa")
    assert config.key_file is not None
    assert not config.key_file.startswith("~")


def test_load_remote_config_host_only() -> None:
    config = load_remote_config("myhost")
    assert config is not None
    assert config.host == "myhost"
    assert config.port == _DEFAULT_PORT
