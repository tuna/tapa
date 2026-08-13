"""Macros generating the standard TAPA kernel test target sets.

The macros in this file stamp out the sh_test/cc_binary/tapa_xo/vpp_xclbin
targets shared by tests/apps and tests/functional packages. Only attributes
that actually vary across those packages are parameterized; everything else
stays literal so the generated targets keep their historical names and
attributes.
"""

load("@rules_cc//cc:defs.bzl", "cc_binary")
load("@rules_shell//shell:sh_test.bzl", "sh_test")
load("@vars//:vars.bzl", "HAS_XRT", "XILINX_HW_EMU_PLATFORM")
load("//bazel:tapa_rules.bzl", "tapa_xo")
load("//bazel:v++_rules.bzl", "vpp_xclbin")

_CARGO_RELEASE_ARTIFACTS = "//fpga-runtime:cargo_release_artifacts"

# Must name the same device as XILINX_PART_NUM, which is what the `.xo` these
# targets link was synthesized for. See the note in VARS.bzl.
_HW_EMU_PLATFORM = XILINX_HW_EMU_PLATFORM

# No vendor include path: host CPU simulation must be self-contained, which
# is what proves the vendor-agnostic migration actually landed.
_HOST_DEPS = [
    "//tapa-lib:tapa-host",
    "@gflags",
]

