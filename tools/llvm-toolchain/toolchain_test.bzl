"""Test the executable files declared by the selected C++ toolchain."""

load("@rules_cc//cc:find_cc_toolchain.bzl", "find_cc_toolchain", "use_cc_toolchain")
load("//bazel:sh_utils.bzl", "RESOLVER", "sh_quote")

def _tool_runfile(path, files):
    for file in files:
        if path == file.path:
            return file.short_path
    for file in files:
        # toolchains_llvm can declare its complete wrapper bin directory.
        if path.startswith(file.path + "/"):
            return file.short_path + path[len(file.path):]
    fail("Selected tool is missing from CcToolchainInfo.all_files: %s" % path)

def _toolchain_test_impl(ctx):
    toolchain = find_cc_toolchain(ctx)
    files = toolchain.all_files.to_list()
    tools = {
        "archiver": toolchain.ar_executable,
        "compiler_driver": toolchain.compiler_executable,
        "coverage": toolchain.gcov_executable,
        "linker": toolchain.ld_executable,
        "nm": toolchain.nm_executable,
        "objcopy": toolchain.objcopy_executable,
        "objdump": toolchain.objdump_executable,
        "preprocessor": toolchain.preprocessor_executable,
        "strip": toolchain.strip_executable,
    }
    multicall = [file for file in files if file.basename == "llvm" and file.dirname.endswith("/bin")]
    if len(multicall) != 1:
        fail("Expected one declared LLVM multicall executable, got: %s" % multicall)
    tool_directory = multicall[0].dirname
    for name in ["clang", "clang++", "llvm"]:
        tools[name] = tool_directory + "/" + name
    argv = []
    for name, path in tools.items():
        argv.append(sh_quote(name + "=") + '"$(resolve_runfile %s)"' % sh_quote(_tool_runfile(path, files)))
    script = ctx.actions.declare_file(ctx.label.name + ".sh")
    ctx.actions.write(
        script,
        """#!/usr/bin/env bash
set -euo pipefail
{resolver}
exec "$(resolve_runfile {checker})" {argv}
""".format(
            resolver = RESOLVER,
            checker = sh_quote(ctx.executable._checker.short_path),
            argv = " ".join(argv),
        ),
        is_executable = True,
    )
    runfiles = ctx.runfiles(transitive_files = toolchain.all_files)
    runfiles = runfiles.merge(ctx.attr._checker[DefaultInfo].default_runfiles)
    return [DefaultInfo(executable = script, runfiles = runfiles)]

toolchain_test = rule(
    implementation = _toolchain_test_impl,
    attrs = {
        "_checker": attr.label(
            default = Label("//tools/llvm-toolchain:check_tools"),
            executable = True,
            cfg = "target",
        ),
    },
    fragments = ["cpp"],
    test = True,
    toolchains = use_cc_toolchain(),
)
