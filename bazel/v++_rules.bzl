"""Custom rule to add V++ target to the target list."""

# Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
# All rights reserved. The contributor(s) of this file has/have agreed to the
# RapidStream Contributor License Agreement.

load("//:VARS.bzl", "XILINX_PLATFORM_REPO_PATHS", "XILINX_TOOL_PATH", "XILINX_TOOL_VERSION", "XILINX_XRT_SETUP")

# Define the implementation of vpp target.
def _vpp_xclbin_impl(ctx):
    # Retrieve the inputs and attributes from the rule invocation.
    vpp = ctx.executable.vpp
    xo = ctx.file.xo
    target = ctx.attr.target
    top_name = ctx.attr.top_name
    platform_name = ctx.attr.platform_name
    xclbin = ctx.actions.declare_file(
        ctx.attr.xclbin or "{}.{}.{}.xclbin".format(
            top_name,
            platform_name,
            target,
        ),
    )

    # Start building the command to run v++.
    vpp_cmd = [
        "--link",
        "--output",
        xclbin.path,
        "--kernel",
        top_name,
        "--platform",
        platform_name,
        "--target",
        target,
        "--connectivity.nk",
        "{top_name}:1:{top_name}".format(top_name = top_name),
        xo.path,
    ]

    if target == "hw_emu":
        # Reduce `mt_level` to avoid excessive amount of processes. Run time of
        # `bazel build //tests/apps/vadd:vadd-hw-emu-xclbin` for reference:
        #   mt_level=1: 651s ("off", +67%)
        #   mt_level=2: 483s (+24$)
        #   mt_level=4: 405s (+4%)
        #   mt_level=8: 390s (default)
        vpp_cmd += [
            "--vivado.prop=fileset.sim_1.xsim.compile.xsc.mt_level=2",
            "--vivado.prop=fileset.sim_1.xsim.elaborate.mt_level=2",
            # Xilinx's bundled GCC may not know about multiarch system header
            # paths (e.g., /usr/include/x86_64-linux-gnu for asm/types.h).
            "--vivado.prop=fileset.sim_1.xsim.compile.xsc.more_options={--gcc_compile_options -I/usr/include/x86_64-linux-gnu}",
            # Also pass -B to the RELR compat dir so the bundled ld finds our
            # stripped libs before the RELR-containing system originals.
            # The -B flag is processed before LIBRARY_PATH in GCC's search
            # order, making it immune to elaborate.sh's LIBRARY_PATH changes.
            "--vivado.prop=fileset.sim_1.xsim.elaborate.xsc.more_options={--gcc_compile_options -I/usr/include/x86_64-linux-gnu --gcc_link_options -B/tmp/tapa-compat-relr/}",
        ]

    # Define a custom action to run the synthesized command.
    ctx.actions.run(
        outputs = [xclbin],
        inputs = [xo],
        tools = [vpp],
        executable = vpp,
        arguments = vpp_cmd,
        mnemonic = "VppLink",
        resource_set = _resource_set,
    )

    # Return default information, including the output file.
    return [DefaultInfo(files = depset([xclbin]))]

# Tell bazel v++ consumes a lot of memory. 2GB is a conservative estimation that
# avoids wasting memory.
def _resource_set(_os, _num_inputs):
    return {"memory": 2000}  # MB

# Define the v++ rule.
vpp_xclbin = rule(
    implementation = _vpp_xclbin_impl,
    attrs = {
        "vpp": attr.label(
            cfg = "exec",
            default = Label("//bazel:v++"),
            executable = True,
            doc = "The v++ executable.",
        ),
        "xo": attr.label(
            allow_single_file = True,
            mandatory = True,
            doc = "The source xo file to be linked.",
        ),
        "top_name": attr.string(
            mandatory = True,
            doc = "The top function name of the kernel.",
        ),
        "platform_name": attr.string(
            mandatory = True,
            doc = "The platform name for the kernel.",
        ),
        "target": attr.string(
            mandatory = True,
            doc = "The target to be linked (sw_emu, hw_emu, hw).",
            values = ["sw_emu", "hw_emu", "hw"],
        ),
        "xclbin": attr.string(
            mandatory = False,
            doc = "The output xclbin file name for the kernel.",
        ),
    },
)

