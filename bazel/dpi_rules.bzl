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
        # Xilinx's bundled GCC may not know about multiarch system header
        # paths (e.g., /usr/include/x86_64-linux-gnu for asm/types.h).
        "-I/usr/include/x86_64-linux-gnu",
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
                # xsc passes --gcc_compile_options through a shell that
                # mangles values with parentheses. Replace with empty
                # values — visibility attributes are not needed for DPI.
                name = d.split("=", 1)[0]
                compile_options.append("-D" + name + "=")
            else:
                compile_options.append("-D" + d)
        transitive_inputs.append(context.headers)

    link_options = [
        # Prefer libraries in the same directory. Escaping '$' due to `xsc` bug.
        "-Wl,-rpath,\\$ORIGIN",
    ]

    # Link deps into the DPI .so. Prefer static archives so that all symbols
    # are embedded directly; this avoids runtime symbol-lookup failures when
    # xsim loads the DPI library via dlopen(RTLD_LOCAL), which prevents
    # transitive shared-library dependencies from resolving each other's
    # symbols (e.g., libglog.so failing to find gflags symbols).
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
                    # Fall back to dynamic linking for libraries without a
                    # static archive (e.g., pre-built Xilinx simulator libs).
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

    # Embed all static archives with --whole-archive so every symbol is
    # available inside the DPI .so at runtime. Use -L/-l: syntax because
    # xsc prepends '-' to link options that don't already start with '-'.
    # These must come BEFORE system library paths to avoid picking up
    # non-PIC system static archives (e.g., /usr/lib/.../libgflags.a).
    if static_archives:
        link_options.append("-Wl,--whole-archive")
        for archive in static_archives:
            link_options += [
                "-L" + archive.dirname,
                "-l:" + archive.basename,
            ]
        link_options.append("-Wl,--no-whole-archive")

    # Multiarch library paths and RELR compatibility are handled by the
    # xilinx_wrapper via LIBRARY_PATH. Explicit -L paths here would take
    # precedence and bypass the RELR-stripped compat libraries.

    # Assemble arguments.
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
    # Use _no_lto_transition so deps are compiled without LTO. Xilinx's
    # bundled binutils-2.37 cannot handle LTO bitcode objects.
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
