"""Custom rule to add TAPA target to the target list."""

# Copyright (c) 2025 RapidStream Design Automation, Inc. and contributors.
# All rights reserved. The contributor(s) of this file has/have agreed to the
# RapidStream Contributor License Agreement.

load(
    "@vars//:vars.bzl",
    "REMOTE_HOST",
    "REMOTE_KEY_FILE",
    "REMOTE_PORT",
    "REMOTE_SSH_CONTROL_DIR",
    "REMOTE_SSH_CONTROL_PERSIST",
    "REMOTE_USER",
    "REMOTE_XILINX_SETTINGS",
    "REMOTE_XILINX_TOOL_PATH",
    "XILINX_PART_NUM",
    "XILINX_TOOL_VERSION",
)
load("@vitis_hls//:defs.bzl", "VITIS_HLS_RUNNER_AVAILABLE", "VIVADO_AVAILABLE")
load("//bazel:sh_utils.bzl", "sh_quote")
load("//bazel:xilinx_versions.bzl", "vitis_layout_subdir")

def _remote_host_flag():
    if not REMOTE_HOST:
        return ""
    host_part = REMOTE_HOST
    if REMOTE_USER:
        host_part = REMOTE_USER + "@" + host_part

    # Always include port so VARS.local.bzl port overrides any ~/.taparc port.
    host_part = host_part + ":" + REMOTE_PORT
    return host_part

def _remote_xilinx_settings():
    if REMOTE_XILINX_SETTINGS:
        return REMOTE_XILINX_SETTINGS
    if REMOTE_XILINX_TOOL_PATH:
        subdir = vitis_layout_subdir(XILINX_TOOL_VERSION)
        return REMOTE_XILINX_TOOL_PATH + subdir + XILINX_TOOL_VERSION + "/settings64.sh"
    return ""

def _remote_tapa_args(ctx):
    remote_args = []
    remote_host = _remote_host_flag()
    if remote_host:
        remote_args.extend(["--remote-host", remote_host])
        if ctx.file.ssh_key:
            remote_args.extend(["--remote-key-file", ctx.file.ssh_key.path])
        xilinx_settings = _remote_xilinx_settings()
        if xilinx_settings:
            remote_args.extend(["--remote-xilinx-settings", xilinx_settings])
        if REMOTE_SSH_CONTROL_DIR:
            remote_args.extend(["--remote-ssh-control-dir", REMOTE_SSH_CONTROL_DIR])
        if REMOTE_SSH_CONTROL_PERSIST:
            remote_args.extend(["--remote-ssh-control-persist", REMOTE_SSH_CONTROL_PERSIST])
    return remote_args

# Peak RSS of one Vitis HLS process, in MB. Measured across the regression
# designs; wide 512-bit sparse tasks sit at the top of that range.
_HLS_JOB_MB = 2048

# Peak RSS of the single Vivado that `tapa pack` spawns for IP packaging, in
# MB. This is the floor rather than an addend: packing starts after every HLS
# job has exited, so an action's high-water mark is one phase or the other,
# never their sum. Designs with thousands of tasks exceed it (lu_decompose,
# 1505 tasks / 9104 Verilog files, peaked near 17 GB), but sizing every action
# for that outlier would serialize the whole build.
_PACK_MB = 6144

def _vendor_exec_requirements(jobs, remote_host):
    """Local resource reservation for one `tapa` invocation.

    A `tapa_xo` action forks `--jobs` concurrent Vitis HLS processes and then
    a Vivado for packaging. Bazel's default estimate for an unannotated action
    is one CPU and ~250 MB, so without this the scheduler treats each one as a
    lightweight compile and runs as many as there are cores -- which is how a
    full `bazel test //...` used to exhaust host memory and take the server
    down with it.

    This goes through `execution_requirements` rather than `resource_set`
    because the latter must be a top-level function and so cannot see `jobs`.
    A `cpu:N` tag on the target would replace the whole reservation, memory
    included, so leave those tags off `tapa_xo` targets.
    """
    reqs = {
        "resources:cpu:{}".format(jobs): "",
        "resources:memory:{}".format(max(jobs * _HLS_JOB_MB, _PACK_MB)): "",
    }
    if remote_host:
        # Vendor tools run on the remote host, so only the ssh client is local.
        reqs = {"requires-network": "1"}
    return reqs

