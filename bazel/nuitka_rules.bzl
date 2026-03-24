"""Custom rule to add Nuitka target to the target list."""

# Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
# All rights reserved. The contributor(s) of this file has/have agreed to the
# RapidStream Contributor License Agreement.

load("@rules_python//python:py_binary.bzl", "py_binary")
load("@rules_python//python:py_executable_info.bzl", "PyExecutableInfo")
load("@tapa_deps//:requirements.bzl", "requirement")

def _nuitka_binary_impl(ctx):
    src = ctx.attr.src[PyExecutableInfo].main
    flags = ctx.attr.flags
    output_dir = ctx.actions.declare_directory(src.basename.split(".")[0] + ".dist")

    py_toolchain = ctx.toolchains["@rules_python//python:toolchain_type"].py3_runtime
    py_interpreter = py_toolchain.interpreter.path
    py_bin_dir = py_toolchain.interpreter.dirname
    py_lib_dir = py_toolchain.interpreter.dirname.rsplit("/", 1)[0] + "/lib"

    nuitka = ctx.executable.nuitka

    nuitka_cmd = [
        src.path,
        "--clang",
        "--output-dir={}".format(output_dir.dirname),
        "--output-filename={}".format(ctx.attr.output_name or ctx.attr.name),
        "--show-scons",
        "--standalone",
    ]

    for file in ctx.attr.src.files.to_list():
        if not file.is_source:
            nuitka_cmd.append("--noinclude-data-files=" + file.short_path)

    if flags:
        nuitka_cmd.extend(flags)

    tools = []
    for tool_name, tool in {
        "ld": ctx.executable._ld,
        "patchelf": ctx.executable._patchelf,
        "readelf": ctx.executable._readelf,
    }.items():
        if type(tool) == type(""):
            tool_file = ctx.actions.declare_symlink("_nuitka_tools/" + tool_name)
            ctx.actions.symlink(output = tool_file, target_path = tool)
        else:
            tool_file = ctx.actions.declare_file("_nuitka_tools/" + tool_name)
            ctx.actions.symlink(output = tool_file, target_file = tool, is_executable = True)
        tools.append(tool_file)

    is_macos = ctx.target_platform_has_constraint(
        ctx.attr._macos_constraint[platform_common.ConstraintValueInfo],
    )

    env = {
        "PATH": ctx.configuration.host_path_separator.join(
            depset([py_bin_dir] + [x.dirname for x in tools]).to_list() + ["/usr/bin", "/bin"],
        ),
        "LIBRARY_PATH": py_lib_dir,
        "CC": "/usr/bin/clang" if is_macos else ctx.executable._clang.path,
    }
    if not is_macos:
        env["LD_LIBRARY_PATH"] = py_lib_dir

    nuitka_runfiles = ctx.attr.nuitka[DefaultInfo].default_runfiles.files

    site_packages_dirs = {}
    for f in nuitka_runfiles.to_list():
        if "/site-packages/" in f.path:
            idx = f.path.index("/site-packages/")
            site_packages_dirs[f.path[:idx + len("/site-packages")]] = True
    if site_packages_dirs:
        env["PYTHONPATH"] = ctx.configuration.host_path_separator.join(
            sorted(site_packages_dirs.keys()),
        )

    tools += [
        ctx.executable._clang,
        nuitka,
        py_toolchain.interpreter,
    ]
    ctx.actions.run(
        outputs = [output_dir],
        inputs = depset([src], transitive = [py_toolchain.files, nuitka_runfiles]),
        tools = tools,
        executable = py_interpreter,
        arguments = [nuitka.path] + nuitka_cmd,
        mnemonic = "Nuitka",
        env = env,
    )

    return [DefaultInfo(files = depset([output_dir]))]

_nuitka_binary = rule(
    implementation = _nuitka_binary_impl,
    attrs = {
        "src": attr.label(
            doc = "The Python binary to compile.",
            mandatory = True,
            providers = [PyExecutableInfo],
        ),
        "output_name": attr.string(
            doc = "The name of the output binary.",
            mandatory = True,
        ),
        "flags": attr.string_list(
            doc = "Flags to pass to Nuitka.",
            default = [],
        ),
        "nuitka": attr.label(
            doc = "The Nuitka executable.",
            mandatory = True,
            executable = True,
            cfg = "exec",
        ),
        "_clang": attr.label(
            doc = "The clang executable.",
            default = Label("@llvm_toolchain_llvm//:bin/clang"),
            executable = True,
            cfg = "exec",
            allow_files = True,
        ),
        "_ld": attr.label(
            doc = "The ld executable.",
            default = Label("@llvm_toolchain_llvm//:bin/ld.lld"),
            executable = True,
            cfg = "exec",
            allow_files = True,
        ),
        "_patchelf": attr.label(
            doc = "The patchelf executable.",
            default = Label("@patchelf"),
            executable = True,
            cfg = "exec",
        ),
        "_readelf": attr.label(
            doc = "The readelf executable.",
            default = Label("@llvm_toolchain_llvm//:bin/llvm-readelf"),
            executable = True,
            cfg = "exec",
            allow_files = True,
        ),
        "_macos_constraint": attr.label(
            default = Label("@platforms//os:macos"),
        ),
    },
    toolchains = [
        "@rules_python//python:toolchain_type",
    ],
    fragments = ["cpp"],
)

def nuitka_binary(name, src, **kwargs):
    py_binary(
        name = name + ".nuitka.py",
        srcs = ["//bazel:nuitka_wrapper.py"],
        main = "nuitka_wrapper.py",
        deps = [src, requirement("nuitka")],
    )
    _nuitka_binary(
        name = name,
        src = src,
        nuitka = name + ".nuitka.py",
        **kwargs
    )
