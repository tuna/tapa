"""Reusable Cargo-backed artifact rules."""

# Copyright (c) 2026 RapidStream Design Automation, Inc. and contributors.
# All rights reserved. The contributor(s) of this file has/have agreed to the
# RapidStream Contributor License Agreement.

def _sh_quote(value):
    return "'" + value.replace("'", "'\"'\"'") + "'"

def _runfiles_resolver():
    return """resolve_runfile() {
  local path="$1"
  local workspace="${TEST_WORKSPACE:-_main}"
  local candidate
  for candidate in \\
    "${RUNFILES_DIR:-}/${workspace}/${path}" \\
    "${RUNFILES_DIR:-}/_main/${path}" \\
    "${RUNFILES_DIR:-}/${path}" \\
    "$0.runfiles/${workspace}/${path}" \\
    "$0.runfiles/_main/${path}" \\
    "$0.runfiles/${path}"; do
    if [[ -f "$candidate" ]]; then
      printf '%s\\n' "$candidate"
      return 0
    fi
  done
  if [[ -n "${RUNFILES_MANIFEST_FILE:-}" ]]; then
    for candidate in "${workspace}/${path}" "_main/${path}" "${path}"; do
      local resolved
      resolved="$(grep -m1 "^${candidate} " "${RUNFILES_MANIFEST_FILE}" | cut -d' ' -f2- || true)"
      if [[ -n "$resolved" && -f "$resolved" ]]; then
        printf '%s\\n' "$resolved"
        return 0
      fi
    done
  fi
  echo "missing runfile: $path" >&2
  return 1
}
"""

def _exec_requirements(ctx):
    requirements = {}
    if "requires-network" in ctx.attr.tags:
        requirements["requires-network"] = "1"
    if ctx.attr.local or "local" in ctx.attr.tags:
        requirements["local"] = "1"
    return requirements

def _single_file(target, attr_name):
    files = target.files.to_list()
    if len(files) != 1:
        fail("{} entries must provide exactly one file, got {}".format(attr_name, len(files)))
    return files[0]

def _script_prelude(cargo, rustc, manifest, first_output, build_args):
    return """#!/bin/bash
set -euo pipefail

CARGO={cargo}
RUSTC={rustc}
MANIFEST={manifest}
FIRST_OUTPUT={first_output}

RUST_BIN_DIR="$(cd "$(dirname "$CARGO")" && pwd)"
RUSTC_BIN_DIR="$(cd "$(dirname "$RUSTC")" && pwd)"
export RUSTC
export PATH="$RUST_BIN_DIR:$RUSTC_BIN_DIR:/usr/bin:/bin"

OUTPUT_DIR="$(dirname "$FIRST_OUTPUT")"
WORK_DIR="$OUTPUT_DIR/.{work_name}.cargo-work"
rm -rf "$WORK_DIR"
mkdir -p "$WORK_DIR"
export CARGO_HOME="$WORK_DIR/cargo-home"
export CARGO_TARGET_DIR="$WORK_DIR/cargo-target"

"$CARGO" build --manifest-path "$MANIFEST" --release --locked {build_args}

copy_required() {{
  test -f "$1" || {{ echo "missing Rust artifact: $1" >&2; exit 1; }}
  mkdir -p "$(dirname "$2")"
  cp "$1" "$2"
}}

empty() {{
  mkdir -p "$(dirname "$1")"
  : > "$1"
}}
""".format(
        cargo = _sh_quote(cargo.path),
        rustc = _sh_quote(rustc.path),
        manifest = _sh_quote(manifest.path),
        first_output = _sh_quote(first_output.path),
        work_name = first_output.owner.name,
        build_args = " ".join([_sh_quote(arg) for arg in build_args]),
    )