def _with_tool_constraints(target, kwargs):
    kwargs = dict(kwargs)
    constraints = list(kwargs.get("target_compatible_with", []))

    # HLS fixtures impersonate the HLS step's output, so synth invokes no
    # vendor tool; for the HLS target, pack (a pure zip writer) needs none
    # either. The .xo packager remains the only fixture-path vendor gate.
    fixtures = bool(kwargs.get("hls_fixtures"))
    needs_hls_runner = not fixtures and not VITIS_HLS_RUNNER_AVAILABLE
    needs_vivado = target in ["xilinx-vitis", "xilinx-slash"] and not VIVADO_AVAILABLE
    if (needs_hls_runner or needs_vivado) and "@platforms//:incompatible" not in constraints:
        constraints.append("@platforms//:incompatible")
    kwargs["target_compatible_with"] = constraints
    return kwargs

def _staged_tapa_command(source, destination, arguments, pre_exec = []):
    """Copy one content-addressed stage, then execute TAPA in the next."""
    lines = [
        "set -euo pipefail",
        "mkdir -p {}".format(sh_quote(destination)),
        "cp -aL {} {}".format(sh_quote(source + "/."), sh_quote(destination)),
        "chmod -R u+w {}".format(sh_quote(destination)),
    ]
    lines += pre_exec
    lines.append("{}".format(" ".join([sh_quote(arg) for arg in arguments])))
    return "\n".join(lines)

def _stage_hls_fixtures(fixture_files, destination):
    """Stage HLS-output fixtures as an mtime-fresh `<work_dir>/hls` tree.

    Each fixture must live under a directory named `hls-fixtures` laid out
    as `<task>/{verilog,report}/<file>`, mirroring the harvested HLS tree.
    Freshness is deterministic: the analyzed `rewritten/` tree (the
    mtime-skip freshness deps) is stamped to a fixed past time and the
    fixtures to now, so the skip never falls through to a real HLS run
    regardless of sandbox clock granularity or cache-materialization
    timestamps.
    """
    lines = [
        "find {} -type f -exec touch -t 200109090146.40 {{}} +".format(
            sh_quote(destination + "/rewritten"),
        ),
    ]
    for f in fixture_files:
        marker = "/hls-fixtures/"
        idx = f.path.find(marker)
        if idx < 0:
            fail("hls_fixtures entry {} must live under a directory named 'hls-fixtures'".format(f.path))
        rel = f.path[idx + len(marker):]
        parts = rel.split("/")
        if len(parts) != 3 or parts[1] not in ["verilog", "report"]:
            fail("hls_fixtures entry {} must be laid out as hls-fixtures/<task>/verilog|report/<file>".format(f.path))
        dest_dir = destination + "/hls/" + parts[0] + "/" + parts[1]
        lines.append("mkdir -p {}".format(sh_quote(dest_dir)))
        lines.append("cp {} {}".format(sh_quote(f.path), sh_quote(dest_dir + "/" + parts[2])))
    lines.append("find {} -type f -exec touch {{}} +".format(sh_quote(destination + "/hls")))
    return lines

def _stage_pack_inputs(source, destination):
    """Keep only artifacts pack reads out of the synth work directory."""
    lines = [
        "mkdir -p {}".format(sh_quote(destination)),
        "cp -aL {} {}".format(sh_quote(source + "/rtl"), sh_quote(destination)),
        "cp -aL {} {}".format(sh_quote(source + "/hls"), sh_quote(destination)),
    ]
    for name in ["tapa.json", "report.json", "report.yaml", "templates_info.json", "template"]:
        path = sh_quote(source + "/" + name)
        lines.append("if [ -e {path} ]; then cp -aL {path} {dest}; fi".format(
            path = path,
            dest = sh_quote(destination),
        ))
    aie = sh_quote(source + "/aie")
    lines.append("if [ -d {aie} ]; then cp -aL {aie} {dest}; fi".format(
        aie = aie,
        dest = sh_quote(destination),
    ))
    return "\n".join(lines)

def _stage_aie_sidecars(output_file, destination):
    """Expose any sidecars pack wrote beside the XO as a declared directory."""
    stem = output_file.basename[:-3]
    lines = ["mkdir -p {}".format(sh_quote(destination.path))]
    for extension in ["libadf.a", "aie.cfg"]:
        source = output_file.dirname + "/" + stem + "." + extension
        lines.append("if [ -f {src} ]; then cp -aL {src} {dest}/; fi".format(
            src = sh_quote(source),
            dest = sh_quote(destination.path),
        ))
    return "\n".join(lines)

