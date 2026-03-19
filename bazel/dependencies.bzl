"""Load the local vendor dependencies for the TAPA project based on VARS.bzl"""

# Copyright (c) 2025 RapidStream Design Automation, Inc. and contributors.
# All rights reserved. The contributor(s) of this file has/have agreed to the
# RapidStream Contributor License Agreement.

load(
    "@vars//:vars.bzl",
    "REMOTE_HOST",
    "REMOTE_KEY_FILE",
    "REMOTE_PORT",
    "REMOTE_SSH_CONTROL_DIR",
    "REMOTE_SSH_CONTROL_PERSIST",
    "REMOTE_USER",
    "REMOTE_XILINX_TOOL_PATH",
    "XILINX_TOOL_LEGACY_PATH",
    "XILINX_TOOL_LEGACY_VERSION",
    "XILINX_TOOL_PATH",
    "XILINX_TOOL_VERSION",
)

def _symlink_dir(rctx, path):
    """Symlink individual entries from a directory into the repository."""
    entries = rctx.execute(["ls", "-1", path])
    for entry in entries.stdout.strip().split("\n"):
        if entry:
            rctx.symlink(path + "/" + entry, entry)

def _optional_local_repository_impl(rctx):
    """Repository rule that creates a stub when the local path doesn't exist."""
    path = rctx.attr.path
    build_file_content = rctx.attr.build_file_content

    result = rctx.execute(["test", "-d", path])
    if result.return_code == 0:
        # Path exists: symlink each entry individually instead of
        # symlinking "." which fails on some Bazel versions when the
        # repository directory already exists.
        _symlink_dir(rctx, path)
    rctx.file("BUILD.bazel", build_file_content)

_optional_local_repository = repository_rule(
    implementation = _optional_local_repository_impl,
    local = True,
    attrs = {
        "path": attr.string(mandatory = True),
        "build_file_content": attr.string(mandatory = True),
    },
)

def _sh_quote(value):
    """Quote a string for POSIX shell single-quoted contexts."""
    return "'" + value.replace("'", "'\"'\"'") + "'"

def _remote_ssh_control_dir(rctx):
    """Return SSH control socket directory for repository fetch."""
    if rctx.attr.remote_ssh_control_dir:
        return rctx.attr.remote_ssh_control_dir
    xdg_runtime = rctx.os.environ.get("XDG_RUNTIME_DIR", "")
    if xdg_runtime:
        return xdg_runtime + "/tapa/ssh"
    return "/tmp/tapa-ssh-mux"

def _fetch_vendor_headers_via_ssh(rctx, remote_path):
    """Fetch vendor headers from the remote host configured in VARS.bzl."""
    host = rctx.attr.remote_host
    user = rctx.attr.remote_user
    port = rctx.attr.remote_port
    key_file = rctx.attr.remote_key_file

    ssh_target = (user + "@" + host) if user else host
    ssh_opts = [
        "-o",
        "BatchMode=yes",
        "-o",
        "StrictHostKeyChecking=accept-new",
        "-o",
        "ConnectTimeout=30",
        "-p",
        port,
    ]
    if key_file:
        home = rctx.os.environ.get("HOME", "")
        resolved_key = key_file.replace("~", home)
        ssh_opts.extend(["-i", resolved_key])
    control_dir = _remote_ssh_control_dir(rctx)
    control_path = control_dir + "/cm-%C"
    ssh_opts.extend([
        "-o",
        "ControlMaster=auto",
        "-o",
        "ControlPath=" + control_path,
        "-o",
        "ControlPersist=" + rctx.attr.remote_ssh_control_persist,
    ])
    rctx.execute(["mkdir", "-p", control_dir])
    rctx.execute(["chmod", "700", control_dir])

    remote_tar_cmd = "tar czf - -C " + _sh_quote(remote_path) + " include"

    # Download include/ directory via tar-over-SSH.
    result = rctx.execute(
        [
            "sh",
            "-c",
            "ssh " + " ".join(ssh_opts) + " " + ssh_target + " " + _sh_quote(remote_tar_cmd) + " | tar xzf -",
        ],
        timeout = 60,
    )
    return result.return_code == 0

