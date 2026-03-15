"""Load the local vendor dependencies for the TAPA project based on VARS.bzl"""

# Copyright (c) 2025 RapidStream Design Automation, Inc. and contributors.
# All rights reserved. The contributor(s) of this file has/have agreed to the
# RapidStream Contributor License Agreement.

load("//:VARS.bzl", "XILINX_TOOL_LEGACY_PATH", "XILINX_TOOL_LEGACY_VERSION", "XILINX_TOOL_PATH", "XILINX_TOOL_VERSION")

def _optional_local_repository_impl(rctx):
    """Repository rule that creates a stub when the local path doesn't exist."""
    build_file_content = rctx.attr.build_file_content

    # Try primary path first, then any fallback glob patterns.
    resolved_path = None
    result = rctx.execute(["test", "-d", rctx.attr.path])
    if result.return_code == 0:
        resolved_path = rctx.attr.path
    else:
        for pattern in rctx.attr.fallback_globs:
            # Use shell glob to expand the pattern.
            result = rctx.execute(["sh", "-c", "ls -1d " + pattern + " 2>/dev/null | head -1"])
            candidate = result.stdout.strip()
            if candidate:
                resolved_path = candidate
                break

    if resolved_path:
        # Path exists: symlink each entry individually instead of
        # symlinking "." which fails on some Bazel versions when the
        # repository directory already exists.
        entries = rctx.execute(["ls", "-1", resolved_path])
        for entry in entries.stdout.strip().split("\n"):
            if entry:
                rctx.symlink(resolved_path + "/" + entry, entry)
    rctx.file("BUILD.bazel", build_file_content)

_optional_local_repository = repository_rule(
    implementation = _optional_local_repository_impl,
    local = True,
    attrs = {
        "path": attr.string(mandatory = True),
        "build_file_content": attr.string(mandatory = True),
        "fallback_globs": attr.string_list(default = []),
    },
)

def _load_dependencies(module_ctx):
    """Load dependencies for the TAPA project."""

    # Load the Xilinx Vitis HLS library
    vitis_hls_path = XILINX_TOOL_PATH + (
        "/Vitis/" if XILINX_TOOL_VERSION >= "2024.2" else "/Vitis_HLS/"
    ) + XILINX_TOOL_VERSION

    # On systems without local Xilinx tools (e.g., macOS), fall back to
    # vendor headers cached by `tapa` via remote sync (~/.cache/tapa/vendor-headers/*/).
    # Users can populate this cache by running any `tapa` command with
    # --remote-host or configuring ~/.taparc.
    home = module_ctx.os.environ.get("HOME", "")
    xdg_cache = module_ctx.os.environ.get("XDG_CACHE_HOME", home + "/.cache")
    vendor_cache_glob = xdg_cache + "/tapa/vendor-headers/*"

    _optional_local_repository(
        name = "vitis_hls",
        build_file_content = """
load("@bazel_skylib//lib:selects.bzl", "selects")

cc_library(
    name = "include",
    hdrs = glob(["include/**/*.h"], allow_empty = True),
    defines = select({
        "@platforms//os:macos": [
            # Xilinx std::complex specializations conflict with libc++ inline
            # namespaces (std::__1::complex vs std::complex). Skip them since
            # TAPA programs do not use std::complex<ap_int/ap_fixed>.
            "AP_INT_SPECIAL_H",
            "AP_FIXED_SPECIAL_H",
            # Disable Xilinx FPO library dependency.
            "HLS_NO_XIL_FPO_LIB",
        ],
        "//conditions:default": [],
    }),
    includes = ["include"],
    visibility = ["//visibility:public"],
)
        """,
        path = vitis_hls_path,
        fallback_globs = [vendor_cache_glob],
    )

    # Starting from 2024.2, Vivado has renamed rdi to xv
    vivado_path = XILINX_TOOL_PATH + "/Vivado/"
    xsim_path = vivado_path + XILINX_TOOL_VERSION + "/data/xsim"
    _optional_local_repository(
        name = "xsim_xv",
        build_file_content = """
cc_library(
    name = "svdpi",
    hdrs = glob(["include/svdpi.h"], allow_empty = True),
    includes = ["include"],
    visibility = ["//visibility:public"],
)
        """,
        path = xsim_path,
    )

    # Use the oldest supported version to ensure compatibility
    vivado_legacy_path = XILINX_TOOL_LEGACY_PATH + "/Vivado/"
    xsim_legacy_path = vivado_legacy_path + XILINX_TOOL_LEGACY_VERSION + "/data/xsim"
    _optional_local_repository(
        name = "xsim_legacy_rdi",
        build_file_content = """
cc_library(
    name = "svdpi",
    hdrs = glob(["include/svdpi.h"], allow_empty = True),
    includes = ["include"],
    visibility = ["//visibility:public"],
)
    """,
        # Use the oldest supported version to ensure compatibility
        path = xsim_legacy_path,
    )

    return module_ctx.extension_metadata(
        root_module_direct_deps = [],
        root_module_direct_dev_deps = "all",
        reproducible = False,
    )

load_dependencies = module_extension(
    implementation = _load_dependencies,
)