def _tapa_xo_impl(ctx):
    if ctx.attr.jobs < 1:
        fail("jobs must be >= 1, got {}".format(ctx.attr.jobs))
    if ctx.files.hls_fixtures and ctx.attr.enable_synth_util:
        fail("hls_fixtures is incompatible with enable_synth_util: post-synth utilization runs Vivado, which the fixture path exists to avoid")
    tapa_cli = ctx.executable.tapa_cli
    srcs = ctx.files.srcs
    top_name = ctx.attr.top_name
    analysis_dir = ctx.actions.declare_directory(ctx.attr.name + ".analysis.tapa")
    synth_dir = ctx.actions.declare_directory(ctx.attr.name + ".synth.tapa")
    pack_input_dir = ctx.actions.declare_directory(ctx.attr.name + ".pack.tapa")
    work_dir = ctx.actions.declare_directory(ctx.attr.name + ".tapa")

    output_file = ctx.outputs.output_file
    if output_file == None and ctx.attr.target == "xilinx-vitis":
        output_file = ctx.actions.declare_file(ctx.attr.name + ".xo")
    if output_file == None and ctx.attr.target in ["xilinx-hls", "xilinx-slash"]:
        output_file = ctx.actions.declare_file(ctx.attr.name + ".zip")
    if ctx.attr.target not in ["xilinx-vitis", "xilinx-hls", "xilinx-slash"]:
        fail("Unsupported target: {}".format(ctx.attr.target))
    aie_output_dir = None
    if ctx.attr.target == "xilinx-vitis":
        aie_output_dir = ctx.actions.declare_directory(ctx.attr.name + ".aie-sidecars")

    remote_host = _remote_host_flag()
    remote_args = _remote_tapa_args(ctx)

    analyze_cmd = [tapa_cli.path, "--work-dir", analysis_dir.path] + remote_args + ["analyze"]
    for src in srcs:
        analyze_cmd.extend(["--input", src.path])
    analyze_cmd.extend(["--top", top_name])
    if ctx.file.tapacc:
        analyze_cmd.extend(["--tapacc", ctx.file.tapacc])
    if ctx.attr.cflags:
        analyze_cmd.extend(["--cflags", ctx.attr.cflags])
    for include in ctx.files.include:
        analyze_cmd.extend(["--cflags", "-I" + include.path])
    analyze_cmd.extend(["--target", ctx.attr.target])
    if ctx.attr.flatten_hierarchy:
        analyze_cmd.extend(["--flatten-hierarchy"])

    analyze_inputs = srcs + ctx.files.hdrs
    if ctx.file.ssh_key:
        analyze_inputs.append(ctx.file.ssh_key)
    ctx.actions.run(
        outputs = [analysis_dir],
        inputs = analyze_inputs,
        tools = [tapa_cli, ctx.executable.vitis_hls_env],
        executable = ctx.executable.vitis_hls_env,
        arguments = analyze_cmd,
        mnemonic = "TapaAnalyze",
        use_default_shell_env = True,
        execution_requirements = {"requires-network": "1"} if remote_host else {},
    )

    synth_cmd = [tapa_cli.path, "--work-dir", synth_dir.path] + remote_args + [
        "synth",
        "--override-report-schema-version",
        "redacted",
        "--jobs",
        str(ctx.attr.jobs),
    ]
    if ctx.attr.platform_name:
        synth_cmd.extend(["--platform", ctx.attr.platform_name])
    if ctx.attr.clock_period:
        synth_cmd.extend(["--clock-period", ctx.attr.clock_period])
    if ctx.attr.part_num:
        synth_cmd.extend(["--part-num", ctx.attr.part_num])
    if not ctx.attr.platform_name and not ctx.attr.clock_period and not ctx.attr.part_num:
        synth_cmd.extend(["--part-num", XILINX_PART_NUM, "--clock-period", "3.33"])
    if ctx.attr.other_hls_configs:
        synth_cmd.extend(["--other-hls-configs", ctx.attr.other_hls_configs])
    if ctx.attr.enable_synth_util:
        synth_cmd.extend(["--enable-synth-util"])
    if ctx.files.hls_fixtures:
        synth_cmd.extend(["--skip-hls-based-on-mtime", "--require-hls-cache"])

    synth_inputs = [analysis_dir] + ctx.files.hls_fixtures
    if ctx.file.ssh_key:
        synth_inputs.append(ctx.file.ssh_key)
    ctx.actions.run_shell(
        outputs = [synth_dir, pack_input_dir],
        inputs = synth_inputs,
        tools = [tapa_cli, ctx.executable.vitis_hls_env],
        command = _staged_tapa_command(
            analysis_dir.path,
            synth_dir.path,
            [ctx.executable.vitis_hls_env.path] + synth_cmd,
            pre_exec = _stage_hls_fixtures(ctx.files.hls_fixtures, synth_dir.path) if ctx.files.hls_fixtures else [],
        ) + "\n" + _stage_pack_inputs(synth_dir.path, pack_input_dir.path),
        mnemonic = "TapaSynth",
        use_default_shell_env = True,
        # Fixtures spawn no vendor processes, so no vendor reservation.
        execution_requirements = {} if ctx.files.hls_fixtures else _vendor_exec_requirements(ctx.attr.jobs, remote_host),
    )

    pack_cmd = [tapa_cli.path, "--work-dir", work_dir.path] + remote_args + [
        "pack",
        "--output",
        output_file.path,
    ]
    for rtl_file in ctx.files.custom_rtl_files:
        pack_cmd.extend(["--custom-rtl", rtl_file.path])

    pack_inputs = [pack_input_dir] + ctx.files.custom_rtl_files
    if ctx.file.ssh_key:
        pack_inputs.append(ctx.file.ssh_key)
    pack_outputs = [output_file, work_dir]
    sidecar_outputs = []
    if aie_output_dir:
        pack_outputs.append(aie_output_dir)
        sidecar_outputs.append(aie_output_dir)
    ctx.actions.run_shell(
        outputs = pack_outputs,
        inputs = pack_inputs,
        tools = [tapa_cli, ctx.executable.vitis_hls_env],
        command = _staged_tapa_command(
            pack_input_dir.path,
            work_dir.path,
            [ctx.executable.vitis_hls_env.path] + pack_cmd,
        ) + ("\n" + _stage_aie_sidecars(output_file, aie_output_dir) if aie_output_dir else ""),
        mnemonic = "TapaPack",
        use_default_shell_env = True,
        execution_requirements = _vendor_exec_requirements(ctx.attr.jobs, remote_host),
    )

    return [
        DefaultInfo(files = depset([output_file])),
        OutputGroupInfo(aie_sidecars = depset(sidecar_outputs)),
    ]

