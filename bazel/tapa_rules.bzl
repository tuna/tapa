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
    "XILINX_TOOL_VERSION",
)

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
        subdir = "/Vitis/" if XILINX_TOOL_VERSION >= "2024.2" else "/Vitis_HLS/"
        return REMOTE_XILINX_TOOL_PATH + subdir + XILINX_TOOL_VERSION + "/settings64.sh"
    return ""

def _tapa_xo_impl(ctx):
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
    if ctx.file.tapa_clang:
        tapa_cmd.extend(["--tapa-clang", ctx.file.tapa_clang])

    if ctx.attr.cflags:
        tapa_cmd.extend(["--cflags", ctx.attr.cflags])

    if ctx.files.include:
        for include in ctx.files.include:
            tapa_cmd.extend(["--cflags", "-I" + include.path])

    tapa_cmd.extend(["--target", ctx.attr.target])

    if ctx.attr.flatten_hierarchy:
        tapa_cmd.extend(["--flatten-hierarchy"])

    tapa_cmd.extend(["floorplan"])

    if ctx.file.floorplan_path:
        tapa_cmd.extend(["--floorplan-path", ctx.file.floorplan_path.path])

    tapa_cmd.extend(["synth"])

    if ctx.file.floorplan_path:
        tapa_cmd.extend(["--floorplan-path", ctx.file.floorplan_path.path])

    if ctx.file.floorplan_config:
        tapa_cmd.extend(["--floorplan-config", ctx.file.floorplan_config.path])

    if ctx.file.device_config:
        tapa_cmd.extend(["--device-config", ctx.file.device_config.path])

    tapa_cmd.extend(["--override-report-schema-version", "redacted"])

    tapa_cmd.extend(["--jobs", "2"])

    if ctx.attr.platform_name:
        tapa_cmd.extend(["--platform", ctx.attr.platform_name])
    if ctx.attr.clock_period:
        tapa_cmd.extend(["--clock-period", ctx.attr.clock_period])
    if ctx.attr.part_num:
        tapa_cmd.extend(["--part-num", ctx.attr.part_num])

    if not ctx.attr.platform_name and not ctx.attr.clock_period and not ctx.attr.part_num:
        tapa_cmd.extend(["--part-num", "xcu250-figd2104-2l-e"])
        tapa_cmd.extend(["--clock-period", "3.33"])

    if ctx.attr.enable_synth_util:
        tapa_cmd.extend(["--enable-synth-util"])
    ab_graph_file = None
    if ctx.attr.gen_ab_graph:
        tapa_cmd.extend(["--gen-ab-graph"])
        ab_graph_file = ctx.actions.declare_file(ctx.attr.name + ".json")

    if ctx.attr.use_graphir:
        tapa_cmd.extend(["--gen-graphir"])

    if output_file != None:
        tapa_cmd.extend(["pack", "--output", output_file.path])
        outputs = [output_file] + outputs
    if ctx.attr.use_graphir:
        tapa_cmd.extend(["--graphir-path", work_dir.path + "/graphir.json"])

    for rtl_file in ctx.files.custom_rtl_files:
        tapa_cmd.extend(["--custom-rtl", rtl_file.path])

    inputs = [src] + ctx.files.hdrs + ctx.files.custom_rtl_files
    if ctx.file.ssh_key:
        inputs.append(ctx.file.ssh_key)
    if ctx.file.floorplan_path:
        inputs.append(ctx.file.floorplan_path)
    if ctx.file.floorplan_config:
        inputs.append(ctx.file.floorplan_config)
    if ctx.file.device_config:
        inputs.append(ctx.file.device_config)
    ctx.actions.run(
        outputs = outputs,
        inputs = inputs,
        tools = [tapa_cli, ctx.executable.vitis_hls_env],
        executable = ctx.executable.vitis_hls_env,
        arguments = tapa_cmd,
        execution_requirements = {"requires-network": "1"} if remote_host else {},
    )

    ab_graph_return = []
    if ab_graph_file:
        ctx.actions.run_shell(
            inputs = [work_dir],
            outputs = [ab_graph_file],
            command = """
            cp {}/ab_graph.json {}
            """.format(work_dir.path, ab_graph_file.path),
        )
        ab_graph_return = [ab_graph_file]

    return [DefaultInfo(files = depset([output_file or work_dir] + ab_graph_return))]

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

    script = """
set -ex
{prefix} analyze {includes} --input {src} --top {top} --target xilinx-vitis
{prefix} synth --part-num {part} --clock-period {clock} --override-report-schema-version=redacted
{prefix} synth --part-num {part} --clock-period {clock} --skip-hls-based-on-mtime --override-report-schema-version=redacted
{prefix} pack --output {output}
""".format(
        prefix = prefix,
        includes = includes,
        src = src.path,
        top = top_name,
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
        execution_requirements = {"requires-network": "1"} if remote_host else {},
    )

    return [DefaultInfo(files = depset([output_file]))]

tapa_reuse_work_dir_xo = rule(
    implementation = _tapa_reuse_work_dir_xo_impl,
    attrs = {
        "src": attr.label(allow_single_file = True, mandatory = True),
        "hdrs": attr.label_list(allow_files = True),
        "include": attr.label_list(allow_files = True),
        "top_name": attr.string(mandatory = True),
        "part_num": attr.string(default = "xcu250-figd2104-2l-e"),
        "clock_period": attr.string(default = "3.33"),
        "tapa_cli": attr.label(
            cfg = "exec",
            default = Label("//tapa"),
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
            default = Label("//tapa"),
            executable = True,
        ),
        "tapacc": attr.label(allow_single_file = True),
        "tapa_clang": attr.label(allow_single_file = True),
        "cflags": attr.string(),
        "target": attr.string(
            default = "xilinx-vitis",
            doc = "The target platform for the synthesis. Default is 'xilinx-vitis'.",
        ),
        "clock_period": attr.string(),
        "part_num": attr.string(),
        "enable_synth_util": attr.bool(),
        "gen_ab_graph": attr.bool(),
        "flatten_hierarchy": attr.bool(),
        "floorplan_path": attr.label(allow_single_file = True),
        "floorplan_config": attr.label(allow_single_file = True),
        "device_config": attr.label(allow_single_file = True),
        "ssh_key": attr.label(
            allow_single_file = True,
            default = Label("@ssh_key//:key") if REMOTE_KEY_FILE else None,
        ),
        "vitis_hls_env": attr.label(
            cfg = "exec",
            default = Label("//bazel:vitis_hls_env"),
            executable = True,
        ),
        "use_graphir": attr.bool(),
    },
)
