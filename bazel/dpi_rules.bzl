"""Custom rule to create DPI libraries."""

# Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
# All rights reserved. The contributor(s) of this file has/have agreed to the
# RapidStream Contributor License Agreement.

load("@rules_cc//cc:defs.bzl", "cc_library")
load("@rules_cc//cc/common:cc_info.bzl", "CcInfo")

def _no_lto_transition_impl(settings, _attr):
    """Remove LTO flags so deps produce regular ELF objects.

    Xilinx's bundled linker (binutils-2.37) cannot handle LTO bitcode
    objects produced by -flto=thin. This transition strips LTO flags from
    the compilation so that static archives contain native ELF objects
    that the old linker can process.
    """
    copts = [c for c in settings["//command_line_option:copt"] if "lto" not in c]
    linkopts = [opt for opt in settings["//command_line_option:linkopt"] if "lto" not in opt]
    return {
        "//command_line_option:copt": copts,
        "//command_line_option:linkopt": linkopts,
    }

_no_lto_transition = transition(
    implementation = _no_lto_transition_impl,
    inputs = ["//command_line_option:copt", "//command_line_option:linkopt"],
    outputs = ["//command_line_option:copt", "//command_line_option:linkopt"],
)

def _dpi_library_impl(ctx):
    compile_options = [
        "-Wall",
        "-Werror",
        "-Wno-sign-compare",
        "-I/usr/include/x86_64-linux-gnu",  # multiarch headers for bundled GCC
    ]
    transitive_inputs = []
    compile_options += ["-isystem" + x.path for x in ctx.files.includes]

    for dep in ctx.attr.deps:
        context = dep[CcInfo].compilation_context
        compile_options += ["-iquote" + x for x in context.quote_includes.to_list()]
        compile_options += ["-isystem" + x for x in context.system_includes.to_list()]
        compile_options += ["-I" + x for x in context.includes.to_list()]
        for d in context.defines.to_list():
            if "(" in d:
                # xsc mangles parentheses in --gcc_compile_options; drop the value.
                name = d.split("=", 1)[0]
                compile_options.append("-D" + name + "=")
            else:
                compile_options.append("-D" + d)
        transitive_inputs.append(context.headers)

    link_options = [
        "-Wl,-rpath,\\$ORIGIN",  # '$' escaped due to xsc bug
    ]

    # Prefer static archives to avoid dlopen(RTLD_LOCAL) symbol-lookup failures.
    direct_inputs = ctx.files.hdrs + ctx.files.srcs
    output = ctx.actions.declare_file(ctx.label.name + ".so")
    runfiles = []
    static_archives = []
    for dep in ctx.attr.deps:
        for linker_input in dep[CcInfo].linking_context.linker_inputs.to_list():
            for library in linker_input.libraries:
                static_lib = library.pic_static_library or library.static_library
                if static_lib:
                    static_archives.append(static_lib)
                    direct_inputs.append(static_lib)
                else:
                    # Fall back to dynamic linking (e.g., pre-built Xilinx simulator libs).
                    dynamic_library = library.resolved_symlink_dynamic_library
                    if dynamic_library == None:
                        dynamic_library = library.dynamic_library
                    if dynamic_library == None:
                        continue
                    direct_inputs.append(dynamic_library)
                    name = dynamic_library.basename
                    name = name.removeprefix("lib")
                    name = name.removesuffix("." + dynamic_library.extension)
                    link_options += [
                        "-L" + dynamic_library.dirname,
                        "-l" + name,
                    ]
                    library_symlink = ctx.actions.declare_file(
                        dynamic_library.basename,
                        sibling = output,
                    )
                    ctx.actions.symlink(
                        output = library_symlink,
                        target_file = dynamic_library,
                    )
                    runfiles.append(library_symlink)

    # Embed static archives with --whole-archive. Use -L/-l: because xsc prepends
    # '-' to options. These must come before system paths to avoid non-PIC archives.
    if static_archives:
        link_options.append("-Wl,--whole-archive")
        for archive in static_archives:
            link_options += [
                "-L" + archive.dirname,
                "-l:" + archive.basename,
            ]
        link_options.append("-Wl,--no-whole-archive")

    # Multiarch paths/RELR compat handled by xilinx_wrapper via LIBRARY_PATH.
    args = [
        "--output=" + output.path,
        "--mt=off",
    ]
    args += ["--gcc_compile_options=" + x for x in compile_options]
    args += ["--gcc_link_options=" + x for x in link_options]
    args += [x.path for x in ctx.files.srcs]

    ctx.actions.run(
        outputs = [output],
        inputs = depset(direct_inputs, transitive = transitive_inputs),
        executable = ctx.executable._xsc,
        tools = [ctx.executable._xsc],
        arguments = args,
        mnemonic = "DpiCompile",
    )
    return [
        DefaultInfo(
            files = depset([output]),
            runfiles = ctx.runfiles(files = runfiles),
        ),
    ]

_DPI_ATTRS = {
    "srcs": attr.label_list(allow_files = True),
    "hdrs": attr.label_list(allow_files = True),
    "includes": attr.label_list(allow_files = True),
    "deps": attr.label_list(providers = [CcInfo], cfg = _no_lto_transition),
    "_allowlist_function_transition": attr.label(
        default = "@bazel_tools//tools/allowlists/function_transition_allowlist",
    ),
}

_dpi_library = rule(
    implementation = _dpi_library_impl,
    attrs = dict(_DPI_ATTRS, **{
        "_xsc": attr.label(
            cfg = "exec",
            default = Label("//bazel:xsc_xv"),
            executable = True,
        ),
    }),
)

_dpi_legacy_rdi_library = rule(
    implementation = _dpi_library_impl,
    attrs = dict(_DPI_ATTRS, **{
        "_xsc": attr.label(
            cfg = "exec",
            default = Label("//bazel:xsc_legacy_rdi"),
            executable = True,
        ),
    }),
)

def dpi_library(name, **kwargs):
    _dpi_library(name = name, **kwargs)
    cc_library(name = name + "_cc", **kwargs)

def dpi_legacy_rdi_library(name, **kwargs):
    _dpi_legacy_rdi_library(name = name, **kwargs)
    cc_library(name = name + "_cc", **kwargs)