def _cargo_test_impl(ctx):
    script = ctx.actions.declare_file(ctx.label.name + "_cargo_test.sh")
    env_exports = "\n".join([
        "export {}={}".format(name, _sh_quote(value))
        for name, value in sorted(ctx.attr.cargo_env.items())
        if value
    ])
    env_files = []
    env_file_exports = []
    env_file_runfiles = []
    for target, name in ctx.attr.cargo_env_files.items():
        file = _single_file(target, "cargo_env_files")
        env_files.append(file)
        env_file_exports.append(
            "export {}=\"$(resolve_runfile {})\"".format(name, _sh_quote(file.short_path)),
        )
        env_file_runfiles.extend([
            target[DefaultInfo].default_runfiles,
            target[DefaultInfo].data_runfiles,
        ])
    maybe_skip = ""
    if ctx.attr.skip_without_xilinx_env:
        maybe_skip = """
if [[ -z "${{XILINX_HLS:-}}" && -z "${{REMOTE_HOST:-}}" ]]; then
  echo "{name}: neither XILINX_HLS nor REMOTE_HOST is set; skipping" >&2
  exit 0
fi
if [[ "${{TAPA_SHARED_VADD_HLS:-}}" == "1" ]]; then
  export TAPA_SHARED_VADD_HLS=1
  echo "{name}: TAPA_SHARED_VADD_HLS=1 set" >&2
else
  echo "{name}: TAPA_SHARED_VADD_HLS unset; shared-vadd fixture test will skip" >&2
fi
""".format(name = ctx.label.name)
    body = """#!/usr/bin/env bash
set -euo pipefail

{resolver}

CARGO="$(resolve_runfile {cargo})"
RUSTC="$(resolve_runfile {rustc})"
RUSTDOC="$(resolve_runfile {rustdoc})"
MANIFEST="$(resolve_runfile {manifest})"

RUST_BIN_DIR="$(cd "$(dirname "$CARGO")" && pwd)"
RUSTC_BIN_DIR="$(cd "$(dirname "$RUSTC")" && pwd)"
RUSTDOC_BIN_DIR="$(cd "$(dirname "$RUSTDOC")" && pwd)"
export RUSTC
export RUSTDOC
export PATH="$RUST_BIN_DIR:$RUSTC_BIN_DIR:$RUSTDOC_BIN_DIR:/usr/bin:/bin"
export CARGO_HOME="${{TEST_TMPDIR}}/cargo-home"
export CARGO_TARGET_DIR="${{TEST_TMPDIR}}/cargo-target"
{env_exports}
{env_file_exports}
{maybe_skip}
exec "$CARGO" test --manifest-path "$MANIFEST" --locked {cargo_args} {test_args}
""".format(
        resolver = _runfiles_resolver(),
        cargo = _sh_quote(ctx.file._cargo.short_path),
        rustc = _sh_quote(ctx.file._rustc.short_path),
        rustdoc = _sh_quote(ctx.file._rustdoc.short_path),
        manifest = _sh_quote(ctx.file.manifest.short_path),
        env_exports = env_exports,
        env_file_exports = "\n".join(env_file_exports),
        maybe_skip = maybe_skip,
        cargo_args = " ".join([_sh_quote(arg) for arg in ctx.attr.cargo_args]),
        test_args = " ".join([_sh_quote(arg) for arg in ctx.attr.test_args]),
    )
    ctx.actions.write(script, body, is_executable = True)
    runfiles = ctx.runfiles(files = ctx.files.srcs + ctx.files.data + [
        ctx.file.manifest,
        ctx.file._cargo,
        ctx.file._rustc,
        ctx.file._rustdoc,
    ] + env_files).merge_all(env_file_runfiles)

    # `data` entries are staged with their own runfiles, so a test can invoke
    # a built tool (e.g. `//tapa-core:tapa`) and have that tool find its own
    # siblings in the merged tree. Mirrors `//bazel:test_tool_rules.bzl`.
    for target in ctx.attr.data:
        runfiles = runfiles.merge(target[DefaultInfo].default_runfiles)
    return [DefaultInfo(executable = script, runfiles = runfiles)]

cargo_test = rule(
    implementation = _cargo_test_impl,
    attrs = {
        "srcs": attr.label_list(allow_files = True),
        "data": attr.label_list(allow_files = True),
        "manifest": attr.label(allow_single_file = True, mandatory = True),
        "cargo_args": attr.string_list(),
        "test_args": attr.string_list(),
        "cargo_env": attr.string_dict(),
        "cargo_env_files": attr.label_keyed_string_dict(allow_files = True),
        "skip_without_xilinx_env": attr.bool(default = False),
        "_cargo": attr.label(
            allow_single_file = True,
            cfg = "target",
            default = Label("@rules_rust//rust/toolchain:current_cargo_files"),
        ),
        "_rustc": attr.label(
            allow_single_file = True,
            cfg = "target",
            default = Label("@rules_rust//rust/toolchain:current_rustc_files"),
        ),
        "_rustdoc": attr.label(
            allow_single_file = True,
            cfg = "target",
            default = Label("@rules_rust//rust/toolchain:current_rustdoc_files"),
        ),
    },
    test = True,
)

def _parse_artifacts(ctx, attr_name):
    artifacts = []
    outputs_by_short_path = {}
    package_prefix = ctx.label.package + "/" if ctx.label.package else ""
    for output in ctx.outputs.outs:
        outputs_by_short_path[output.short_path] = output
        if output.short_path.startswith(package_prefix):
            outputs_by_short_path[output.short_path.removeprefix(package_prefix)] = output
    for spec in getattr(ctx.attr, attr_name):
        parts = spec.split(":", 1)
        if len(parts) != 2:
            fail("{} entries must be '<cargo artifact>:<output path>', got {}".format(attr_name, spec))
        artifact, output_path = parts
        if output_path not in outputs_by_short_path:
            fail("{} references undeclared output {}".format(attr_name, output_path))
        artifacts.append((artifact, outputs_by_short_path[output_path]))
    return artifacts

