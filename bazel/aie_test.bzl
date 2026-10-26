"""Vendor-gated AI Engine system tests."""

def _host_platform_transition_impl(settings, _attr):
    return {
        "//command_line_option:platforms": [settings["//command_line_option:host_platform"]],
    }

_host_platform_transition = transition(
    implementation = _host_platform_transition_impl,
    inputs = ["//command_line_option:host_platform"],
    outputs = ["//command_line_option:platforms"],
)

def _vck190_platform_transition_impl(_settings, _attr):
    return {"//command_line_option:platforms": [Label("//bazel:vck190-aarch64")]}

_vck190_platform_transition = transition(
    implementation = _vck190_platform_transition_impl,
    inputs = [],
    outputs = ["//command_line_option:platforms"],
)

def _aie_vck190_test_impl(ctx):
    tapa = ctx.executable._tapa
    host = ctx.executable.host
    runner = ctx.file._runner
    vpp_env = ctx.executable._vpp_env
    kernel = ctx.file.kernel
    script = ctx.actions.declare_file(ctx.label.name + ".sh")
    script_body = """#!/bin/bash
set -euo pipefail
ROOT="${{TEST_SRCDIR}}/${{TEST_WORKSPACE}}"
exec "${{ROOT}}/{vpp_env}" "${{ROOT}}/{runner}" \
  "${{ROOT}}/{tapa}" \
  "${{ROOT}}/{host}" \
  "${{ROOT}}/{kernel}" \
  {platform}
""".format(
        runner = runner.short_path,
        vpp_env = vpp_env.short_path,
        tapa = tapa.short_path,
        host = host.short_path,
        kernel = kernel.short_path,
        platform = ctx.attr.platform,
    )
    ctx.actions.write(script, script_body, is_executable = True)
    runfiles = ctx.runfiles(files = [tapa, host, runner, vpp_env, kernel] + ctx.files.headers)
    for target in [ctx.attr._tapa, ctx.attr.host[0]]:
        runfiles = runfiles.merge(target[DefaultInfo].default_runfiles)
    return DefaultInfo(executable = script, runfiles = runfiles)

aie_vck190_test = rule(
    implementation = _aie_vck190_test_impl,
    cfg = _host_platform_transition,
    test = True,
    attrs = {
        "_allowlist_function_transition": attr.label(
            default = "@bazel_tools//tools/allowlists/function_transition_allowlist",
        ),
        "headers": attr.label_list(allow_files = True),
        "kernel": attr.label(allow_single_file = True, mandatory = True),
        "platform": attr.string(mandatory = True),
        "host": attr.label(
            cfg = _vck190_platform_transition,
            executable = True,
            mandatory = True,
        ),
        "_runner": attr.label(
            allow_single_file = True,
            default = Label("//tests/functional/aie-vadd:run_vck190.sh"),
        ),
        "_tapa": attr.label(
            cfg = "exec",
            executable = True,
            default = Label("//tapa-core:tapa"),
        ),
        "_vpp_env": attr.label(
            cfg = "exec",
            executable = True,
            default = Label("//bazel:v++_env.sh"),
        ),
    },
)
