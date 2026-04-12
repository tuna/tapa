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

from tapa.remote.config import RemoteConfig
from tapa.remote.ssh import run_ssh_with_stdout

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
    config: RemoteConfig,
    xilinx_settings: str,
) -> dict[str, str]:
    """Source settings64.sh on remote and return XILINX_HLS and XILINX_VITIS."""
    cmd = (
        f"source {shlex.quote(xilinx_settings)} && "
        "echo XILINX_HLS=$XILINX_HLS && echo XILINX_VITIS=$XILINX_VITIS"
    )
    exit_status, stdout, stderr = run_ssh_with_stdout(config, cmd)
    if exit_status != 0:
        _logger.warning(
            "Failed to query Xilinx paths on remote: %s",
            stderr.decode("utf-8", errors="replace"),
        )
        return {}

    return {
        key: val
        for line in stdout.decode("utf-8", errors="replace").strip().splitlines()
        if "=" in line
        for key, _, val in [line.partition("=")]
        if val
    }


def _download_dir(
    config: RemoteConfig,
    remote_path: str,
    local_path: str,
) -> bool:
    """Download a remote directory to a local path via tar-over-SSH."""
    exit_status, stdout, stderr = run_ssh_with_stdout(
        config, f"tar czf - -C {shlex.quote(remote_path)} ."
    )
    if exit_status != 0:
        _logger.warning(
            "Download failed for %s: %s",
            remote_path,
            stderr.decode("utf-8", errors="replace"),
        )
        return False
    if not stdout:
        _logger.warning("Empty tar for %s", remote_path)
        return False

    os.makedirs(local_path, exist_ok=True)
    with tarfile.open(mode="r:gz", fileobj=io.BytesIO(stdout)) as tar:
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

    paths = _query_remote_xilinx_paths(config, config.xilinx_settings)

    xilinx_tool = paths.get("XILINX_HLS") or paths.get("XILINX_VITIS")
    if not xilinx_tool:
        _logger.warning("Could not determine XILINX_HLS or XILINX_VITIS on remote")
        return None

    os.makedirs(cache_dir, exist_ok=True)

    # Remove stale patch marker so that _patch_vendor_headers_for_macos
    # re-applies patches after a fresh download (e.g., if a previous run
    # wrote the marker but crashed before writing .synced).
    patch_marker = os.path.join(cache_dir, ".patched_macos_complex")
    if os.path.exists(patch_marker):
        os.remove(patch_marker)

    remote_include = f"{xilinx_tool}/include"
    local_include = os.path.join(cache_dir, "include")
    if not _download_dir(config, remote_include, local_include):
        _logger.warning("Failed to download vendor include directory")
        return None
    _logger.info("Downloaded %s -> %s", remote_include, local_include)

    # Find and download tps/lnx64/gcc-*/include/ (C++ stdlib headers)
    _, stdout, _ = run_ssh_with_stdout(
        config, f"ls -d {shlex.quote(xilinx_tool)}/tps/lnx64/gcc-*/include 2>/dev/null"
    )
    for gcc_include in (
        ln.strip()
        for ln in stdout.decode("utf-8", errors="replace").strip().splitlines()
        if ln.strip()
    ):
        local_gcc = os.path.join(cache_dir, os.path.relpath(gcc_include, xilinx_tool))
        if _download_dir(config, gcc_include, local_gcc):
            _logger.info("Downloaded %s -> %s", gcc_include, local_gcc)

    _patch_vendor_headers_for_macos(cache_dir)

    with open(marker, "w", encoding="utf-8") as f:
        f.write(xilinx_tool + "\n")

    _logger.info("Vendor headers cached at %s", cache_dir)
    return cache_dir