def _tapa_reuse_work_dir_xo_impl(ctx):
    tapa_cli = ctx.executable.tapa_cli
    srcs = ctx.files.srcs
    top_name = ctx.attr.top_name
    output_file = ctx.actions.declare_file(ctx.attr.name + ".xo")
    work_dir = ctx.actions.declare_directory(ctx.attr.name + ".tapa")

    tapa_prefix = [tapa_cli.path, "--work-dir", work_dir.path]
    remote_host = _remote_host_flag()
    tapa_prefix.extend(_remote_tapa_args(ctx))

    include_flags = []
    for inc in ctx.files.include:
        include_flags.extend(["--cflags", "-I" + inc.path])

    env_path = ctx.executable.vitis_hls_env.path
    prefix = " ".join([env_path] + tapa_prefix)
    includes = " ".join(include_flags)
    input_flags = " ".join(["--input " + src.path for src in srcs])
    part_num = ctx.attr.part_num
    clock_period = ctx.attr.clock_period

    # `tapa synth` defaults `--jobs` to the host's available parallelism, which
    # would fork that many Vitis HLS processes out of a single Bazel action.
    # Pin it so the fan-out matches what `_vendor_exec_requirements` reserves.
    script = """
set -ex
{prefix} analyze {includes} {input_flags} --top {top} --target xilinx-vitis
{prefix} synth --jobs {jobs} --part-num {part} --clock-period {clock} --override-report-schema-version=redacted
{prefix} synth --jobs {jobs} --part-num {part} --clock-period {clock} --skip-hls-based-on-mtime --override-report-schema-version=redacted
{prefix} pack --output {output}
""".format(
        prefix = prefix,
        includes = includes,
        input_flags = input_flags,
        top = top_name,
        jobs = ctx.attr.jobs,
        part = part_num,
        clock = clock_period,
        output = output_file.path,
    )

    inputs = srcs + ctx.files.hdrs
    if ctx.file.ssh_key:
        inputs.append(ctx.file.ssh_key)
    ctx.actions.run_shell(
        outputs = [output_file, work_dir],
        inputs = inputs,
        tools = [tapa_cli, ctx.executable.vitis_hls_env],
        command = script,
        execution_requirements = _vendor_exec_requirements(ctx.attr.jobs, remote_host),
    )

    return [DefaultInfo(files = depset([output_file]))]

