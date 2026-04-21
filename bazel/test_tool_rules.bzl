"""Rules for tests implemented by //tools:test_tools."""

# Copyright (c) 2026 RapidStream Design Automation, Inc. and contributors.
# All rights reserved. The contributor(s) of this file has/have agreed to the
# RapidStream Contributor License Agreement.

def _sh_quote(value):
    return "'" + value.replace("'", "'\"'\"'") + "'"

def _test_tool_test_impl(ctx):
    script = ctx.actions.declare_file(ctx.label.name + ".sh")
    argv = [ctx.attr.command] + ctx.attr.tool_args + [file.short_path for file in ctx.files.data_args]
    body = """#!/usr/bin/env bash
set -euo pipefail

resolve_runfile() {{
  local path="$1"
  local workspace="${{TEST_WORKSPACE:-_main}}"
  local candidate
  for candidate in \\
    "${{RUNFILES_DIR:-}}/${{workspace}}/${{path}}" \\
    "${{RUNFILES_DIR:-}}/_main/${{path}}" \\
    "${{RUNFILES_DIR:-}}/${{path}}" \\
    "$0.runfiles/${{workspace}}/${{path}}" \\
    "$0.runfiles/_main/${{path}}" \\
    "$0.runfiles/${{path}}"; do
    if [[ -f "$candidate" ]]; then
      printf '%s\\n' "$candidate"
      return 0
    fi
  done
  if [[ -n "${{RUNFILES_MANIFEST_FILE:-}}" ]]; then
    for candidate in "${{workspace}}/${{path}}" "_main/${{path}}" "${{path}}"; do
      local resolved
      resolved="$(grep -m1 "^${{candidate}} " "${{RUNFILES_MANIFEST_FILE}}" | cut -d' ' -f2- || true)"
      if [[ -n "$resolved" && -f "$resolved" ]]; then
        printf '%s\\n' "$resolved"
        return 0
      fi
    done
  fi
  echo "missing runfile: $path" >&2
  return 1
}}

exec "$(resolve_runfile {test_tool})" {argv}
""".format(
        test_tool = _sh_quote(ctx.executable.test_tool.short_path),
        argv = " ".join([_sh_quote(arg) for arg in argv]),
    )
    ctx.actions.write(script, body, is_executable = True)
    runfiles = ctx.runfiles(files = ctx.files.data + ctx.files.data_args)
    runfiles = runfiles.merge(ctx.attr.test_tool[DefaultInfo].default_runfiles)
    for target in ctx.attr.data + ctx.attr.data_args:
        runfiles = runfiles.merge(target[DefaultInfo].default_runfiles)
    return [DefaultInfo(executable = script, runfiles = runfiles)]

test_tool_test = rule(
    implementation = _test_tool_test_impl,
    attrs = {
        "command": attr.string(mandatory = True),
        "data": attr.label_list(allow_files = True),
        "data_args": attr.label_list(allow_files = True),
        "test_tool": attr.label(
            cfg = "target",
            default = Label("//tools:test_tools"),
            executable = True,
        ),
        "tool_args": attr.string_list(),
    },
    test = True,
)
