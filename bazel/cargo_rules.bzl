"""Reusable Cargo-backed artifact rules."""

# Copyright (c) 2026 RapidStream Design Automation, Inc. and contributors.
# All rights reserved. The contributor(s) of this file has/have agreed to the
# RapidStream Contributor License Agreement.

def _sh_quote(value):
    return "'" + value.replace("'", "'\"'\"'") + "'"

def _exec_requirements(ctx):
    requirements = {}
    if "requires-network" in ctx.attr.tags:
        requirements["requires-network"] = "1"
    if ctx.attr.local or "local" in ctx.attr.tags:
        requirements["local"] = "1"
    return requirements

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