def _xilinx_wrapper_impl(ctx):
    output = ctx.actions.declare_file(ctx.attr.name)
    tool_path = "{}/{}/{}".format(
        ctx.attr.tool_path,
        ctx.attr.tool,
        ctx.attr.tool_version,
    )
    lines = [
        "#!/bin/bash",
        "set -e",
        # Xilinx tools use #!/bin/sh with bash-specific syntax internally.
        # On systems where /bin/sh is dash, this causes syntax errors.
        # Work around by creating a temp dir with sh -> bash in PATH.
        '_TAPA_SH_DIR="$(mktemp -d)"',
        'ln -sf "$(command -v bash)" "$_TAPA_SH_DIR/sh"',
        'export PATH="$_TAPA_SH_DIR:$PATH"',
        'trap \'rm -rf "$_TAPA_SH_DIR"\' EXIT',
        "source {}/settings64.sh".format(tool_path),
    ]
    if ctx.attr.xrt:
        lines.append("source {}".format(ctx.attr.xrt_setup))

    # Re-prepend $_TAPA_SH_DIR to PATH after sourcing settings64.sh, which
    # prepends Vivado/Vitis bin dirs. Our wrapper scripts (sh, xsc) must
    # remain first in PATH to be found before the vendor originals.
    lines.append('export PATH="$_TAPA_SH_DIR:$PATH"')
    lines.append("export HOME=/tmp")  # Dump trash generated by Vivado to /tmp

    # Xilinx's bundled GCC may not know about multiarch system header paths.
    # CPLUS_INCLUDE_PATH is picked up by GCC for C++ compilations (e.g., xelab).
    lines.append('export CPLUS_INCLUDE_PATH="/usr/include/x86_64-linux-gnu${CPLUS_INCLUDE_PATH:+:$CPLUS_INCLUDE_PATH}"')
    lines.append('export C_INCLUDE_PATH="/usr/include/x86_64-linux-gnu${C_INCLUDE_PATH:+:$C_INCLUDE_PATH}"')

    # Xilinx's bundled GCC/ld may not know about multiarch library paths.
    # Always add them via LIBRARY_PATH so the bundled ld can find them.
    # LIBRARY_PATH is used by GCC to pass additional -L flags to ld, but
    # after any explicit -L flags — so we avoid explicit -L in Bazel rules
    # to let compat libs (below) take priority when needed.
    lines.append('export LIBRARY_PATH="/usr/lib/x86_64-linux-gnu:/lib/x86_64-linux-gnu${LIBRARY_PATH:+:$LIBRARY_PATH}"')

    # Xilinx's bundled binutils-2.37 cannot link system libraries with RELR
    # relocations (requires >= 2.38). When RELR is detected, create
    # RELR-stripped copies of affected system libraries and linker scripts,
    # and prepend them to LIBRARY_PATH so the bundled ld finds them first.
    lines.append('_TAPA_COMPAT="$(mktemp -d)"')
    lines.append('trap \'rm -rf "$_TAPA_SH_DIR" "$_TAPA_COMPAT"\' EXIT')
    lines.append('if readelf -S /lib/x86_64-linux-gnu/libc.so.6 2>/dev/null | grep -q "\\.relr\\.dyn"; then')
    lines.append("  if command -v objcopy >/dev/null 2>&1; then")

    # Strip RELR from each affected library, but only if it has RELR.
    lines.append("    for _lib in /lib/x86_64-linux-gnu/libc.so.6 /lib64/ld-linux-x86-64.so.2 /lib/x86_64-linux-gnu/libm.so.6 /lib/x86_64-linux-gnu/libmvec.so.1; do")
    lines.append('      if [ -f "$_lib" ] && readelf -S "$_lib" 2>/dev/null | grep -q "\\.relr\\.dyn"; then')
    lines.append('        objcopy --remove-section .relr.dyn "$_lib" "$_TAPA_COMPAT/$(basename $_lib)"')
    lines.append("      fi")
    lines.append("    done")

    # Create libc.so linker script pointing to stripped copies.
    lines.append('    if [ -f "$_TAPA_COMPAT/libc.so.6" ]; then')
    lines.append('      _ld_so="$_TAPA_COMPAT/ld-linux-x86-64.so.2"')
    lines.append('      [ -f "$_ld_so" ] || _ld_so=/lib64/ld-linux-x86-64.so.2')
    lines.append('      printf "OUTPUT_FORMAT(elf64-x86-64)\\nGROUP ( %s/libc.so.6 /usr/lib/x86_64-linux-gnu/libc_nonshared.a AS_NEEDED ( %s ) )\\n" "$_TAPA_COMPAT" "$_ld_so" > "$_TAPA_COMPAT/libc.so"')
    lines.append("    fi")

    # Create libm.so linker script pointing to stripped copies.
    lines.append('    if [ -f "$_TAPA_COMPAT/libm.so.6" ]; then')
    lines.append('      _mvec="$_TAPA_COMPAT/libmvec.so.1"')
    lines.append('      [ -f "$_mvec" ] || _mvec=/lib/x86_64-linux-gnu/libmvec.so.1')
    lines.append('      printf "OUTPUT_FORMAT(elf64-x86-64)\\nGROUP ( %s/libm.so.6 AS_NEEDED ( %s ) )\\n" "$_TAPA_COMPAT" "$_mvec" > "$_TAPA_COMPAT/libm.so"')
    lines.append("    fi")
    lines.append('    export LIBRARY_PATH="$_TAPA_COMPAT:$LIBRARY_PATH"')

    # Also populate the fixed path /tmp/tapa-compat-relr/ referenced by
    # the v++ elaborate xsc.more_options prop (-B/tmp/tapa-compat-relr/).
    # The -B flag makes GCC search this dir before LIBRARY_PATH, which is
    # critical because Vivado-generated elaborate.sh prepends
    # /usr/lib/x86_64-linux-gnu to LIBRARY_PATH (overriding our ordering).
    lines.append("    if [ ! -d /tmp/tapa-compat-relr ]; then")
    lines.append('      cp -a "$_TAPA_COMPAT" /tmp/tapa-compat-relr.tmp.$$')

    # Fix linker scripts: they reference $_TAPA_COMPAT (a tempdir that
    # will be cleaned up). Rewrite them to reference the final path.
    lines.append('      sed -i "s|$_TAPA_COMPAT|/tmp/tapa-compat-relr|g" /tmp/tapa-compat-relr.tmp.$$/libc.so /tmp/tapa-compat-relr.tmp.$$/libm.so 2>/dev/null || true')
    lines.append("      mv /tmp/tapa-compat-relr.tmp.$$ /tmp/tapa-compat-relr 2>/dev/null || rm -rf /tmp/tapa-compat-relr.tmp.$$")
    lines.append("    fi")

    # Install a GCC specs file so the bundled GCC always searches our compat
    # dir first. This is needed because xelab (Vivado simulator elaboration)
    # internally calls the bundled GCC with -B pointing to bundled
    # binutils-2.37, and we cannot intercept xelab's invocation (Vivado's
    # rdi::run_program does not preserve PATH). The specs file adds
    # -B/tmp/tapa-compat-relr/ which GCC processes before other -B flags,
    # making the bundled ld find our RELR-stripped libs first.
    lines.append('    _gcc_specs="$({gcc_dir}/bin/gcc -print-search-dirs 2>/dev/null | sed -n "s/install: //p")/specs"'.format(
        gcc_dir = "{}/Vivado/{}/tps/lnx64/gcc-9.3.0".format(
            ctx.attr.tool_path,
            ctx.attr.tool_version,
        ),
    ))
    lines.append('    if [ -n "$_gcc_specs" ] && [ ! -f "$_gcc_specs" ]; then')
    lines.append('      printf "*self_spec:\\n-B/tmp/tapa-compat-relr/\\n" > "$_gcc_specs" 2>/dev/null || true')
    lines.append("    fi")
    lines.append("  fi")
    lines.append("fi")
    lines.append('export PLATFORM_REPO_PATHS="${{PLATFORM_REPO_PATHS:-{}}}"'.format(
        ctx.attr.platform_repo_paths,
    ))
    if ctx.attr.argv0:
        lines.append('exec {} "$@"'.format(ctx.attr.argv0))
    else:
        lines.append('exec "$@"')
    ctx.actions.write(output, "\n".join(lines), is_executable = True)
    return [DefaultInfo(executable = output)]

# Generates a shell script wrapper that sets up necessary Xilinx environment.
xilinx_wrapper = rule(
    implementation = _xilinx_wrapper_impl,
    executable = True,
    attrs = {
        "tool": attr.string(
            mandatory = True,
            doc = "The Xilinx tool under the tool path, e.g., Vivado, Vitis.",
        ),
        "argv0": attr.string(
            doc = 'Optional "$0" prepended to "$@".',
        ),
        "xrt": attr.bool(
            default = False,
            doc = "If true, also set up XRT environment.",
        ),
        "xrt_setup": attr.string(
            doc = "Path to XRT setup.sh script.",
            default = XILINX_XRT_SETUP,
        ),
        "tool_path": attr.string(
            doc = "The tool path for this target.",
            default = XILINX_TOOL_PATH,
        ),
        "tool_version": attr.string(
            doc = "The tool version for this target.",
            default = XILINX_TOOL_VERSION,
        ),
        "platform_repo_paths": attr.string(
            doc = "Colon-separated platform repository paths.",
            default = XILINX_PLATFORM_REPO_PATHS,
        ),
    },
)
