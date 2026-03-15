"""Utility to lookup distribution paths for TAPA."""

__copyright__ = """
Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
All rights reserved. The contributor(s) of this file has/have agreed to the
RapidStream Contributor License Agreement.
"""

import logging
import os.path
import platform
import subprocess as _subprocess
from collections.abc import Iterable
from functools import cache
from pathlib import Path
from typing import Literal

_logger = logging.getLogger().getChild(__name__)

# The potential paths for the distribution paths, in order of preference.
# TAPA will attempt to find the executable by iteratively visiting each
# parent directory of the current source file, appending each potential
# path to the parent directory, and checking if the file exists. The first
# match will be used, and the nearest parent directory will be used to
# resolve relative paths.
POTENTIAL_PATHS: dict[str, tuple[str, ...]] = {
    "fpga-runtime-include": (
        "fpga-runtime",
        "usr/include",
    ),
    "fpga-runtime-lib": (
        "fpga-runtime",
        "usr/lib",
    ),
    "tapa-cpp-binary": (
        "tapa-cpp/tapa-cpp",
        "usr/bin/tapa-cpp",
    ),
    "tapa-extra-runtime-include": (
        "tapa-system-include/tapa-extra-runtime-include",
        "usr/include",
    ),
    "tapa-fast-cosim-dpi-lib": (
        "fpga-runtime",
        "usr/lib",
    ),
    "tapa-lib-include": (
        "tapa-lib",
        "usr/include",
    ),
    "tapa-lib-lib": (
        "tapa-lib",
        "usr/lib",
    ),
    "tapa-system-include": (
        "tapa-system-include/tapa-system-include",
        "usr/share/tapa/system-include",
    ),
    "tapacc-binary": (
        "tapacc/tapacc",
        "usr/bin/tapacc",
    ),
}


@cache
def find_resource(file: str) -> Path:
    """Find the resource path in the potential paths.

    Args:
        file: The file to find.

    Returns:
        The path to the executable if found, otherwise None.
    """
    assert file in POTENTIAL_PATHS, f"Unknown file: {file}"

    for path in POTENTIAL_PATHS[file]:
        for parent in Path(__file__).absolute().parents:
            potential_path = parent / path
            if potential_path.exists():
                return potential_path

    error = f"Unable to find {file} in the potential paths"
    raise FileNotFoundError(error)


@cache
def find_external_lib_in_runfiles() -> set[Path]:
    """Find the external libraries in the runfiles.

    Returns:
        The set of external libraries' directories in the Bazel runfiles.
        If the execution is not in a Bazel runfiles, an empty set is returned.
    """
    for parent in Path(__file__).absolute().parents:
        potential_path = parent / "tapa.runfiles"

        # if the execution is in a Bazel runfiles
        if potential_path.exists():
            return {
                potential_path / "gflags+",
                potential_path / "glog+",
                potential_path / "tinyxml2+",
                potential_path / "yaml-cpp+",
                potential_path / "rules_boost++non_module_dependencies+boost",
            }

    return set()


def get_xilinx_tool_path(tool_name: Literal["HLS", "VITIS"] = "HLS") -> str | None:
    """Returns the XILINX_<TOOL> path."""
    xilinx_tool_path = os.environ.get(f"XILINX_{tool_name}")
    if xilinx_tool_path is None:
        _logger.critical("not adding vendor include paths;")
        _logger.critical("please set XILINX_%s", tool_name)
        _logger.critical("you may run `source /path/to/Vitis/settings64.sh`")
    elif not Path(xilinx_tool_path).exists():
        _logger.critical(
            "XILINX_%s path does not exist: %s", tool_name, xilinx_tool_path
        )
        xilinx_tool_path = None
    return xilinx_tool_path


def get_xpfm_path(platform: str) -> str | None:
    """Returns the XPFM path for a platform."""
    xilinx_vitis_path = get_xilinx_tool_path("VITIS")
    path_in_vitis = f"{xilinx_vitis_path}/base_platforms/{platform}/{platform}.xpfm"
    path_in_opt = f"/opt/xilinx/platforms/{platform}/{platform}.xpfm"
    if os.path.exists(path_in_vitis):
        return path_in_vitis
    if os.path.exists(path_in_opt):
        return path_in_opt

    _logger.critical("Cannot find XPFM for platform %s", platform)
    return None


