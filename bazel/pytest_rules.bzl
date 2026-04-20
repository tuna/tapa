"""Custom rule to run pytest-based tests."""

# Copyright (c) 2025 RapidStream Design Automation, Inc. and contributors.
# All rights reserved. The contributor(s) of this file has/have agreed to the
# RapidStream Contributor License Agreement.

load("@tapa_deps//:requirements.bzl", "requirement")

def _runfiles_path(file):
    path = file.short_path
    if path.startswith("../"):
        return path[3:]
    return path

def _shell_quote(value):
    return "'" + value.replace("'", "'\"'\"'") + "'"

def _pytest_test_impl(ctx):
    script = ctx.actions.declare_file(ctx.label.name + "_pytest_runner.sh")
    runner_path = _runfiles_path(ctx.executable._runner)
    python_path = _runfiles_path(ctx.file._python)
    args = [_shell_quote(ctx.expand_location(arg, ctx.attr.srcs + ctx.attr.data + ctx.attr.deps)) for arg in ctx.attr.args]
    args.extend(["\"$(resolve_runfile %s)\"" % _shell_quote(_runfiles_path(src)) for src in ctx.files.srcs])

    ctx.actions.write(
        output = script,
        is_executable = True,
        content = """#!/usr/bin/env bash
set -euo pipefail

if [[ -n "${{RUNFILES_DIR:-}}" && -f "${{RUNFILES_DIR}}/bazel_tools/tools/bash/runfiles/runfiles.bash" ]]; then
  source "${{RUNFILES_DIR}}/bazel_tools/tools/bash/runfiles/runfiles.bash"
elif [[ -n "${{RUNFILES_MANIFEST_FILE:-}}" ]]; then
  source "$(grep -m1 '^bazel_tools/tools/bash/runfiles/runfiles.bash ' "${{RUNFILES_MANIFEST_FILE}}" | cut -d' ' -f2-)"
elif [[ -f "$0.runfiles/bazel_tools/tools/bash/runfiles/runfiles.bash" ]]; then
  source "$0.runfiles/bazel_tools/tools/bash/runfiles/runfiles.bash"
else
  echo "cannot find Bazel runfiles library" >&2
  exit 1
fi

resolve_runfile() {{
  local path="$1"
  if [[ -n "${{TEST_SRCDIR:-}}" && -n "${{TEST_WORKSPACE:-}}" && -e "${{TEST_SRCDIR}}/${{TEST_WORKSPACE}}/${{path}}" ]]; then
    printf '%s\\n' "${{TEST_SRCDIR}}/${{TEST_WORKSPACE}}/${{path}}"
    return 0
  fi
  if [[ -n "${{TEST_SRCDIR:-}}" && -e "${{TEST_SRCDIR}}/${{path}}" ]]; then
    printf '%s\\n' "${{TEST_SRCDIR}}/${{path}}"
    return 0
  fi
  rlocation "${{TEST_WORKSPACE:-_main}}/${{path}}" || rlocation "$path"
}}

args=(
  {args}
)

exec "$(resolve_runfile {runner})" --python "$(resolve_runfile {python})" pytest "${{args[@]}}" "$@"
""".format(
            args = "\n  ".join(args),
            runner = _shell_quote(runner_path),
            python = _shell_quote(python_path),
        ),
    )

    runfiles = ctx.runfiles(
        files = ctx.files.srcs + ctx.files.data + ctx.files.deps + [
            ctx.executable._runner,
            ctx.file._python,
        ],
    ).merge(ctx.attr._runfiles_lib[DefaultInfo].default_runfiles)
    runfiles = runfiles.merge_all([
        target[DefaultInfo].default_runfiles
        for target in ctx.attr.data + ctx.attr.deps
    ])

    return [DefaultInfo(executable = script, runfiles = runfiles)]

_pytest_test = rule(
    implementation = _pytest_test_impl,
    test = True,
    attrs = {
        "data": attr.label_list(allow_files = True),
        "deps": attr.label_list(),
        "srcs": attr.label_list(allow_files = [".py"], mandatory = True),
        "_python": attr.label(
            default = Label("@python_3_13//:python3"),
            allow_single_file = True,
        ),
        "_runner": attr.label(
            default = Label("//tools:pytest_runner"),
            executable = True,
            cfg = "target",
        ),
        "_runfiles_lib": attr.label(default = Label("@bazel_tools//tools/bash/runfiles")),
    },
)

def py_test(name, srcs, deps = [], args = [], data = [], **kwargs):
    _pytest_test(
        name = name,
        srcs = srcs,
        deps = deps + [requirement("pytest")],
        args = args,
        data = data,
        **kwargs
    )