_tapa_reuse_work_dir_xo = rule(
    implementation = _tapa_reuse_work_dir_xo_impl,
    attrs = {
        "srcs": attr.label_list(allow_files = [".cpp"], mandatory = True),
        "hdrs": attr.label_list(allow_files = True),
        "include": attr.label_list(allow_files = True),
        "top_name": attr.string(mandatory = True),
        "part_num": attr.string(default = XILINX_PART_NUM),
        "clock_period": attr.string(default = "3.33"),
        "jobs": attr.int(
            default = 1,
            doc = "Parallel HLS jobs (must be >= 1). The action reserves " +
                  "CPU and memory to match; do not add a cpu:N tag, which " +
                  "would override that.",
        ),
        "tapa_cli": attr.label(
            cfg = "exec",
            default = Label("//tapa-core:tapa"),
            executable = True,
        ),
        "ssh_key": attr.label(
            allow_single_file = True,
            default = Label("@ssh_key//:key") if REMOTE_KEY_FILE else None,
        ),
        "vitis_hls_env": attr.label(
            cfg = "exec",
            default = Label("//bazel:vitis_hls_env"),
            executable = True,
        ),
    },
)

_tapa_xo = rule(
    implementation = _tapa_xo_impl,
    attrs = {
        "srcs": attr.label_list(allow_files = [".cpp"], mandatory = True),
        "hdrs": attr.label_list(allow_files = True),
        "include": attr.label_list(allow_files = True),
        "top_name": attr.string(mandatory = True),
        "custom_rtl_files": attr.label_list(allow_files = True),
        "platform_name": attr.string(),
        "output_file": attr.output(),
        "tapa_cli": attr.label(
            cfg = "exec",
            default = Label("//tapa-core:tapa"),
            executable = True,
        ),
        "tapacc": attr.label(allow_single_file = True),
        "cflags": attr.string(),
        "target": attr.string(
            default = "xilinx-vitis",
            doc = "The target platform for the synthesis. Default is 'xilinx-vitis'.",
        ),
        "clock_period": attr.string(),
        "part_num": attr.string(),
        "connectivity": attr.label(
            allow_single_file = True,
            doc = "Optional memory-connectivity .ini retained as an input " +
                  "for downstream link rules; not forwarded to pack.",
        ),
        "jobs": attr.int(
            default = 1,
            doc = "Parallel HLS jobs (must be >= 1). The action reserves " +
                  "CPU and memory to match; do not add a cpu:N tag, which " +
                  "would override that.",
        ),
        "other_hls_configs": attr.string(
            doc = "Extra Tcl appended verbatim to every generated HLS script.",
        ),
        "enable_synth_util": attr.bool(),
        "flatten_hierarchy": attr.bool(),
        "hls_fixtures": attr.label_list(
            allow_files = True,
            doc = "HLS-output fixtures staged as the <work_dir>/hls tree " +
                  "before `tapa synth --skip-hls-based-on-mtime`, replacing " +
                  "every Vitis HLS run. Files must live under a directory " +
                  "named 'hls-fixtures' laid out as <task>/verilog/<file> " +
                  "and <task>/report/<task>_csynth.xml, mirroring the " +
                  "harvested HLS tree. With the 'xilinx-hls' target the " +
                  "whole analyze-synth-pack chain then runs without Xilinx " +
                  "tools; 'xilinx-vitis' still needs Vivado for packaging.",
        ),
        "ssh_key": attr.label(
            allow_single_file = True,
            default = Label("@ssh_key//:key") if REMOTE_KEY_FILE else None,
        ),
        "vitis_hls_env": attr.label(
            cfg = "exec",
            default = Label("//bazel:vitis_hls_env"),
            executable = True,
        ),
    },
)

