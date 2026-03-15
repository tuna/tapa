"""Fetch vendor (Xilinx) include headers from a remote host."""

import glob
import hashlib
import io
import logging
import os
import platform
import re
import shlex
import tarfile

import paramiko

from tapa.remote.config import RemoteConfig
from tapa.remote.connection import get_connection

_logger = logging.getLogger().getChild(__name__)

_CACHE_BASE = os.path.join(
    os.environ.get("XDG_CACHE_HOME", os.path.expanduser("~/.cache")),
    "tapa",
    "vendor-headers",
)


def _cache_key(config: RemoteConfig) -> str:
    """Compute a cache key from the remote config."""
    raw = f"{config.host}:{config.port}:{config.xilinx_settings or ''}"
    return hashlib.sha256(raw.encode()).hexdigest()[:16]


def _query_remote_xilinx_paths(
    ssh: paramiko.SSHClient,
    xilinx_settings: str,
) -> dict[str, str]:
    """Source settings64.sh on remote and return XILINX_HLS and XILINX_VITIS."""
    cmd = (
        f"source {shlex.quote(xilinx_settings)} && "
        "echo XILINX_HLS=$XILINX_HLS && echo XILINX_VITIS=$XILINX_VITIS"
    )
    _, stdout, stderr = ssh.exec_command(cmd)
    output = stdout.read().decode("utf-8", errors="replace")
    exit_status = stdout.channel.recv_exit_status()
    if exit_status != 0:
        err = stderr.read().decode("utf-8", errors="replace")
        _logger.warning("Failed to query Xilinx paths on remote: %s", err)
        return {}

    result: dict[str, str] = {}
    for line in output.strip().splitlines():
        if "=" in line:
            key, _, val = line.partition("=")
            if val:
                result[key] = val
    return result


def _download_dir(
    ssh: paramiko.SSHClient,
    remote_path: str,
    local_path: str,
) -> bool:
    """Download a remote directory to a local path via tar-over-SSH."""
    _, stdout, stderr = ssh.exec_command(
        f"tar czf - -C {shlex.quote(remote_path)} .",
    )
    tar_data = stdout.read()
    exit_status = stdout.channel.recv_exit_status()
    if exit_status != 0:
        err = stderr.read().decode("utf-8", errors="replace")
        _logger.warning("Download failed for %s: %s", remote_path, err)
        return False
    if not tar_data:
        _logger.warning("Empty tar for %s", remote_path)
        return False

    os.makedirs(local_path, exist_ok=True)
    buf = io.BytesIO(tar_data)
    with tarfile.open(mode="r:gz", fileobj=buf) as tar:
        tar.extractall(path=local_path, filter="fully_trusted")
    return True


def _patch_vendor_headers_for_macos(cache_dir: str) -> None:
    """Patch vendor headers for macOS libc++ compatibility.

    On macOS, libc++ puts std::complex in an inline namespace (std::__1::complex).
    Xilinx vendor headers (ap_int_special.h, ap_fixed_special.h) forward-declare
    'namespace std { template<typename _Tp> class complex; }' which creates a
    separate std::complex that conflicts with std::__1::complex, causing ambiguity.

    This patches those files to use '#include <complex>' instead.
    """
    if platform.system() != "Darwin":
        return

    marker = os.path.join(cache_dir, ".patched_macos_complex")
    if os.path.exists(marker):
        return

    # Pattern: the forward declaration block in ap_int_special.h / ap_fixed_special.h
    pattern = re.compile(
        r"// FIXME AP_AUTOCC cannot handle many standard headers,"
        r" so declare instead of\n"
        r"// include\.\n"
        r"// #include <complex>\n"
        r"namespace std \{\n"
        r"template<typename _Tp> class complex;\n"
        r"\}"
    )
    replacement = "#include <complex>"

    patched_any = False
    for header in glob.glob(
        os.path.join(cache_dir, "include", "etc", "ap_*_special.h")
    ):
        with open(header, encoding="utf-8") as f:
            content = f.read()
        new_content = pattern.sub(replacement, content)
        if new_content != content:
            with open(header, "w", encoding="utf-8") as f:
                f.write(new_content)
            _logger.info("Patched %s for macOS libc++ compatibility", header)
            patched_any = True

    if patched_any:
        with open(marker, "w", encoding="utf-8") as f:
            f.write("patched\n")
        _logger.info("Applied macOS libc++ compatibility patches")


def sync_remote_vendor_includes(config: RemoteConfig) -> str | None:
    """Fetch Xilinx vendor include headers from the remote host.

    Downloads the include/ and tps/lnx64/gcc-*/include/ directories to a
    local cache so that tapacc can use them during the analyze step.

    Returns the local path mimicking XILINX_HLS, or None on failure.
    """
    if not config.xilinx_settings:
        _logger.info("No xilinx_settings configured; skipping vendor header sync")
        return None

    cache_dir = os.path.join(_CACHE_BASE, _cache_key(config))
    marker = os.path.join(cache_dir, ".synced")
    if os.path.exists(marker):
        _logger.info("Using cached vendor headers from %s", cache_dir)
        _patch_vendor_headers_for_macos(cache_dir)
        return cache_dir

    _logger.info("Fetching vendor include headers from %s ...", config.host)

    ssh = get_connection(config)
    paths = _query_remote_xilinx_paths(ssh, config.xilinx_settings)

    xilinx_tool = paths.get("XILINX_HLS") or paths.get("XILINX_VITIS")
    if not xilinx_tool:
        _logger.warning("Could not determine XILINX_HLS or XILINX_VITIS on remote")
        return None

    os.makedirs(cache_dir, exist_ok=True)

    # Download include/ (ap_int.h, ap_utils.h, hls_stream.h, etc.)
    remote_include = f"{xilinx_tool}/include"
    local_include = os.path.join(cache_dir, "include")
    if not _download_dir(ssh, remote_include, local_include):
        _logger.warning("Failed to download vendor include directory")
        return None
    _logger.info("Downloaded %s -> %s", remote_include, local_include)

    # Download tps/lnx64/gcc-*/include/ (C++ stdlib headers)
    # First, find which gcc versions exist
    _, stdout, _ = ssh.exec_command(
        f"ls -d {shlex.quote(xilinx_tool)}/tps/lnx64/gcc-*/include 2>/dev/null"
    )
    gcc_include_dirs = stdout.read().decode("utf-8", errors="replace").strip()
    stdout.channel.recv_exit_status()

    for gcc_include_raw in gcc_include_dirs.splitlines():
        gcc_include = gcc_include_raw.strip()
        if not gcc_include:
            continue
        # Compute relative path: tps/lnx64/gcc-X.Y.Z/include
        rel = os.path.relpath(gcc_include, xilinx_tool)
        local_gcc = os.path.join(cache_dir, rel)
        if _download_dir(ssh, gcc_include, local_gcc):
            _logger.info("Downloaded %s -> %s", gcc_include, local_gcc)

    # Patch vendor headers for macOS compatibility before marking as synced
    _patch_vendor_headers_for_macos(cache_dir)

    # Write marker to indicate successful sync
    with open(marker, "w", encoding="utf-8") as f:
        f.write(xilinx_tool + "\n")

    _logger.info("Vendor headers cached at %s", cache_dir)
    return cache_dir
