"""Expose the installed Vitis aarch64 sysroot to the cross toolchain."""

load("@vars//:vars.bzl", "XILINX_TOOL_PATH", "XILINX_TOOL_VERSION")

def _vck190_sysroot_impl(rctx):
    sysroot = "{}/{}/Vitis/gnu/aarch64/lin/aarch64-linux/cortexa72-cortexa53-amd-linux".format(
        XILINX_TOOL_PATH,
        XILINX_TOOL_VERSION,
    )
    if not rctx.path(sysroot).exists:
        rctx.file("sysroot/MISSING", "Vitis aarch64 sysroot not found: {}\n".format(sysroot))
    else:
        for entry in rctx.path(sysroot).readdir():
            rctx.symlink(entry, "sysroot/" + entry.basename)
    rctx.file(
        "sysroot/BUILD.bazel",
        """filegroup(
    name = "sysroot",
    srcs = ["."],
    visibility = ["//visibility:public"],
)
""",
    )

vck190_sysroot = repository_rule(
    implementation = _vck190_sysroot_impl,
    local = True,
)