def _cargo_artifacts_impl(ctx):
    outputs = ctx.outputs.outs
    if not outputs:
        fail("outs must not be empty")

    script = ctx.actions.declare_file(ctx.label.name + "_cargo_artifacts.sh")
    body = _script_prelude(
        ctx.file._cargo,
        ctx.file._rustc,
        ctx.file.manifest,
        outputs[0],
        ctx.attr.build_args,
    )
    for artifact, output in _parse_artifacts(ctx, "artifacts"):
        body += "copy_required \"$CARGO_TARGET_DIR/release/{artifact}\" {output}\n".format(
            artifact = artifact,
            output = _sh_quote(output.path),
        )
    for output_path in ctx.attr.executable_outputs:
        package_prefix = ctx.label.package + "/" if ctx.label.package else ""
        outputs_by_short_path = {}
        for out in outputs:
            outputs_by_short_path[out.short_path] = out
            if out.short_path.startswith(package_prefix):
                outputs_by_short_path[out.short_path.removeprefix(package_prefix)] = out
        output = outputs_by_short_path.get(output_path)
        if output == None:
            fail("executable_outputs references undeclared output {}".format(output_path))
        body += "chmod +x {}\n".format(_sh_quote(output.path))

    ctx.actions.write(script, body, is_executable = True)
    ctx.actions.run(
        inputs = depset(ctx.files.srcs + [ctx.file.manifest]),
        outputs = outputs,
        tools = [ctx.file._cargo, ctx.file._rustc, script],
        executable = script,
        mnemonic = "CargoArtifacts",
        execution_requirements = _exec_requirements(ctx),
    )
    return DefaultInfo(files = depset(outputs))

def _cargo_executable_artifacts_impl(ctx):
    outputs = ctx.outputs.outs
    if len(outputs) != 1:
        fail("executable Cargo artifact rules must have exactly one output")

    default = _cargo_artifacts_impl(ctx)
    return DefaultInfo(
        files = default.files,
        executable = outputs[0],
    )

def _cargo_platform_artifacts_impl(ctx):
    outputs = ctx.outputs.outs
    if not outputs:
        fail("outs must not be empty")

    script = ctx.actions.declare_file(ctx.label.name + "_cargo_platform_artifacts.sh")
    body = _script_prelude(
        ctx.file._cargo,
        ctx.file._rustc,
        ctx.file.manifest,
        outputs[0],
        ctx.attr.build_args,
    )
    body += "case \"$(uname -s)\" in\n"
    body += "  Darwin)\n"
    darwin = _parse_artifacts(ctx, "darwin_artifacts")
    linux = _parse_artifacts(ctx, "linux_artifacts")
    for artifact, output in darwin:
        body += "    copy_required \"$CARGO_TARGET_DIR/release/{artifact}\" {output}\n".format(
            artifact = artifact,
            output = _sh_quote(output.path),
        )
    for _, output in linux:
        body += "    empty {}\n".format(_sh_quote(output.path))
    body += "    ;;\n"
    body += "  Linux)\n"
    for artifact, output in linux:
        body += "    copy_required \"$CARGO_TARGET_DIR/release/{artifact}\" {output}\n".format(
            artifact = artifact,
            output = _sh_quote(output.path),
        )
    for _, output in darwin:
        body += "    empty {}\n".format(_sh_quote(output.path))
    body += "    ;;\n"
    body += "  *)\n"
    body += "    echo \"unsupported Rust artifact platform: $(uname -s)\" >&2\n"
    body += "    exit 1\n"
    body += "    ;;\n"
    body += "esac\n"

    ctx.actions.write(script, body, is_executable = True)
    ctx.actions.run(
        inputs = depset(ctx.files.srcs + [ctx.file.manifest]),
        outputs = outputs,
        tools = [ctx.file._cargo, ctx.file._rustc, script],
        executable = script,
        mnemonic = "CargoPlatformArtifacts",
        execution_requirements = _exec_requirements(ctx),
    )
    return DefaultInfo(files = depset(outputs))

_COMMON_ATTRS = {
    "srcs": attr.label_list(allow_files = True),
    "manifest": attr.label(allow_single_file = True, mandatory = True),
    "outs": attr.output_list(mandatory = True),
    "build_args": attr.string_list(),
    "local": attr.bool(default = False),
    "_cargo": attr.label(
        allow_single_file = True,
        cfg = "exec",
        default = Label("@rules_rust//rust/toolchain:current_cargo_files"),
    ),
    "_rustc": attr.label(
        allow_single_file = True,
        cfg = "exec",
        default = Label("@rules_rust//rust/toolchain:current_rustc_files"),
    ),
}

_cargo_artifacts = rule(
    implementation = _cargo_artifacts_impl,
    attrs = dict(_COMMON_ATTRS, **{
        "artifacts": attr.string_list(mandatory = True),
        "executable_outputs": attr.string_list(),
    }),
)

_cargo_executable_artifacts = rule(
    implementation = _cargo_executable_artifacts_impl,
    attrs = dict(_COMMON_ATTRS, **{
        "artifacts": attr.string_list(mandatory = True),
        "executable_outputs": attr.string_list(),
    }),
    executable = True,
)

cargo_platform_artifacts = rule(
    implementation = _cargo_platform_artifacts_impl,
    attrs = dict(_COMMON_ATTRS, **{
        "darwin_artifacts": attr.string_list(mandatory = True),
        "linux_artifacts": attr.string_list(mandatory = True),
    }),
)

def cargo_artifacts(executable = False, **kwargs):
    if executable:
        _cargo_executable_artifacts(**kwargs)
    else:
        _cargo_artifacts(**kwargs)