def _get_vendor_include_paths(*, include_gcc: bool) -> Iterable[str]:
    """Yields include paths that are automatically available in vendor tools.

    Args:
        include_gcc: If True, include vendor GCC C++ stdlib headers.
            These are Linux-specific (require glibc) so should only be
            enabled on Linux or when targeting remote Linux HLS execution.
    """
    xilinx_hls: str | None = None
    for tool_name in "HLS", "VITIS":
        # 2024.2 moved the HLS include path from Vitis_HLS to Vitis
        xilinx_hls = get_xilinx_tool_path(tool_name)
        if xilinx_hls is not None:
            include = os.path.join(xilinx_hls, "include")
            if os.path.exists(include):
                yield include
                break

    if xilinx_hls is not None and include_gcc:
        # GCC C++ stdlib headers from the vendor toolchain are Linux-specific
        # (they depend on glibc). On non-Linux (e.g., macOS with remote vendor
        # headers), we skip them and keep the platform's own C++ stdlib.
        tps_lnx64 = Path(xilinx_hls) / "tps" / "lnx64"
        gcc_paths = tps_lnx64.glob("gcc-*.*.*")
        gcc_versions = [path.name.split("-")[1] for path in gcc_paths]
        if not gcc_versions:
            _logger.critical("cannot find HLS vendor GCC")
            _logger.critical("it should be at %s", tps_lnx64)
            return
        gcc_versions.sort(key=lambda x: tuple(map(int, x.split("."))))
        latest_gcc = gcc_versions[-1]

        # include VITIS_HLS/tps/lnx64/g++-<version>/include/c++/<version>
        cpp_include = tps_lnx64 / f"gcc-{latest_gcc}" / "include" / "c++" / latest_gcc
        if not cpp_include.exists():
            _logger.critical("cannot find HLS vendor paths for C++")
            _logger.critical("it should be at %s", cpp_include)
            return
        yield str(cpp_include)

        # there might be a x86_64-pc-linux-gnu or x86_64-linux-gnu
        if (cpp_include / "x86_64-pc-linux-gnu").exists():
            yield os.path.join(cpp_include, "x86_64-pc-linux-gnu")
        elif (cpp_include / "x86_64-linux-gnu").exists():
            yield os.path.join(cpp_include, "x86_64-linux-gnu")
        else:
            _logger.critical("cannot find HLS vendor paths for C++ (x86_64)")
            _logger.critical("it should be at %s", cpp_include)
            return


@cache
def get_vendor_include_paths() -> Iterable[str]:
    """Yields vendor include paths for local compilation."""
    yield from _get_vendor_include_paths(include_gcc=platform.system() == "Linux")


@cache
def get_tapa_cflags() -> tuple[str, ...]:
    """Return the CFLAGS for compiling TAPA programs.

    The CFLAGS include the TAPA include and system include paths when applicable.
    """
    include_flags: list[str] = []

    try:
        tapa_lib_include = find_resource("tapa-lib-include")

        # Validate that the found path actually contains tapa headers,
        # not stale Bazel build artifacts.
        if not (tapa_lib_include / "tapa.h").exists():
            msg = "tapa.h not found in tapa-lib-include"
            raise FileNotFoundError(msg)

        # WORKAROUND: tapa-lib-include must be included first to make Vitis happy
        include_flags.append("-isystem" + str(tapa_lib_include))

        # Add optional runtime includes (may not be available on all platforms).
        for resource in ("fpga-runtime-include", "tapa-extra-runtime-include"):
            try:
                inc = find_resource(resource)
                if inc != tapa_lib_include:
                    include_flags.append("-isystem" + str(inc))
            except FileNotFoundError:
                pass
    except FileNotFoundError:
        _logger.warning(
            "TAPA runtime libraries not found; "
            "runtime include paths will not be added to CFLAGS"
        )

    return (
        *include_flags,
        # Suppress warnings that does not recognize TAPA attributes
        "-Wno-attributes",
        # Suppress warnings that does not recognize HLS pragmas
        "-Wno-unknown-pragmas",
        # Suppress warnings that does not recognize HLS labels
        "-Wno-unused-label",
        # Replace compiler specific builtins with generic ones
        "-D__builtin_FILE()=__FILE__",
        "-D__builtin_LINE()=__LINE__",
    )


