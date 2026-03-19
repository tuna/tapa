"""OpenSSH helpers for remote execution with multiplexing."""

__copyright__ = """
Copyright (c) 2025 RapidStream Design Automation, Inc. and contributors.
All rights reserved. The contributor(s) of this file has/have agreed to the
RapidStream Contributor License Agreement.
"""

import contextlib
import fcntl
import logging
import os
import re
import subprocess
import threading
from collections.abc import Sequence

from tapa.remote.config import RemoteConfig

_logger = logging.getLogger().getChild(__name__)

_MUX_FAILURE_PATTERNS = (
    "control socket connect",
    "mux_client_hello_exchange",
    "master is dead",
    "stale control socket",
)

_MASTER_READY_LOCK = threading.Lock()
_MASTER_READY_KEYS: set[tuple[str, str, int, str, str, str, bool]] = set()


def _default_ssh_control_dir() -> str:
    xdg_runtime = os.environ.get("XDG_RUNTIME_DIR")
    if xdg_runtime:
        return os.path.join(xdg_runtime, "tapa", "ssh")
    return "/tmp/tapa-ssh-mux"


def get_ssh_control_dir(config: RemoteConfig) -> str:
    """Return the control socket directory for SSH multiplexing."""
    if config.ssh_control_dir:
        return os.path.expanduser(config.ssh_control_dir)
    return _default_ssh_control_dir()


def _ensure_ssh_control_dir(config: RemoteConfig) -> str:
    control_dir = get_ssh_control_dir(config)
    os.makedirs(control_dir, mode=0o700, exist_ok=True)
    with contextlib.suppress(OSError):
        os.chmod(control_dir, 0o700)
    return control_dir


def ssh_target(config: RemoteConfig) -> str:
    """Return the OpenSSH target string for the remote host."""
    return f"{config.user}@{config.host}" if config.user else config.host


def build_ssh_args(config: RemoteConfig) -> list[str]:
    """Build common OpenSSH CLI arguments for remote execution."""
    args = [
        "ssh",
        "-o",
        "BatchMode=yes",
        "-o",
        "StrictHostKeyChecking=accept-new",
        "-o",
        "ConnectTimeout=30",
        "-p",
        str(config.port),
    ]
    if config.key_file:
        args.extend(["-i", os.path.expanduser(config.key_file)])
    if config.ssh_multiplex:
        control_dir = _ensure_ssh_control_dir(config)
        control_path = os.path.join(control_dir, "cm-%C")
        args.extend(
            [
                "-o",
                "ControlMaster=auto",
                "-o",
                f"ControlPath={control_path}",
                "-o",
                f"ControlPersist={config.ssh_control_persist}",
            ]
        )
    return args


def _resolve_control_path(config: RemoteConfig) -> str | None:
    if not config.ssh_multiplex:
        return None
    cmd = [*build_ssh_args(config), "-G", ssh_target(config)]
    result = subprocess.run(
        cmd,
        capture_output=True,
        check=False,
    )
    if result.returncode != 0:
        return None
    output = result.stdout.decode("utf-8", errors="replace")
    match = re.search(r"^controlpath (.+)$", output, re.MULTILINE)
    if match is None:
        return None
    return match.group(1).strip()


def _master_key(config: RemoteConfig) -> tuple[str, str, int, str, str, str, bool]:
    return (
        config.host,
        config.user,
        config.port,
        config.key_file or "",
        get_ssh_control_dir(config),
        config.ssh_control_persist,
        config.ssh_multiplex,
    )


def _is_mux_failure(result: subprocess.CompletedProcess[bytes]) -> bool:
    if result.returncode == 0:
        return False
    stderr = result.stderr.decode("utf-8", errors="replace").lower()
    return any(pattern in stderr for pattern in _MUX_FAILURE_PATTERNS)


def _remove_stale_control_socket(config: RemoteConfig) -> None:
    control_path = _resolve_control_path(config)
    if not control_path:
        return
    if os.path.exists(control_path):
        with contextlib.suppress(OSError):
            os.remove(control_path)
            _logger.warning("Removed stale SSH control socket: %s", control_path)


def _check_master(config: RemoteConfig) -> bool:
    if not config.ssh_multiplex:
        return False
    cmd = [*build_ssh_args(config), "-O", "check", ssh_target(config)]
    result = subprocess.run(
        cmd,
        capture_output=True,
        check=False,
    )
    return result.returncode == 0


def ensure_ssh_master(config: RemoteConfig) -> None:
    """Ensure a shared OpenSSH control master is running."""
    if not config.ssh_multiplex:
        return

    key = _master_key(config)
    with _MASTER_READY_LOCK:
        if key in _MASTER_READY_KEYS:
            return

    control_dir = _ensure_ssh_control_dir(config)
    lock_path = os.path.join(control_dir, "master.lock")
    with open(lock_path, "a", encoding="utf-8") as lock_file:
        fcntl.flock(lock_file.fileno(), fcntl.LOCK_EX)
        if _check_master(config):
            with _MASTER_READY_LOCK:
                _MASTER_READY_KEYS.add(key)
            return
        _remove_stale_control_socket(config)
        cmd = [*build_ssh_args(config), "-MNf", ssh_target(config)]
        result = subprocess.run(
            cmd,
            capture_output=True,
            check=False,
        )
        if result.returncode != 0 and not _check_master(config):
            err = result.stderr.decode("utf-8", errors="replace").strip()
            _logger.warning("Failed to start SSH control master: %s", err)
            return

    with _MASTER_READY_LOCK:
        _MASTER_READY_KEYS.add(key)


def run_ssh(
    config: RemoteConfig,
    remote_command: str,
    *,
    input_bytes: bytes | None = None,
    timeout: float | None = None,
) -> subprocess.CompletedProcess[bytes]:
    """Run a remote command via OpenSSH."""
    ensure_ssh_master(config)
    cmd = [*build_ssh_args(config), ssh_target(config), remote_command]
    result = subprocess.run(
        cmd,
        input=input_bytes,
        capture_output=True,
        timeout=timeout,
        check=False,
    )
    if _is_mux_failure(result):
        _remove_stale_control_socket(config)
        result = subprocess.run(
            cmd,
            input=input_bytes,
            capture_output=True,
            timeout=timeout,
            check=False,
        )
    return result


def run_ssh_with_stdout(
    config: RemoteConfig,
    command: str,
    *,
    timeout: float | None = None,
) -> tuple[int, bytes, bytes]:
    """Run a command and return (returncode, stdout, stderr)."""
    result = run_ssh(config, command, timeout=timeout)
    return result.returncode, result.stdout, result.stderr


def run_ssh_with_stdin(
    config: RemoteConfig,
    command: str,
    data: bytes,
    *,
    timeout: float | None = None,
) -> tuple[int, bytes]:
    """Run a command with binary stdin and return (returncode, stderr)."""
    result = run_ssh(config, command, input_bytes=data, timeout=timeout)
    return result.returncode, result.stderr


def run_local_command(
    argv: Sequence[str],
    *,
    timeout: float | None = None,
) -> subprocess.CompletedProcess[bytes]:
    """Run a local command and capture output as bytes."""
    return subprocess.run(
        list(argv),
        capture_output=True,
        timeout=timeout,
        check=False,
    )
