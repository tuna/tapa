"""ToolProcess ABC and implementations for local and remote execution."""

__copyright__ = """
Copyright (c) 2025 RapidStream Design Automation, Inc. and contributors.
All rights reserved. The contributor(s) of this file has/have agreed to the
RapidStream Contributor License Agreement.
"""

import abc
import contextlib
import io
import logging
import os
import re
import shlex
import shutil
import subprocess
import tarfile
import uuid
from types import TracebackType
from typing import Any, Self

from tapa.remote.config import RemoteConfig, get_remote_config
from tapa.remote.ssh import run_ssh_with_stdin, run_ssh_with_stdout

_logger = logging.getLogger().getChild(__name__)


class ToolProcess(abc.ABC):
    """ABC for tool process wrappers (local or remote)."""

    returncode: int | None = None

    @abc.abstractmethod
    def communicate(self, timeout: float | None = None) -> tuple[bytes, bytes]:
        """Run the process and return (stdout, stderr)."""

    def __enter__(self) -> Self:
        return self

    @abc.abstractmethod
    def __exit__(
        self,
        exc_type: type[BaseException] | None,
        exc_value: BaseException | None,
        traceback: TracebackType | None,
    ) -> None: ...

    def wait(self, timeout: float | None = None) -> int:
        """Wait for the process to complete and return the exit code."""
        self.communicate(timeout=timeout)
        assert self.returncode is not None
        return self.returncode


class LocalToolProcess(ToolProcess):
    """Wraps subprocess.Popen for local execution."""

    def __init__(
        self,
        cmd_args: list[str] | str,
        *,
        cwd: str | None = None,
        env: dict[str, str] | None = None,
        stdout: int | None = subprocess.PIPE,
        stderr: int | None = subprocess.PIPE,
        **kwargs: Any,  # noqa: ANN401
    ) -> None:
        self._proc = subprocess.Popen(
            cmd_args,
            cwd=cwd,
            env=env,
            stdout=stdout,
            stderr=stderr,
            **kwargs,
        )
        self.returncode = self._proc.returncode

    def communicate(self, timeout: float | None = None) -> tuple[bytes, bytes]:
        stdout, stderr = self._proc.communicate(timeout=timeout)
        self.returncode = self._proc.returncode
        return (
            stdout if isinstance(stdout, bytes) else b"",
            stderr if isinstance(stderr, bytes) else b"",
        )

    def __exit__(
        self,
        exc_type: type[BaseException] | None,
        exc_value: BaseException | None,
        traceback: TracebackType | None,
    ) -> None:
        self._proc.__exit__(exc_type, exc_value, traceback)
        self.returncode = self._proc.returncode


def _untar_to_directory(data: bytes, local_path: str) -> None:
    """Extract a tar.gz archive into a directory.

    Handles conflicts where a file and directory share the same name
    (e.g., Vitis HLS creates kernel.xml as a directory, but a prior
    step may have created it as a file).
    """
    buf = io.BytesIO(data)
    with tarfile.open(mode="r:gz", fileobj=buf) as tar:
        for member in tar:
            target = os.path.join(local_path, member.name)
            if member.isdir():
                if os.path.exists(target) and not os.path.isdir(target):
                    os.remove(target)
                os.makedirs(target, exist_ok=True)
            else:
                if os.path.isdir(target):
                    shutil.rmtree(target)
                tar.extract(member, path=local_path, filter="fully_trusted")


def _local_to_remote_path(local_path: str, session_dir: str) -> str:
    """Map a local absolute path to a remote path under the session dir."""
    rel = local_path.lstrip("/")
    return f"{session_dir}/rootfs/{rel}"


def _rewrite_paths_in_string(
    text: str,
    local_paths: list[str],
    session_dir: str,
) -> str:
    """Rewrite local absolute paths in a string to remote paths (single-pass).

    Uses regex alternation to avoid double-replacement when one local path
    is a prefix of another (e.g., /a/b/c and /a/b/c/x86_64-linux-gnu).
    """
    if not local_paths:
        return text
    # Sort longest-first so longer paths match before their parent prefixes
    sorted_paths = sorted(local_paths, key=len, reverse=True)
    # Build a single regex with alternation of all escaped paths
    pattern = re.compile("|".join(re.escape(p) for p in sorted_paths))
    return pattern.sub(lambda m: _local_to_remote_path(m.group(0), session_dir), text)