@cache
def get_tapa_ldflags() -> tuple[str, ...]:
    """Return the LDFLAGS for linking TAPA programs.

    The LDFLAGS include the TAPA library path when applicable, and adds the -l flags for
    the TAPA libraries.
    """
    libraries = {
        find_resource("fpga-runtime-lib"),
        find_resource("tapa-lib-lib"),
    } | find_external_lib_in_runfiles()
    rpath_flags = [f"-Wl,-rpath,{library}" for library in libraries]
    lib_flags = [f"-L{library}" for library in libraries]

    return (
        *rpath_flags,
        *lib_flags,
        "-ltapa",
        "-lcontext",
        "-lthread",
        "-lfrt",
        "-lglog",
        "-lgflags",
        "-lOpenCL",
        "-ltinyxml2",
        "-lyaml-cpp",
        "-lstdc++fs",
    )


@cache
def get_tapacc_cflags(for_remote_hls: bool = False) -> tuple[str, ...]:
    """Return CFLAGS with vendor libraries for HLS.

    This CFLAGS include the tapa and HLS vendor libraries.

    Args:
        for_remote_hls: If True, include vendor GCC C++ stdlib headers and
            -nostdinc++ even on non-Linux. This is needed when HLS runs on
            a remote Linux host while the local machine is macOS.
    """
    # Add vendor include files to tapacc cflags
    include_gcc = platform.system() == "Linux" or for_remote_hls
    vendor_include_paths = ()
    for vendor_path in _get_vendor_include_paths(include_gcc=include_gcc):
        vendor_include_paths += ("-isystem" + vendor_path,)
        _logger.info("added vendor include path `%s`", vendor_path)

    # TODO: Vitis HLS highly depends on the assert to be defined in
    #       the system include path in a certain way. One attempt was
    #       to disable NDEBUG but this causes hls::assume to fail.
    #       A full solution is to provide tapa::assume that guarantees
    #       the produce the pattern that HLS likes.
    #       For now, newer versions of glibc will not be supported.
    # Only use -nostdinc++ when vendor GCC paths are available to replace it.
    # On non-Linux (e.g., macOS), we keep the platform's C++ stdlib, so we
    # only have the Xilinx include dir without GCC headers to replace it.
    nostdinc_flag = ("-nostdinc++",) if vendor_include_paths and include_gcc else ()

    # When running remote HLS from macOS, the flatten step (tapa-cpp) expands
    # assert() using macOS's assert.h, which calls __assert_rtn. This function
    # doesn't exist on Linux. Map it to Linux's __assert_fail.
    assert_compat_flag = ()
    if for_remote_hls and platform.system() == "Darwin":
        assert_compat_flag = (
            "-D__assert_rtn(func,file,line,expr)=__assert_fail(expr,file,line,func)",
        )

    return (
        # Use the stdc++ library from the HLS toolchain when available.
        *nostdinc_flag,
        *get_tapa_cflags(),
        *vendor_include_paths,
        *assert_compat_flag,
    )


@cache
def _get_macos_sysroot_flags() -> tuple[str, ...]:
    """Return -isysroot flag for macOS SDK if available."""
    if platform.system() != "Darwin":
        return ()

    try:
        sdk_path = _subprocess.check_output(
            ["xcrun", "--show-sdk-path"],
            text=True,
            timeout=10,
        ).strip()
        if sdk_path:
            return ("-isysroot", sdk_path)
    except (FileNotFoundError, _subprocess.SubprocessError):
        _logger.warning("xcrun not found; macOS SDK headers may be missing")

    return ()


@cache
def get_system_cflags() -> tuple[str, ...]:
    """Return CFLAGS for system libraries, such as clang and libc++.

    Uses -idirafter so that LLVM builtin headers from tapa-system-include
    are searched after any platform C++ standard library headers (e.g.,
    macOS libc++), avoiding conflicts with wrapper headers like stddef.h.
    On macOS, also adds -isysroot for the SDK so that tapa-cpp and tapacc
    (custom clang builds) can find system C++ headers.
    """
    flags: list[str] = []
    flags.extend(_get_macos_sysroot_flags())
    if system_include_path := find_resource("tapa-system-include"):
        flags.append("-idirafter" + str(system_include_path))
    return tuple(flags)
