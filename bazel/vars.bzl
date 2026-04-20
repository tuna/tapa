"""Resolve VARS.bzl with optional VARS.local.bzl overrides."""

def _parse_vars(contents):
    vars = {}
    for raw_line in contents.splitlines():
        line = raw_line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        name, value = [part.strip() for part in line.split("=", 1)]
        if not name.isupper() or name.startswith("_"):
            continue
        vars[name] = value
    return vars

def _format_vars(vars):
    return "\n".join(["{} = {}".format(key, vars[key]) for key in sorted(vars.keys())])

def _vars_repository_impl(rctx):
    """Repository rule that merges VARS.bzl with VARS.local.bzl overrides."""
    defaults = rctx.path(rctx.attr.defaults)
    local = rctx.path(str(defaults).replace("VARS.bzl", "VARS.local.bzl"))
    merged = _parse_vars(rctx.read(defaults))
    if local.exists:
        merged.update(_parse_vars(rctx.read(local)))

    rctx.file("vars.bzl", _format_vars(merged))
    rctx.file("BUILD.bazel", "")

_vars_repository = repository_rule(
    implementation = _vars_repository_impl,
    local = True,
    attrs = {
        "defaults": attr.label(
            default = Label("//:VARS.bzl"),
            allow_single_file = True,
        ),
    },
)

def _resolve_vars_impl(module_ctx):
    _vars_repository(name = "vars")
    return module_ctx.extension_metadata(
        root_module_direct_deps = [],
        root_module_direct_dev_deps = "all",
        reproducible = False,
    )

resolve_vars = module_extension(
    implementation = _resolve_vars_impl,
)