def _vitis_hls_repository_impl(rctx):
    """Repository rule for Vitis HLS that fetches headers via SSH if needed."""
    path = rctx.attr.path
    build_file_content = rctx.attr.build_file_content

    result = rctx.execute(["test", "-d", path])
    if result.return_code == 0:
        # Local Xilinx tools available — symlink directly.
        _symlink_dir(rctx, path)
    else:
        # No local tools. Try fetching from remote via SSH.
        fetched = False
        if rctx.attr.remote_host and rctx.attr.remote_path:
            fetched = _fetch_vendor_headers_via_ssh(rctx, rctx.attr.remote_path)

        if not fetched:
            # print is the only way to emit warnings from repository rules.
            # buildifier: disable=print
            print("NOTE: Vitis HLS headers not available. " +
                  "Set REMOTE_HOST in VARS.bzl or install Xilinx tools locally.")

    rctx.file("BUILD.bazel", build_file_content)

_vitis_hls_repository = repository_rule(
    implementation = _vitis_hls_repository_impl,
    local = True,
    attrs = {
        "path": attr.string(mandatory = True),
        "build_file_content": attr.string(mandatory = True),
        "remote_path": attr.string(default = ""),
        "remote_host": attr.string(default = ""),
        "remote_user": attr.string(default = ""),
        "remote_port": attr.string(default = "22"),
        "remote_key_file": attr.string(default = ""),
        "remote_ssh_control_dir": attr.string(default = ""),
        "remote_ssh_control_persist": attr.string(default = "30m"),
    },
)

def _ssh_key_repository_impl(rctx):
    """Repository rule that copies an SSH key file for sandbox access."""
    key_path = rctx.attr.key_file
    if key_path.startswith("~"):
        home = rctx.os.environ.get("HOME", "")
        key_path = home + key_path[1:]

    key_file = rctx.path(key_path)
    if key_file.exists:
        rctx.symlink(key_file, "key")
        rctx.file("BUILD.bazel", 'exports_files(["key"], visibility = ["//visibility:public"])\n')
    else:
        # buildifier: disable=print
        print("NOTE: SSH key file not found: " + key_path)
        rctx.file("key", "")
        rctx.file("BUILD.bazel", 'exports_files(["key"], visibility = ["//visibility:public"])\n')

_ssh_key_repository = repository_rule(
    implementation = _ssh_key_repository_impl,
    local = True,
    attrs = {
        "key_file": attr.string(mandatory = True),
    },
)

def _load_dependencies(module_ctx):
    """Load dependencies for the TAPA project."""

    # Make SSH key available as a Bazel target for sandbox access.
    _ssh_key_repository(
        name = "ssh_key",
        key_file = REMOTE_KEY_FILE if REMOTE_KEY_FILE else "/dev/null",
    )

    # Load the Xilinx Vitis HLS library
    vitis_hls_subdir = "/Vitis/" if XILINX_TOOL_VERSION >= "2024.2" else "/Vitis_HLS/"
    vitis_hls_path = XILINX_TOOL_PATH + vitis_hls_subdir + XILINX_TOOL_VERSION

    # Compute the remote path using the same version logic.
    remote_vitis_hls_path = ""
    if REMOTE_HOST and REMOTE_XILINX_TOOL_PATH:
        remote_vitis_hls_path = REMOTE_XILINX_TOOL_PATH + vitis_hls_subdir + XILINX_TOOL_VERSION

    _vitis_hls_repository(
        name = "vitis_hls",
        build_file_content = """
load("@rules_cc//cc:cc_library.bzl", "cc_library")

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
        remote_path = remote_vitis_hls_path,
        remote_host = REMOTE_HOST,
        remote_user = REMOTE_USER,
        remote_port = REMOTE_PORT,
        remote_key_file = REMOTE_KEY_FILE,
        remote_ssh_control_dir = REMOTE_SSH_CONTROL_DIR,
        remote_ssh_control_persist = REMOTE_SSH_CONTROL_PERSIST,
    )

    # Starting from 2024.2, Vivado has renamed rdi to xv
    vivado_path = XILINX_TOOL_PATH + "/Vivado/"
    xsim_path = vivado_path + XILINX_TOOL_VERSION + "/data/xsim"
    _optional_local_repository(
        name = "xsim_xv",
        build_file_content = """
load("@rules_cc//cc:cc_library.bzl", "cc_library")

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
load("@rules_cc//cc:cc_library.bzl", "cc_library")

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
