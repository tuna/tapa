"""Load the local vendor dependencies for the TAPA project based on VARS.bzl"""

# Copyright (c) 2025 RapidStream Design Automation, Inc. and contributors.
# All rights reserved. The contributor(s) of this file has/have agreed to the
# RapidStream Contributor License Agreement.

load("//:VARS.bzl", "XILINX_TOOL_LEGACY_PATH", "XILINX_TOOL_LEGACY_VERSION", "XILINX_TOOL_PATH", "XILINX_TOOL_VERSION")

def _optional_local_repository_impl(rctx):
    """Repository rule that creates a stub when the local path doesn't exist."""
    path = rctx.attr.path
    build_file_content = rctx.attr.build_file_content

    result = rctx.execute(["test", "-d", path])
    if result.return_code == 0:
        # Path exists: symlink each entry individually instead of
        # symlinking "." which fails on some Bazel versions when the
        # repository directory already exists.
        entries = rctx.execute(["ls", "-1", path])
        for entry in entries.stdout.strip().split("\n"):
            if entry:
                rctx.symlink(path + "/" + entry, entry)
        rctx.file("BUILD.bazel", build_file_content)
    else:
        # Path does not exist: create a stub with empty targets
        rctx.file("BUILD.bazel", build_file_content)

_optional_local_repository = repository_rule(
    implementation = _optional_local_repository_impl,
    local = True,
    attrs = {
        "path": attr.string(mandatory = True),
        "build_file_content": attr.string(mandatory = True),
    },
)

def _load_dependencies(module_ctx):
    """Load dependencies for the TAPA project."""

    # Load the Xilinx Vitis HLS library
    vitis_hls_path = XILINX_TOOL_PATH + (
        "/Vitis/" if XILINX_TOOL_VERSION >= "2024.2" else "/Vitis_HLS/"
    ) + XILINX_TOOL_VERSION

    _optional_local_repository(
        name = "vitis_hls",
        build_file_content = """
cc_library(
    name = "include",
    hdrs = glob(["include/**/*.h"], allow_empty = True),
    includes = ["include"],
    visibility = ["//visibility:public"],
)
        """,
        path = vitis_hls_path,
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