def tapa_app_test(
        name,
        host_srcs,
        top_name,
        kernel_hdrs = None,
        sim_args = [],
        macos_sim_args = [],
        hw_test_args = [],
        extra_test_data = [],
        hw_test_timeout = "moderate",
        xo_visibility = None):
    """Declares the standard tests/apps target set for a single app.

    Generates sh_test targets `<name>` (software simulation), `<name>-xosim`
    (the `.xo` under TAPA's own testbench), `<name>-hw-emu` (the `.xclbin`
    under XRT hardware emulation), and `<name>-verilator-zipsim`; a
    `<name>-host` cc_binary; `<name>-xo` and `<name>-zip` tapa_xo targets;
    and a `<name>-hw-emu-xclbin` vpp_xclbin target.

    Each simulation target is named for what it simulates. `-xosim` and
    `-hw-emu` were swapped until 2026-08, which read as cosim having been
    dropped: the XRT-gated target wore the `-xosim` name, so a host without
    XRT skipped what looked like the `.xo` tier.

    Args:
        name: App name; also the basename of the `<name>.cpp` kernel source.
        host_srcs: `srcs` of the `<name>-host` cc_binary, typically
            `glob(["*.cpp", "*.h"])`.
        top_name: Kernel top function name.
        kernel_hdrs: `hdrs` of both tapa_xo targets; defaults to
            `["<name>.h"]`.
        sim_args: Extra args of the `<name>` sh_test on all platforms.
        macos_sim_args: Extra args of the `<name>` sh_test on macOS only.
        hw_test_args: Extra trailing args of the `-xosim`, `-hw-emu`, and
            `-verilator-zipsim` sh_tests.
        extra_test_data: Extra data prepended to the sh_tests' `data`.
        hw_test_timeout: Timeout of the hardware simulation sh_tests.
        xo_visibility: Optional visibility applied to both tapa_xo targets.
    """
    host_label = ":%s-host" % name
    xo_label = ":%s-xo" % name
    zip_label = ":%s-zip" % name
    xclbin_label = ":%s-hw-emu-xclbin" % name
    if kernel_hdrs == None:
        kernel_hdrs = ["%s.h" % name]

    sim_test_args = ["$(location %s-host)" % name] + sim_args
    if macos_sim_args:
        sim_test_args = sim_test_args + select({
            "//bazel:is_macos": macos_sim_args,
            "//conditions:default": [],
        })

    xo_kwargs = {}
    if xo_visibility != None:
        xo_kwargs["visibility"] = xo_visibility

    sh_test(
        name = name,
        size = "medium",
        srcs = select({
            "//bazel:is_macos": ["//bazel:sim_env.sh"],
            "//conditions:default": ["//bazel:v++_env.sh"],
        }),
        args = sim_test_args,
        data = extra_test_data + [host_label],
        env = {"TAPA_CONCURRENCY": "2"},
        tags = ["cpu:2"],
    )

    # Simulates the `.xo` with TAPA's own testbench through frt-cosim, the
    # same thing `tapa_functional_test` calls `-xosim`. No XRT, no platform
    # shell: the kernel RTL only.
    sh_test(
        name = "%s-xosim" % name,
        size = "enormous",
        timeout = hw_test_timeout,
        srcs = ["//bazel:v++_env.sh"],
        args = [
            "$(location %s-host)" % name,
            "--bitstream=$(location %s-xo)" % name,
        ] + hw_test_args,
        data = extra_test_data + [
            host_label,
            xo_label,
            _CARGO_RELEASE_ARTIFACTS,
        ],
        tags = ["cpu:2"],
    )

    # Vitis hardware emulation: the linked `.xclbin` under XRT, kernel plus
    # platform shell. Needs XRT and a platform v++ will still accept, so it
    # is gated -- see the HAS_XRT note in VARS.local.bzl.
    sh_test(
        name = "%s-hw-emu" % name,
        size = "enormous",
        timeout = hw_test_timeout,
        srcs = ["//bazel:xrt_env.sh"],
        args = [
            "$(location %s-host)" % name,
            "--bitstream=$(location %s-hw-emu-xclbin)" % name,
        ] + hw_test_args,
        data = extra_test_data + [
            host_label,
            xclbin_label,
        ],
        tags = ["cpu:2"],
        target_compatible_with = [] if HAS_XRT else ["@platforms//:incompatible"],
    )

    sh_test(
        name = "%s-verilator-zipsim" % name,
        size = "enormous",
        timeout = hw_test_timeout,
        srcs = ["//bazel:sim_env.sh"],
        args = [
            "$(location %s-host)" % name,
            "--bitstream=$(location %s-zip)" % name,
            "--cosim_simulator=verilator",
        ] + hw_test_args,
        data = extra_test_data + [
            host_label,
            zip_label,
            _CARGO_RELEASE_ARTIFACTS,
            "@verilator//:verilator_executable",
        ],
        env = {"VERILATOR_BIN": "$(rootpath @verilator//:verilator_executable)"},
        tags = ["cpu:2"],
    )

    cc_binary(
        name = "%s-host" % name,
        srcs = host_srcs,
        visibility = ["//tests/functional:__subpackages__"],
        deps = _HOST_DEPS,
    )

    tapa_xo(
        name = "%s-xo" % name,
        src = "%s.cpp" % name,
        hdrs = kernel_hdrs,
        include = ["."],
        top_name = top_name,
        **xo_kwargs
    )

    tapa_xo(
        name = "%s-zip" % name,
        src = "%s.cpp" % name,
        hdrs = kernel_hdrs,
        include = ["."],
        target = "xilinx-hls",
        top_name = top_name,
        **xo_kwargs
    )

    vpp_xclbin(
        name = "%s-hw-emu-xclbin" % name,
        platform_name = _HW_EMU_PLATFORM,
        target = "hw_emu",
        target_compatible_with = [] if HAS_XRT else ["@platforms//:incompatible"],
        top_name = top_name,
        xo = xo_label,
    )

