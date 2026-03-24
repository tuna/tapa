"""OpenSSH helpers for remote execution with multiplexing."""

__copyright__ = """
Copyright (c) 2025 RapidStream Design Automation, Inc. and contributors.
All rights reserved. The contributor(s) of this file has/have agreed to the
RapidStream Contributor License Agreement.
"""

import contextlib
import datetime
import fcntl
import logging
import os
import re
import subprocess
import threading

from tapa.remote.config import RemoteConfig

_logger = logging.getLogger().getChild(__name__)

_MUX_FAILURE_PATTERNS = (
    "control socket connect",
    "mux_client_hello_exchange",
    "mux_client_request_session",
    "read from master failed",
    "master is dead",
    "stale control socket",
    "master refused session request",
    "broken pipe",
)

_MASTER_READY_LOCK = threading.Lock()
_MASTER_READY_KEYS: set[tuple[str, str, int, str, str, str, bool]] = set()
_MASTER_FAILED_KEYS: set[tuple[str, str, int, str, str, str, bool]] = set()


def _default_ssh_control_dir() -> str:
    xdg_runtime = os.environ.get("XDG_RUNTIME_DIR")
    if xdg_runtime:
        return os.path.join(xdg_runtime, "tapa", "ssh")
    return "/tmp/tapa-ssh-mux"


def get_ssh_control_dir(config: RemoteConfig) -> str:
    """Return the control socket directory for SSH multiplexing."""
    return config.ssh_control_dir or _default_ssh_control_dir()


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
        "ConnectTimeout=10",
        "-p",
        str(config.port),
    ]
    if config.key_file:
        args.extend(["-i", config.key_file])
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
                "-o",
                "ServerAliveInterval=30",
                "-o",
                "ServerAliveCountMax=3",
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


def _kill_master(config: RemoteConfig) -> None:
    """Send exit command to the SSH control master process."""
    cmd = [*build_ssh_args(config), "-O", "exit", ssh_target(config)]
    subprocess.run(cmd, capture_output=True, check=False, timeout=10)


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


def _append_mux_log(control_dir: str, message: str) -> None:
    """Append a timestamped line to the mux activity log file."""
    log_path = os.path.join(control_dir, "mux.log")
    ts = datetime.datetime.now().strftime("%Y-%m-%d %H:%M:%S")
    with contextlib.suppress(OSError), open(log_path, "a", encoding="utf-8") as f:
        f.write(f"[{ts}] pid={os.getpid()} {message}\n")


def ensure_ssh_master(
    config: RemoteConfig,
    *,
    force_restart: bool = False,
) -> None:
    """Ensure a shared OpenSSH control master is running.

    Args:
        config: Remote host configuration.
        force_restart: If True, kill any existing master and start fresh.
            Used when a mux failure is detected (e.g., TCP connection died
            but master process is still alive).
    """
    if not config.ssh_multiplex:
        return

    key = _master_key(config)
    if not force_restart:
        with _MASTER_READY_LOCK:
            if key in _MASTER_READY_KEYS or key in _MASTER_FAILED_KEYS:
                return

    control_dir = _ensure_ssh_control_dir(config)
    lock_path = os.path.join(control_dir, "master.lock")
    with open(lock_path, "a", encoding="utf-8") as lock_file:
        fcntl.flock(lock_file.fileno(), fcntl.LOCK_EX)

        if force_restart:
            _kill_master(config)
            _remove_stale_control_socket(config)
            msg = f"SSH mux: force-restarting master for {config.host}:{config.port}"
            _logger.info(msg)
            _append_mux_log(control_dir, msg)
        elif _check_master(config):
            control_path = _resolve_control_path(config)
            msg = (
                f"SSH mux: reusing existing master"
                f" at {control_path}"
                f" for {config.host}:{config.port}"
            )
            _logger.info(msg)
            _append_mux_log(control_dir, msg)
            with _MASTER_READY_LOCK:
                _MASTER_READY_KEYS.add(key)
            return
        else:
            _remove_stale_control_socket(config)

        cmd = [*build_ssh_args(config), "-MNf", ssh_target(config)]
        result = subprocess.run(
            cmd,
            capture_output=True,
            check=False,
        )
        if result.returncode != 0 and not _check_master(config):
            err = result.stderr.decode("utf-8", errors="replace").strip()
            msg = (
                f"SSH mux: FAILED to start master for"
                f" {config.host}:{config.port}: {err}"
            )
            _logger.warning(msg)
            _append_mux_log(control_dir, msg)
            with _MASTER_READY_LOCK:
                _MASTER_FAILED_KEYS.add(key)
            return
        control_path = _resolve_control_path(config)
        msg = (
            f"SSH mux: started new master at {control_path}"
            f" for {config.host}:{config.port}"
        )
        _logger.info(msg)
        _append_mux_log(control_dir, msg)

    with _MASTER_READY_LOCK:
        # Clear any previous failure state on success.
        _MASTER_FAILED_KEYS.discard(key)
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

    # Fail fast if master is known to be unreachable.
    key = _master_key(config)
    with _MASTER_READY_LOCK:
        if key in _MASTER_FAILED_KEYS:
            return subprocess.CompletedProcess(
                args=[],
                returncode=255,
                stdout=b"",
                stderr=b"SSH connection unavailable (master failed)\n",
            )

    cmd = [*build_ssh_args(config), ssh_target(config), remote_command]
    result = subprocess.run(
        cmd,
        input=input_bytes,
        capture_output=True,
        timeout=timeout,
        check=False,
    )
    if _is_mux_failure(result):
        with _MASTER_READY_LOCK:
            _MASTER_READY_KEYS.discard(key)
            _MASTER_FAILED_KEYS.discard(key)
        ensure_ssh_master(config, force_restart=True)

        # Only retry if the new master was established successfully.
        with _MASTER_READY_LOCK:
            if key in _MASTER_FAILED_KEYS:
                return result

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
