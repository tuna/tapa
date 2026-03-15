"""Resolve VARS.bzl with optional VARS.local.bzl overrides."""

_MERGE_SCRIPT = """\
import sys, os
defaults = {}
local_overrides = {}
exec(open(sys.argv[1]).read(), defaults)
if os.path.exists(sys.argv[2]):
    exec(open(sys.argv[2]).read(), local_overrides)
merged = {}
for k, v in defaults.items():
    if not k.startswith('_') and k == k.upper():
        merged[k] = v
for k, v in local_overrides.items():
    if not k.startswith('_') and k == k.upper():
        merged[k] = v
lines = []
for k in sorted(merged.keys()):
    v = merged[k]
    if isinstance(v, bool):
        lines.append(f'{k} = {"True" if v else "False"}')
    elif isinstance(v, str):
        lines.append(f'{k} = "{v}"')
    else:
        lines.append(f'{k} = {v!r}')
print('\\n'.join(lines))
"""

def _vars_repository_impl(rctx):
    """Repository rule that merges VARS.bzl with VARS.local.bzl overrides."""
    defaults = str(rctx.path(rctx.attr.defaults))
    local = defaults.replace("VARS.bzl", "VARS.local.bzl")

    result = rctx.execute(["python3", "-c", _MERGE_SCRIPT, defaults, local])
    if result.return_code != 0:
        fail("Failed to resolve VARS: " + result.stderr)

    rctx.file("vars.bzl", result.stdout)
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