def _tapa_slash_link_impl(ctx):
    if ctx.attr.jobs < 1:
        fail("jobs must be >= 1, got {}".format(ctx.attr.jobs))
    if not ctx.attr.output_name.endswith(".vbin"):
        fail("output_name must end in .vbin")

    tapa_cli = ctx.executable.tapa_cli
    package = ctx.file.package
    connectivity = ctx.file.connectivity
    work_dir = ctx.actions.declare_directory(ctx.attr.name + ".tapa")
    vbin = ctx.actions.declare_file(ctx.attr.output_name)
    metrics = ctx.actions.declare_file(ctx.attr.name + "/link-metrics.json")
    remote_host = _remote_host_flag()
    command = [tapa_cli.path, "--work-dir", work_dir.path] + _remote_tapa_args(ctx) + [
        "link",
        "--package",
        package.path,
        "--connectivity",
        connectivity.path,
        "--output",
        vbin.path,
        "--jobs",
        str(ctx.attr.jobs),
    ]
    if ctx.attr.sim:
        command.append("--sim")

    inputs = [package, connectivity]
    if ctx.file.ssh_key:
        inputs.append(ctx.file.ssh_key)
    ctx.actions.run_shell(
        outputs = [work_dir, vbin, metrics],
        inputs = inputs,
        tools = [tapa_cli, ctx.executable.vitis_hls_env],
        command = "set -euo pipefail\n{}\nmkdir -p {}\ncp {} {}".format(
            " ".join([sh_quote(arg) for arg in [ctx.executable.vitis_hls_env.path] + command]),
            sh_quote(metrics.dirname),
            sh_quote(work_dir.path + "/link/link-metrics.json"),
            sh_quote(metrics.path),
        ),
        mnemonic = "TapaSlashLink",
        use_default_shell_env = True,
        execution_requirements = {"requires-network": "1"} if remote_host else {
            "resources:cpu:{}".format(ctx.attr.jobs): "",
            "resources:memory:{}".format(8192 if ctx.attr.sim else 24576): "",
        },
    )
    return [
        DefaultInfo(files = depset([vbin])),
        OutputGroupInfo(link_metrics = depset([metrics])),
    ]

_tapa_slash_link = rule(
    implementation = _tapa_slash_link_impl,
    attrs = {
        "package": attr.label(allow_single_file = True, mandatory = True),
        "connectivity": attr.label(allow_single_file = True, mandatory = True),
        "output_name": attr.string(mandatory = True),
        "sim": attr.bool(),
        "jobs": attr.int(default = 1),
        "tapa_cli": attr.label(
            cfg = "exec",
            default = Label("//tapa-core:tapa"),
            executable = True,
        ),
        "ssh_key": attr.label(
            allow_single_file = True,
            default = Label("@ssh_key//:key") if REMOTE_KEY_FILE else None,
        ),
        "vitis_hls_env": attr.label(
            cfg = "exec",
            default = Label("//bazel:vitis_hls_env"),
            executable = True,
        ),
    },
)

def tapa_slash_link(name, **kwargs):
    """Run `tapa link` and expose its VBIN and link-metrics.json outputs.

    Args:
      name: Target name.
      **kwargs: Attributes forwarded to the underlying rule.
    """
    constraints = list(kwargs.get("target_compatible_with", []))
    if not VIVADO_AVAILABLE and "@platforms//:incompatible" not in constraints:
        constraints.append("@platforms//:incompatible")
    kwargs["target_compatible_with"] = constraints
    _tapa_slash_link(name = name, **kwargs)

def tapa_reuse_work_dir_xo(name, **kwargs):
    """Build the work-dir reuse fixture when XO tools are available.

    Args:
      name: Target name.
      **kwargs: Attributes forwarded to the underlying rule.
    """
    _tapa_reuse_work_dir_xo(
        name = name,
        **_with_tool_constraints("xilinx-vitis", kwargs)
    )

def tapa_xo(name, **kwargs):
    """Analyze, synthesize, and package a kernel with staged cache inputs.

    Args:
      name: Target name.
      **kwargs: Attributes forwarded to the underlying rule.
    """
    _tapa_xo(
        name = name,
        **_with_tool_constraints(kwargs.get("target", "xilinx-vitis"), kwargs)
    )