def _rewrite_cmd_args(
    cmd_args: list[str] | str,
    local_paths: list[str],
    session_dir: str,
) -> list[str] | str:
    """Rewrite local absolute paths in command args to remote paths."""
    if isinstance(cmd_args, str):
        return _rewrite_paths_in_string(cmd_args, local_paths, session_dir)

    return [_rewrite_paths_in_string(arg, local_paths, session_dir) for arg in cmd_args]


def _upload_paths(config: RemoteConfig, paths: list[str], session_dir: str) -> None:
    """Upload all local files/dirs to remote in a single SSH session.

    Creates a single tar archive containing all paths (preserving their
    absolute directory structure) and extracts it on the remote host under
    ``<session_dir>/rootfs/``.  This uses one SSH channel instead of one
    per path, dramatically reducing the number of multiplexed sessions.
    """
    rootfs = f"{session_dir}/rootfs"

    for p in paths:
        if not os.path.exists(p):
            _logger.warning("Upload path does not exist: %s", p)
    valid_paths = [p for p in paths if os.path.exists(p)]

    if not valid_paths:
        run_ssh_with_stdout(config, f"mkdir -p {shlex.quote(rootfs)}")
        return

    buf = io.BytesIO()
    with tarfile.open(mode="w:gz", fileobj=buf, dereference=True) as tar:
        for local_path in valid_paths:
            arcname_prefix = local_path.lstrip("/")
            remote_path = _local_to_remote_path(local_path, session_dir)
            _logger.info("Uploading %s -> %s", local_path, remote_path)
            if os.path.isdir(local_path):
                for entry in os.listdir(local_path):
                    tar.add(
                        os.path.join(local_path, entry),
                        arcname=os.path.join(arcname_prefix, entry),
                    )
            else:
                tar.add(local_path, arcname=arcname_prefix)
    tar_data = buf.getvalue()

    command = (
        f"mkdir -p {shlex.quote(rootfs)} && "
        f"tar xzf - -C {shlex.quote(rootfs)} --no-same-owner"
    )
    returncode, stderr = run_ssh_with_stdin(config, command, tar_data)
    if returncode != 0:
        err = stderr.decode("utf-8", errors="replace")
        _logger.error("Batch upload failed: %s", err)


def _download_paths(config: RemoteConfig, paths: list[str], session_dir: str) -> None:
    """Download remote directories back to local via tar-over-SSH."""
    for local_path in paths:
        remote_path = _local_to_remote_path(local_path, session_dir)
        _logger.info("Downloading %s -> %s", remote_path, local_path)
        returncode, tar_data, dl_stderr = run_ssh_with_stdout(
            config,
            f"tar czf - -C {shlex.quote(remote_path)} .",
        )
        if returncode != 0:
            err = dl_stderr.decode("utf-8", errors="replace")
            _logger.warning("Download failed for %s: %s", remote_path, err)
            continue
        if tar_data:
            os.makedirs(local_path, exist_ok=True)
            _untar_to_directory(tar_data, local_path)


# Environment variables safe to forward to the remote host.
# Everything else (secrets, macOS-specific vars, etc.) is dropped.
_REMOTE_ENV_ALLOWLIST = {"HOME", "LANG", "LC_ALL", "LC_CTYPE"}


