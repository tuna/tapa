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

def _tapa_xo_impl(ctx):
    if ctx.attr.jobs < 1:
        fail("jobs must be >= 1, got {}".format(ctx.attr.jobs))
    tapa_cli = ctx.executable.tapa_cli
    src = ctx.file.src
    top_name = ctx.attr.top_name
    work_dir = ctx.actions.declare_directory(ctx.attr.name + ".tapa")

    output_file = ctx.outputs.output_file
    if output_file == None and ctx.attr.target == "xilinx-vitis":
        output_file = ctx.actions.declare_file(ctx.attr.name + ".xo")
    if output_file == None and ctx.attr.target == "xilinx-hls":
        output_file = ctx.actions.declare_file(ctx.attr.name + ".zip")

    if ctx.attr.target not in ["xilinx-vitis", "xilinx-hls"]:
        fail("Unsupported target: {}".format(ctx.attr.target))

    outputs = [work_dir]

    tapa_cmd = [tapa_cli.path, "--work-dir", work_dir.path]

    remote_host = _remote_host_flag()
    if remote_host:
        tapa_cmd.extend(["--remote-host", remote_host])
        if ctx.file.ssh_key:
            tapa_cmd.extend(["--remote-key-file", ctx.file.ssh_key.path])
        xilinx_settings = _remote_xilinx_settings()
        if xilinx_settings:
            tapa_cmd.extend(["--remote-xilinx-settings", xilinx_settings])
        if REMOTE_SSH_CONTROL_DIR:
            tapa_cmd.extend(["--remote-ssh-control-dir", REMOTE_SSH_CONTROL_DIR])
        if REMOTE_SSH_CONTROL_PERSIST:
            tapa_cmd.extend(["--remote-ssh-control-persist", REMOTE_SSH_CONTROL_PERSIST])

    tapa_cmd.extend(["analyze", "--input", src.path, "--top", top_name])

    if ctx.file.tapacc:
        tapa_cmd.extend(["--tapacc", ctx.file.tapacc])
    if ctx.file.tapa_cpp:
        tapa_cmd.extend(["--tapa-cpp", ctx.file.tapa_cpp])

    if ctx.attr.cflags:
        tapa_cmd.extend(["--cflags", ctx.attr.cflags])

    if ctx.files.include:
        for include in ctx.files.include:
            tapa_cmd.extend(["--cflags", "-I" + include.path])

    tapa_cmd.extend(["--target", ctx.attr.target])

    if ctx.attr.flatten_hierarchy:
        tapa_cmd.extend(["--flatten-hierarchy"])

    tapa_cmd.extend(["synth"])

    tapa_cmd.extend(["--override-report-schema-version", "redacted"])

    tapa_cmd.extend(["--jobs", str(ctx.attr.jobs)])

    if ctx.attr.platform_name:
        tapa_cmd.extend(["--platform", ctx.attr.platform_name])
    if ctx.attr.clock_period:
        tapa_cmd.extend(["--clock-period", ctx.attr.clock_period])
    if ctx.attr.part_num:
        tapa_cmd.extend(["--part-num", ctx.attr.part_num])

    if not ctx.attr.platform_name and not ctx.attr.clock_period and not ctx.attr.part_num:
        tapa_cmd.extend(["--part-num", XILINX_PART_NUM])
        tapa_cmd.extend(["--clock-period", "3.33"])

    if ctx.attr.enable_synth_util:
        tapa_cmd.extend(["--enable-synth-util"])

    if output_file != None:
        tapa_cmd.extend(["pack", "--output", output_file.path])
        if ctx.file.connectivity:
            tapa_cmd.extend(["--connectivity", ctx.file.connectivity.path])
        outputs = [output_file] + outputs

    for rtl_file in ctx.files.custom_rtl_files:
        tapa_cmd.extend(["--custom-rtl", rtl_file.path])

    inputs = [src] + ctx.files.hdrs + ctx.files.custom_rtl_files
    if ctx.file.connectivity:
        inputs.append(ctx.file.connectivity)
    if ctx.file.ssh_key:
        inputs.append(ctx.file.ssh_key)
    ctx.actions.run(
        outputs = outputs,
        inputs = inputs,
        tools = [tapa_cli, ctx.executable.vitis_hls_env],
        executable = ctx.executable.vitis_hls_env,
        arguments = tapa_cmd,
        execution_requirements = _vendor_exec_requirements(ctx.attr.jobs, remote_host),
    )

    return [DefaultInfo(files = depset([output_file or work_dir]))]