def tapa_functional_test(
        name,
        host_srcs,
        top_name = "VecAdd",
        kernel_hdrs = None,
        sim_args = [],
        macos_sim_args = ["1000"],
        sim_size = "medium",
        xosim_size = "enormous",
        xosim_timeout = "moderate",
        xosim_tags = ["cpu:2"]):
    """Declares the standard tests/functional target set for a single test.

    Generates `<name>` and `<name>-xosim` sh_tests, a `<name>-host` cc_binary,
    and a `<name>-xo` tapa_xo target, matching the historical per-test BUILD
    stanzas. The kernel under test is the shared `vadd.cpp` sample.

    Args:
        name: Test name.
        host_srcs: `srcs` of the `<name>-host` cc_binary, typically
            `glob(["*.cpp"])`.
        top_name: Kernel top function name.
        kernel_hdrs: `hdrs` of the tapa_xo target; omitted when None.
        sim_args: Extra args of the `<name>` sh_test on all platforms.
        macos_sim_args: Extra args of the `<name>` sh_test on macOS only.
        sim_size: Size of the `<name>` sh_test.
        xosim_size: Size of the `<name>-xosim` sh_test; omitted when None.
        xosim_timeout: Timeout of the `<name>-xosim` sh_test; omitted when
            None.
        xosim_tags: Tags of the `<name>-xosim` sh_test; omitted when None.
    """
    sim_test_args = ["$(location %s-host)" % name] + sim_args
    if macos_sim_args:
        sim_test_args = sim_test_args + select({
            "//bazel:is_macos": macos_sim_args,
            "//conditions:default": [],
        })

    xosim_kwargs = {}
    xosim_kwargs["size"] = xosim_size
    if xosim_timeout != None:
        xosim_kwargs["timeout"] = xosim_timeout
    if xosim_tags != None:
        xosim_kwargs["tags"] = xosim_tags

    kernel_kwargs = {}
    if kernel_hdrs != None:
        kernel_kwargs["hdrs"] = kernel_hdrs

    sh_test(
        name = name,
        size = sim_size,
        srcs = select({
            "//bazel:is_macos": ["//bazel:sim_env.sh"],
            "//conditions:default": ["//bazel:v++_env.sh"],
        }),
        args = sim_test_args,
        data = [":%s-host" % name],
        env = {"TAPA_CONCURRENCY": "2"},
        tags = ["cpu:2"],
    )

    sh_test(
        name = "%s-xosim" % name,
        srcs = ["//bazel:v++_env.sh"],
        args = [
            "$(location %s-host)" % name,
            "--bitstream=$(location %s-xo)" % name,
            "1000",
        ],
        data = [
            ":%s-host" % name,
            ":%s-xo" % name,
            _CARGO_RELEASE_ARTIFACTS,
        ],
        **xosim_kwargs
    )

    cc_binary(
        name = "%s-host" % name,
        srcs = host_srcs,
        deps = _HOST_DEPS,
    )

    tapa_xo(
        name = "%s-xo" % name,
        src = "vadd.cpp",
        top_name = top_name,
        **kernel_kwargs
    )

def regression_xosim(name, xo, host_srcs, host_defines = [], hw_test_args = [], extra_test_data = [], hw_test_timeout = None):
    """Declares a host binary plus a manual fast-cosim test for a regression design.

    `<name>-host` builds the design's TAPA host program; `<name>-xosim` runs it
    against the `.xo` under TAPA's own testbench through frt-cosim, the same
    mechanism tests/apps uses. Both are `manual`: they need Vivado and real
    wall time, so nothing runs them unless asked.

    Args:
        name: Base target name.
        xo: Label of the `.xo` to simulate.
        host_srcs: `srcs` of the `<name>-host` cc_binary.
        host_defines: Preprocessor definitions for the host sources.
        hw_test_args: Extra trailing args passed to the host after `--bitstream`.
        extra_test_data: Extra data files needed at run time (e.g. input graphs).
        hw_test_timeout: Timeout of the cosim sh_test; defaults to the
            `enormous` size's ceiling.
    """
    host_label = ":%s-host" % name

    cc_binary(
        name = "%s-host" % name,
        srcs = host_srcs,
        includes = ["include"],
        local_defines = host_defines,
        tags = ["manual"],
        deps = _HOST_DEPS,
    )

    sh_test(
        name = "%s-xosim" % name,
        size = "enormous",
        timeout = hw_test_timeout,
        srcs = ["//bazel:v++_env.sh"],
        args = [
            "$(location %s)" % host_label,
            "--bitstream=$(location %s)" % xo,
        ] + hw_test_args,
        data = extra_test_data + [
            host_label,
            xo,
            _CARGO_RELEASE_ARTIFACTS,
        ],
        tags = [
            "cpu:2",
            "manual",
        ],
    )