def _build_remote_command(  # noqa: PLR0913, PLR0917
    rewritten_args: list[str] | str,
    remote_cwd: str,
    env: dict[str, str],
    all_local_paths: list[str],
    session_dir: str,
    xilinx_settings: str | None,
) -> str:
    """Build the full remote shell command string."""
    cmd_parts: list[str] = []

    if xilinx_settings:
        cmd_parts.append(f"source {shlex.quote(xilinx_settings)}")

    for key, val in env.items():
        if key not in _REMOTE_ENV_ALLOWLIST and not key.startswith("TAPA_"):
            continue
        remapped_val = _rewrite_paths_in_string(val, all_local_paths, session_dir)
        cmd_parts.append(f"export {key}={shlex.quote(remapped_val)}")

    if isinstance(rewritten_args, str):
        cmd_parts.append(f"cd {shlex.quote(remote_cwd)} && exec {rewritten_args}")
    else:
        cmd_parts.append(
            f"cd {shlex.quote(remote_cwd)} && exec "
            + " ".join(shlex.quote(a) for a in rewritten_args)
        )

    return " ; ".join(cmd_parts)


class RemoteToolProcess(ToolProcess):
    """Executes a tool process on a remote machine via SSH."""

    def __init__(  # noqa: PLR0913
        self,
        cmd_args: list[str] | str,
        *,
        cwd: str | None = None,
        env: dict[str, str] | None = None,
        config: RemoteConfig,
        extra_upload_paths: tuple[str, ...] = (),
        extra_download_paths: tuple[str, ...] = (),
        download_cwd: bool = False,
        **kwargs: Any,  # noqa: ANN401, ARG002
    ) -> None:
        self._cmd_args = cmd_args
        self._cwd = cwd or os.getcwd()
        self._env = env or {}
        self._config = config
        self._extra_upload_paths = extra_upload_paths
        self._extra_download_paths = extra_download_paths
        self._download_cwd = download_cwd
        self._session_dir = f"{config.work_dir}/{uuid.uuid4()}"
        self._communicated = False

    def communicate(self, timeout: float | None = None) -> tuple[bytes, bytes]:
        if self._communicated:
            return b"", b""
        self._communicated = True

        all_local_paths = list(
            dict.fromkeys(
                [self._cwd, *self._extra_upload_paths, *self._extra_download_paths]
            )
        )

        _logger.info("Creating remote session directory: %s", self._session_dir)
        _upload_paths(
            self._config, [self._cwd, *self._extra_upload_paths], self._session_dir
        )

        remote_cwd = _local_to_remote_path(self._cwd, self._session_dir)
        full_cmd = _build_remote_command(
            _rewrite_cmd_args(self._cmd_args, all_local_paths, self._session_dir),
            remote_cwd,
            self._env,
            all_local_paths,
            self._session_dir,
            self._config.xilinx_settings,
        )
        _logger.info("Executing remote command: %s", full_cmd)

        returncode, stdout_data, stderr_data = run_ssh_with_stdout(
            self._config,
            f"bash -c {shlex.quote(full_cmd)}",
            timeout=timeout,
        )
        self.returncode = returncode
        _logger.info("Remote command exited with code %d", self.returncode)

        download_list = [
            *([self._cwd] if self._download_cwd else []),
            *self._extra_download_paths,
        ]
        _download_paths(self._config, download_list, self._session_dir)

        return stdout_data, stderr_data

    def __exit__(
        self,
        exc_type: type[BaseException] | None,
        exc_value: BaseException | None,
        traceback: TracebackType | None,
    ) -> None:
        if not self._communicated:
            with contextlib.suppress(Exception):
                self.communicate()


def create_tool_process(  # noqa: PLR0913
    cmd_args: list[str] | str,
    *,
    cwd: str | None = None,
    env: dict[str, str] | None = None,
    extra_upload_paths: tuple[str, ...] = (),
    extra_download_paths: tuple[str, ...] = (),
    download_cwd: bool = False,
    **kwargs: Any,  # noqa: ANN401
) -> ToolProcess:
    """Factory: create a local or remote tool process based on config."""
    config = get_remote_config()
    if config is not None:
        return RemoteToolProcess(
            cmd_args,
            cwd=cwd,
            env=env,
            config=config,
            extra_upload_paths=extra_upload_paths,
            extra_download_paths=extra_download_paths,
            download_cwd=download_cwd,
            **kwargs,
        )
    return LocalToolProcess(cmd_args, cwd=cwd, env=env, **kwargs)