def _tapa_reuse_work_dir_xo_impl(ctx):
    tapa_cli = ctx.executable.tapa_cli
    src = ctx.file.src
    top_name = ctx.attr.top_name
    output_file = ctx.actions.declare_file(ctx.attr.name + ".xo")
    work_dir = ctx.actions.declare_directory(ctx.attr.name + ".tapa")

    tapa_prefix = [tapa_cli.path, "--work-dir", work_dir.path]
    remote_host = _remote_host_flag()
    if remote_host:
        tapa_prefix.extend(["--remote-host", remote_host])
        if ctx.file.ssh_key:
            tapa_prefix.extend(["--remote-key-file", ctx.file.ssh_key.path])
        xilinx_settings = _remote_xilinx_settings()
        if xilinx_settings:
            tapa_prefix.extend(["--remote-xilinx-settings", xilinx_settings])
        if REMOTE_SSH_CONTROL_DIR:
            tapa_prefix.extend(["--remote-ssh-control-dir", REMOTE_SSH_CONTROL_DIR])
        if REMOTE_SSH_CONTROL_PERSIST:
            tapa_prefix.extend(["--remote-ssh-control-persist", REMOTE_SSH_CONTROL_PERSIST])

    include_flags = []
    for inc in ctx.files.include:
        include_flags.extend(["--cflags", "-I" + inc.path])

    env_path = ctx.executable.vitis_hls_env.path
    prefix = " ".join([env_path] + tapa_prefix)
    includes = " ".join(include_flags)
    part_num = ctx.attr.part_num
    clock_period = ctx.attr.clock_period

    # `tapa synth` defaults `--jobs` to the host's available parallelism, which
    # would fork that many Vitis HLS processes out of a single Bazel action.
    # Pin it so the fan-out matches what `_vendor_exec_requirements` reserves.
    script = """
set -ex
{prefix} analyze {includes} --input {src} --top {top} --target xilinx-vitis
{prefix} synth --jobs {jobs} --part-num {part} --clock-period {clock} --override-report-schema-version=redacted
{prefix} synth --jobs {jobs} --part-num {part} --clock-period {clock} --skip-hls-based-on-mtime --override-report-schema-version=redacted
{prefix} pack --output {output}
""".format(
        prefix = prefix,
        includes = includes,
        src = src.path,
        top = top_name,
        jobs = ctx.attr.jobs,
        part = part_num,
        clock = clock_period,
        output = output_file.path,
    )

    inputs = [src] + ctx.files.hdrs
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

tapa_reuse_work_dir_xo = rule(
    implementation = _tapa_reuse_work_dir_xo_impl,
    attrs = {
        "src": attr.label(allow_single_file = True, mandatory = True),
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

tapa_xo = rule(
    implementation = _tapa_xo_impl,
    attrs = {
        "src": attr.label(allow_single_file = True, mandatory = True),
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
        "tapa_cpp": attr.label(allow_single_file = True),
        "cflags": attr.string(),
        "target": attr.string(
            default = "xilinx-vitis",
            doc = "The target platform for the synthesis. Default is 'xilinx-vitis'.",
        ),
        "clock_period": attr.string(),
        "part_num": attr.string(),
        "connectivity": attr.label(
            allow_single_file = True,
            doc = "Optional memory-connectivity .ini forwarded to `pack " +
                  "--connectivity`; consumed when pack emits a v++ link " +
                  "script, accepted and idle for a bare .xo.",
        ),
        "jobs": attr.int(
            default = 1,
            doc = "Parallel HLS jobs (must be >= 1). The action reserves " +
                  "CPU and memory to match; do not add a cpu:N tag, which " +
                  "would override that.",
        ),
        "enable_synth_util": attr.bool(),
        "flatten_hierarchy": attr.bool(),
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
