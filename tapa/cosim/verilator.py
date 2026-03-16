"""Verilator-based cosimulation backend for TAPA.

Generates a C++ testbench and builds/runs it with Verilator, providing an
open-source alternative to xsim that works on both Linux and macOS.
"""

import logging
import os
import re
import shutil
import subprocess
import sys
from collections.abc import Sequence
from pathlib import Path

from tapa.cosim.common import AXI, Arg, parse_register_addr

_logger = logging.getLogger().getChild(__name__)


def generate_verilator_tb(
    config: dict,
    axi_list: list[AXI],
    tb_output_dir: str,
) -> None:
    """Generate C++ testbench and support files for Verilator simulation."""
    Path(tb_output_dir).mkdir(parents=True, exist_ok=True)

    top_name: str = config["top_name"]
    args: Sequence[Arg] = config["args"]
    verilog_path: str = config["verilog_path"]
    mode: str = config["mode"]

    _logger.info("Generating Verilator testbench for %s", top_name)
    _logger.info("   Mode: %s", mode)
    _logger.info("   AXI interfaces: %s", [a.name for a in axi_list])

    # Copy RTL and TCL files to output directory for Verilator
    rtl_dir = Path(tb_output_dir) / "rtl"
    rtl_dir.mkdir(parents=True, exist_ok=True)
    for ext in ("*.v", "*.sv", "*.tcl"):
        for src_file in Path(verilog_path).glob(ext):
            target = rtl_dir / src_file.name
            target.write_bytes(src_file.read_bytes())

    # Detect and replace Xilinx IPs with behavioral models
    ip_replacements = _detect_xilinx_ips(rtl_dir)
    for ip_file in ip_replacements:
        _logger.info("   Generated behavioral replacement: %s", ip_file)

    # Detect which peek ports are exposed at the top-level module.
    # Peek ports only exist at the top level for leaf tasks; non-leaf tasks
    # have internal FIFOs that drive peek ports internally.
    top_level_peek = _detect_top_level_peek_ports(rtl_dir / f"{top_name}.v", args)

    # Parse control register addresses (Vitis mode)
    reg_addrs: dict[str, list[str]] = {}
    if mode == "vitis":
        ctrl_path = f"{verilog_path}/{top_name}_control_s_axi.v"
        reg_addrs = parse_register_addr(ctrl_path)

    # Generate the C++ testbench
    tb_cpp = _generate_cpp_testbench(
        top_name, axi_list, args, config, reg_addrs, mode, top_level_peek
    )
    (Path(tb_output_dir) / "tb.cpp").write_text(tb_cpp, encoding="utf-8")

    # Generate the DPI-C FP32 support file
    dpi_c = _generate_dpi_support()
    (Path(tb_output_dir) / "dpi_support.cpp").write_text(dpi_c, encoding="utf-8")

    # Generate build script
    build_sh = _generate_build_script(top_name)
    build_path = Path(tb_output_dir) / "build.sh"
    build_path.write_text(build_sh, encoding="utf-8")
    build_path.chmod(0o755)

    _logger.info("Verilator testbench generated in %s", tb_output_dir)


def launch_verilator(config: dict, tb_output_dir: str) -> None:
    """Build and run the Verilator simulation."""
    top_name: str = config["top_name"]
    _logger.info("Building Verilator simulation for %s", top_name)

    build_script = Path(tb_output_dir) / "build.sh"
    if not build_script.exists():
        _logger.error("Build script not found: %s", build_script)
        sys.exit(1)

    # Build
    result = subprocess.run(
        [str(build_script)],
        check=False,
        cwd=tb_output_dir,
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        _logger.error("Verilator build failed:\n%s\n%s", result.stdout, result.stderr)
        sys.exit(result.returncode)
    _logger.info("Verilator build succeeded")

    # Run
    binary = Path(tb_output_dir) / f"obj_dir/V{top_name}"
    if not binary.exists():
        _logger.error("Simulation binary not found: %s", binary)
        sys.exit(1)

    _logger.info("Running Verilator simulation")
    result = subprocess.run(
        [str(binary)],
        check=False,
        cwd=tb_output_dir,
        capture_output=True,
        text=True,
    )
    sys.stdout.write(result.stdout)
    sys.stderr.write(result.stderr)

    if result.returncode != 0:
        _logger.error("Verilator simulation failed with code %d", result.returncode)
        sys.exit(result.returncode)
    _logger.info("Verilator simulation finished successfully")


def _detect_top_level_peek_ports(
    top_module_path: Path, args: Sequence[Arg]
) -> set[str]:
    """Return the set of peek_qualified_names that exist at the top level.

    Peek ports are only exposed at the top level for leaf tasks.  Non-leaf
    tasks have internal FIFOs that drive peek ports internally, so the top
    module does not include them as I/O ports.  We check specifically for
    port declarations (input/output) to avoid matching internal wires.
    """
    result: set[str] = set()
    if not top_module_path.exists():
        return result
    content = top_module_path.read_text(encoding="utf-8", errors="replace")
    for arg in args:
        pn = arg.peek_qualified_name
        # Check that it's declared as a port (input/output), not an internal wire
        if pn and re.search(
            rf"\b(?:input|output)\b[^;]*\b{re.escape(pn)}_dout\b", content
        ):
            result.add(pn)
    return result


def _detect_xilinx_ips(rtl_dir: Path) -> list[str]:
    """Detect Xilinx IP instantiations and generate behavioral replacements.

    First tries to parse companion TCL files (generated by Vitis HLS) which
    contain precise IP configuration (operation type, precision, latency).
    Falls back to module name heuristics when no TCL file is available.

    Returns a list of generated replacement file names.
    """
    replacements = []

    for v_file in sorted(rtl_dir.glob("*.v")):
        content = v_file.read_text(encoding="utf-8", errors="replace")
        # Find instantiations of *_ip modules
        ip_insts = re.findall(r"^(\w+_ip)\s+\w+\s*\(", content, re.MULTILINE)
        for ip_module in ip_insts:
            ip_path = rtl_dir / f"{ip_module}.v"
            if ip_path.exists():
                ip_content = ip_path.read_text(encoding="utf-8", errors="replace")
                if "`pragma protect" not in ip_content:
                    continue

            # Try TCL-based detection first, then fall back to name heuristics
            tcl_path = rtl_dir / f"{ip_module}.tcl"
            ip_config = None
            if tcl_path.exists():
                ip_config = _parse_ip_tcl(tcl_path)

            if ip_config is not None:
                dpi_func = ip_config["dpi_func"]
                latency = ip_config["latency"]
            else:
                dpi_func = _detect_fp_operation_from_name(ip_module)
                latency = 5  # default pipeline depth
                if dpi_func is None:
                    _logger.warning(
                        "Cannot determine operation for %s — skipping",
                        ip_module,
                    )
                    continue

            replacement = _generate_fp_ip_replacement(ip_module, dpi_func, latency)
            ip_path = rtl_dir / f"{ip_module}.v"
            ip_path.write_text(replacement, encoding="utf-8")
            replacements.append(ip_module)
            total_from_name = _extract_total_latency_from_name(ip_module)
            _logger.info(
                "Generated behavioral replacement: %s"
                " (using %s, c_latency=%d, total_from_name=%s)",
                ip_module,
                dpi_func,
                latency,
                total_from_name,
            )

    return replacements


def _parse_ip_tcl(tcl_path: Path) -> dict | None:
    """Parse a Vitis HLS IP TCL file to extract IP configuration.

    Returns a dict with 'dpi_func' and 'latency', or None if the IP type
    is not supported for behavioral replacement.
    """
    content = tcl_path.read_text(encoding="utf-8", errors="replace")

    # Check if this is a floating_point IP
    if "create_ip -name floating_point" not in content:
        return None

    # Extract CONFIG properties
    config = {}
    for match in re.finditer(r"CONFIG\.(\w+)\s+(\S+)", content):
        config[match.group(1)] = match.group(2).rstrip("\\")

    # Determine precision
    precision = config.get("a_precision_type", "Single")
    is_double = precision.lower() == "double"

    # Determine operation
    add_sub = config.get("add_sub_value", "Add")
    op_type = config.get("operation_type", "")

    if "Add" in op_type or "Subtract" in op_type:
        if add_sub.lower() == "subtract" or add_sub.lower() == "sub":
            op = "sub"
        else:
            op = "add"
    elif "Multiply" in op_type:
        op = "mul"
    elif "Fixed_to_float" in op_type:
        op = "sitofp"
    elif "Float_to_fixed" in op_type:
        op = "fptosi"
    else:
        # Unknown operation type
        return None

    prefix = "fp64" if is_double else "fp32"
    dpi_func = f"{prefix}_{op}"

    # Extract latency
    latency = int(config.get("c_latency", "5"))

    return {"dpi_func": dpi_func, "latency": latency}


# Map of Vitis HLS FP operation name patterns to DPI-C function names
_FP_NAME_MAP = {
    "fadd": "fp32_add",
    "fsub": "fp32_sub",
    "fmul": "fp32_mul",
    "dadd": "fp64_add",
    "dsub": "fp64_sub",
    "dmul": "fp64_mul",
    "sitofp": "fp32_sitofp",
    "uitofp": "fp32_uitofp",
    "fptosi": "fp32_fptosi",
    "fptoui": "fp32_fptoui",
}

# DPI functions that take a single input (unary conversion operations)
_UNARY_DPI_FUNCS = {
    "fp32_sitofp",
    "fp32_uitofp",
    "fp32_fptosi",
    "fp32_fptoui",
    "fp64_sitofp",
    "fp64_uitofp",
    "fp64_fptosi",
    "fp64_fptoui",
}


def _detect_fp_operation_from_name(module_name: str) -> str | None:
    """Detect the floating-point operation from a module name (fallback)."""
    lower = module_name.lower()
    for pattern, func in _FP_NAME_MAP.items():
        if f"_{pattern}_" in lower or f"_{pattern}s_" in lower:
            return func
    return None


def _extract_total_latency_from_name(module_name: str) -> int | None:
    """Extract the documented total latency from an HLS IP module name.

    HLS wrapper names encode the total through-wrapper latency, e.g.:
      Add_fadd_32ns_32ns_32_7_full_dsp_1_ip  →  total latency = 7
      Add_sitofp_32ns_32_5_no_dsp_1_ip       →  total latency = 5
    """
    m = re.search(r"_(\d+)_(?:full|no|medium|max)_dsp_\d+_ip$", module_name)
    if m:
        return int(m.group(1))
    return None


# The HLS wrapper around each _ip adds 2 cycles of overhead:
# 1 cycle for input buffering (din_buf registers) and 1 cycle for
# the ce_r clock-enable delay before aclken reaches the _ip.
_HLS_WRAPPER_OVERHEAD = 2


def _generate_fp_ip_replacement(
    module_name: str, dpi_func: str, latency: int = 5
) -> str:
    """Generate a behavioral Verilog module for a Xilinx FP IP.

    Creates a pipelined module that uses DPI-C for the actual computation.
    Supports both binary (add/sub/mul) and unary (conversion) operations.
    """
    is_unary = dpi_func in _UNARY_DPI_FUNCS
    # Determine bit width from function name
    # ruff: noqa: PLR2004 — 64/32 are standard IEEE 754 widths
    bit_width = 64 if "64" in dpi_func else 32
    ret_type = "longint unsigned" if bit_width == 64 else "int unsigned"
    arg_type = ret_type

    # The _ip module sits inside an HLS wrapper that adds overhead cycles.
    # Extract the documented total latency from the module name and subtract
    # the wrapper overhead to get the correct _ip-internal pipe depth.
    # For fadd (c_latency=5, total=7): pipe_depth = 7 - 2 = 5
    # For sitofp (c_latency=5, total=5): pipe_depth = 5 - 2 = 3
    total_from_name = _extract_total_latency_from_name(module_name)
    if total_from_name is not None:
        pipe_depth = max(total_from_name - _HLS_WRAPPER_OVERHEAD, 1)
    else:
        pipe_depth = max(latency, 1)

    if is_unary:
        return _generate_unary_ip(
            module_name, dpi_func, bit_width, ret_type, arg_type, pipe_depth
        )
    return _generate_binary_ip(
        module_name, dpi_func, bit_width, ret_type, arg_type, pipe_depth
    )


def _generate_binary_ip(  # noqa: PLR0913, PLR0917
    module_name: str,
    dpi_func: str,
    bit_width: int,
    ret_type: str,
    arg_type: str,
    pipe_depth: int,
) -> str:
    """Generate a two-input (binary) FP IP behavioral replacement."""
    return f"""\
// Behavioral replacement for Xilinx floating-point IP
// Generated by TAPA Verilator cosim backend
`timescale 1ns/1ps

module {module_name} (
    input  wire        aclk,
    input  wire        aclken,
    input  wire        s_axis_a_tvalid,
    input  wire [{bit_width - 1}:0] s_axis_a_tdata,
    input  wire        s_axis_b_tvalid,
    input  wire [{bit_width - 1}:0] s_axis_b_tdata,
    output wire        m_axis_result_tvalid,
    output wire [{bit_width - 1}:0] m_axis_result_tdata
);

import "DPI-C" function {ret_type} {dpi_func}(\
input {arg_type} a, input {arg_type} b);

reg [{bit_width - 1}:0] pipe [0:{pipe_depth - 1}];
reg [{pipe_depth - 1}:0]  valid_pipe;

integer i;

always @(posedge aclk) begin
    if (aclken) begin
        pipe[0] <= {dpi_func}(s_axis_a_tdata, s_axis_b_tdata);
        valid_pipe[0] <= s_axis_a_tvalid & s_axis_b_tvalid;
        for (i = 1; i < {pipe_depth}; i = i + 1) begin
            pipe[i] <= pipe[i-1];
            valid_pipe[i] <= valid_pipe[i-1];
        end
    end
end

assign m_axis_result_tdata  = pipe[{pipe_depth - 1}];
assign m_axis_result_tvalid = valid_pipe[{pipe_depth - 1}];

endmodule
"""


def _generate_unary_ip(  # noqa: PLR0913, PLR0917
    module_name: str,
    dpi_func: str,
    bit_width: int,
    ret_type: str,
    arg_type: str,
    pipe_depth: int,
) -> str:
    """Generate a single-input (unary) conversion IP behavioral replacement."""
    return f"""\
// Behavioral replacement for Xilinx conversion IP
// Generated by TAPA Verilator cosim backend
`timescale 1ns/1ps

module {module_name} (
    input  wire        aclk,
    input  wire        aclken,
    input  wire        s_axis_a_tvalid,
    input  wire [{bit_width - 1}:0] s_axis_a_tdata,
    output wire        m_axis_result_tvalid,
    output wire [{bit_width - 1}:0] m_axis_result_tdata
);

import "DPI-C" function {ret_type} {dpi_func}(input {arg_type} a);

reg [{bit_width - 1}:0] pipe [0:{pipe_depth - 1}];
reg [{pipe_depth - 1}:0]  valid_pipe;

integer i;

always @(posedge aclk) begin
    if (aclken) begin
        pipe[0] <= {dpi_func}(s_axis_a_tdata);
        valid_pipe[0] <= s_axis_a_tvalid;
        for (i = 1; i < {pipe_depth}; i = i + 1) begin
            pipe[i] <= pipe[i-1];
            valid_pipe[i] <= valid_pipe[i-1];
        end
    end
end

assign m_axis_result_tdata  = pipe[{pipe_depth - 1}];
assign m_axis_result_tvalid = valid_pipe[{pipe_depth - 1}];

endmodule
"""


def _generate_dpi_support() -> str:
    """Generate C++ file with DPI-C behavioral models for Xilinx IPs."""
    return """\
#include <cstdint>
#include <cstring>

extern "C" {

// IEEE 754 single-precision floating-point addition
unsigned int fp32_add(unsigned int a, unsigned int b) {
    float fa, fb, fc;
    memcpy(&fa, &a, sizeof(float));
    memcpy(&fb, &b, sizeof(float));
    fc = fa + fb;
    unsigned int result;
    memcpy(&result, &fc, sizeof(unsigned int));
    return result;
}

// IEEE 754 single-precision floating-point subtraction
unsigned int fp32_sub(unsigned int a, unsigned int b) {
    float fa, fb, fc;
    memcpy(&fa, &a, sizeof(float));
    memcpy(&fb, &b, sizeof(float));
    fc = fa - fb;
    unsigned int result;
    memcpy(&result, &fc, sizeof(unsigned int));
    return result;
}

// IEEE 754 single-precision floating-point multiplication
unsigned int fp32_mul(unsigned int a, unsigned int b) {
    float fa, fb, fc;
    memcpy(&fa, &a, sizeof(float));
    memcpy(&fb, &b, sizeof(float));
    fc = fa * fb;
    unsigned int result;
    memcpy(&result, &fc, sizeof(unsigned int));
    return result;
}

// IEEE 754 double-precision floating-point addition
unsigned long long fp64_add(unsigned long long a, unsigned long long b) {
    double da, db, dc;
    memcpy(&da, &a, sizeof(double));
    memcpy(&db, &b, sizeof(double));
    dc = da + db;
    unsigned long long result;
    memcpy(&result, &dc, sizeof(unsigned long long));
    return result;
}

// IEEE 754 double-precision floating-point subtraction
unsigned long long fp64_sub(unsigned long long a, unsigned long long b) {
    double da, db, dc;
    memcpy(&da, &a, sizeof(double));
    memcpy(&db, &b, sizeof(double));
    dc = da - db;
    unsigned long long result;
    memcpy(&result, &dc, sizeof(unsigned long long));
    return result;
}

// IEEE 754 double-precision floating-point multiplication
unsigned long long fp64_mul(unsigned long long a, unsigned long long b) {
    double da, db, dc;
    memcpy(&da, &a, sizeof(double));
    memcpy(&db, &b, sizeof(double));
    dc = da * db;
    unsigned long long result;
    memcpy(&result, &dc, sizeof(unsigned long long));
    return result;
}

// Signed integer to IEEE 754 single-precision float
unsigned int fp32_sitofp(unsigned int a) {
    int32_t ia;
    memcpy(&ia, &a, sizeof(int32_t));
    float f = (float)ia;
    unsigned int result;
    memcpy(&result, &f, sizeof(unsigned int));
    return result;
}

// Unsigned integer to IEEE 754 single-precision float
unsigned int fp32_uitofp(unsigned int a) {
    float f = (float)a;
    unsigned int result;
    memcpy(&result, &f, sizeof(unsigned int));
    return result;
}

// IEEE 754 single-precision float to signed integer
unsigned int fp32_fptosi(unsigned int a) {
    float fa;
    memcpy(&fa, &a, sizeof(float));
    int32_t result = (int32_t)fa;
    unsigned int uresult;
    memcpy(&uresult, &result, sizeof(unsigned int));
    return uresult;
}

// IEEE 754 single-precision float to unsigned integer
unsigned int fp32_fptoui(unsigned int a) {
    float fa;
    memcpy(&fa, &a, sizeof(float));
    unsigned int result = (unsigned int)fa;
    return result;
}

// Signed 64-bit integer to IEEE 754 double-precision float
unsigned long long fp64_sitofp(unsigned long long a) {
    int64_t ia;
    memcpy(&ia, &a, sizeof(int64_t));
    double f = (double)ia;
    unsigned long long result;
    memcpy(&result, &f, sizeof(unsigned long long));
    return result;
}

// Unsigned 64-bit integer to IEEE 754 double-precision float
unsigned long long fp64_uitofp(unsigned long long a) {
    double f = (double)a;
    unsigned long long result;
    memcpy(&result, &f, sizeof(unsigned long long));
    return result;
}

// IEEE 754 double-precision float to signed 64-bit integer
unsigned long long fp64_fptosi(unsigned long long a) {
    double fa;
    memcpy(&fa, &a, sizeof(double));
    int64_t result = (int64_t)fa;
    unsigned long long uresult;
    memcpy(&uresult, &result, sizeof(unsigned long long));
    return uresult;
}

// IEEE 754 double-precision float to unsigned 64-bit integer
unsigned long long fp64_fptoui(unsigned long long a) {
    double fa;
    memcpy(&fa, &a, sizeof(double));
    unsigned long long result = (unsigned long long)fa;
    return result;
}

}  // extern "C"
"""


def _generate_cpp_testbench(  # noqa: PLR0913, PLR0917
    top_name: str,
    axi_list: list[AXI],
    args: Sequence[Arg],
    config: dict,
    reg_addrs: dict[str, list[str]],
    mode: str,
    top_level_peek: set[str] | None = None,
) -> str:
    """Generate a C++ testbench for Verilator simulation."""
    lines: list[str] = []
    lines.extend(_cpp_preamble(top_name, axi_list, args, mode))
    lines.extend(
        _cpp_main_body(
            top_name, axi_list, args, config, reg_addrs, mode, top_level_peek
        )
    )
    lines.extend(_cpp_axi_helpers(axi_list, mode))
    return "\n".join(lines) + "\n"


def _cpp_preamble(
    top_name: str,
    axi_list: list[AXI],
    args: Sequence[Arg],
    mode: str,
) -> list[str]:
    """Generate C++ includes, types, and global declarations."""
    lines: list[str] = [
        "#include <cerrno>",
        "#include <cstdio>",
        "#include <cstdlib>",
        "#include <cstring>",
        "#include <fstream>",
        "#include <map>",
        "#include <vector>",
        "",
        "#include <fcntl.h>",
        "#include <sys/mman.h>",
        "#include <sys/stat.h>",
        "#include <sched.h>",
        "#include <unistd.h>",
        f'#include "V{top_name}.h"',
        '#include "verilated.h"',
        "",
        "static std::map<uint64_t, uint8_t> memory;",
        "",
        "// Byte-oriented memory read: copies nbytes from memory into buf",
        "static void mem_read(uint64_t addr, void* buf, size_t nbytes) {",
        "    auto* dst = reinterpret_cast<uint8_t*>(buf);",
        "    for (size_t i = 0; i < nbytes; i++)",
        "        dst[i] = memory[addr + i];",
        "}",
        "",
        "// Byte-oriented memory write: copies nbytes from buf into memory",
        "// strb is a per-byte write enable bitmask",
        (
            "static void mem_write(uint64_t addr, const void* buf,"
            " uint64_t strb, size_t nbytes) {"
        ),
        "    auto* src = reinterpret_cast<const uint8_t*>(buf);",
        "    for (size_t i = 0; i < nbytes; i++)",
        "        if (strb & (1ULL << i))",
        "            memory[addr + i] = src[i];",
        "}",
        "",
        "static void load_binary(const char* path, uint64_t base, size_t size) {",
        "    std::ifstream f(path, std::ios::binary);",
        '    if (!f) { fprintf(stderr, "Cannot open %s\\n", path); return; }',
        "    std::vector<char> buf(size);",
        "    f.read(buf.data(), size);",
        "    size_t n = f.gcount();",
        "    for (size_t i = 0; i < n; i++) memory[base + i] = (uint8_t)buf[i];",
        "}",
        "",
        "static void dump_binary(const char* path, uint64_t base, size_t size) {",
        "    std::ofstream f(path, std::ios::binary);",
        "    for (size_t i = 0; i < size; i++)",
        "        f.put((char)memory[base + i]);",
        "}",
        "",
        "struct AxiReadPort {",
        "    bool busy = false;",
        "    uint64_t addr = 0;",
        "    uint8_t len = 0, beat = 0, id = 0;",
        "};",
        "",
        "struct AxiWritePort {",
        "    bool aw_got = false, b_pending = false;",
        "    uint64_t addr = 0;",
        "    uint8_t beat = 0, id = 0;",
        "};",
        "",
    ]
    for axi in axi_list:
        lines.append(f"static AxiReadPort rd_{axi.name};")
        lines.append(f"static AxiWritePort wr_{axi.name};")
    lines.extend(
        [
            "",
            f"static V{top_name}* dut;",
            "",
            "static void tick() {",
            "    dut->ap_clk = 1; dut->eval();",
            "    dut->ap_clk = 0; dut->eval();",
            "}",
            "",
        ]
    )
    lines.append("static void service_all_axi();")
    if mode == "vitis":
        lines.append("static void ctrl_write(uint8_t addr, uint32_t data);")

    # Add stream queue types if there are stream args
    stream_args = [a for a in args if a.is_stream]
    if stream_args:
        lines.extend(_cpp_stream_types(stream_args))

    lines.append("")
    return lines


def _cpp_stream_types(stream_args: Sequence[Arg]) -> list[str]:
    """Generate C++ shared memory queue types and declarations.

    The host creates SharedMemoryQueues via shm_open; we mmap them and
    interact with them concurrently during simulation (like xosim DPI).
    """
    lines: list[str] = [
        "",
        "// SharedMemoryQueue access for FIFO-style AXI-Stream interfaces.",
        "// The host creates SharedMemoryQueues via shm_open; we mmap them and",
        "// interact with them concurrently during simulation (like xosim DPI).",
        "",
        "struct ShmMapping {",
        "    uint8_t* base = nullptr;",
        "    size_t len = 0;",
        "    int fd = -1;",
        "    uint32_t depth = 0;",
        "    uint32_t width = 0;",
        "    uint64_t* tail_ptr = nullptr;",
        "    uint64_t* head_ptr = nullptr;",
        "    uint8_t* ring = nullptr;",
        "};",
        "",
        "static ShmMapping shm_map(const char* path) {",
        "    ShmMapping m;",
        "    m.fd = shm_open(path, O_RDWR, 0600);",
        "    if (m.fd < 0) return m;",
        "    struct stat st;",
        "    if (fstat(m.fd, &st) < 0 || st.st_size < 32) {",
        "        close(m.fd); m.fd = -1; return m;",
        "    }",
        "    m.len = st.st_size;",
        "    m.base = static_cast<uint8_t*>(",
        "        mmap(nullptr, m.len, PROT_READ | PROT_WRITE,",
        "             MAP_SHARED, m.fd, 0));",
        "    if (m.base == MAP_FAILED) {",
        "        m.base = nullptr; close(m.fd); m.fd = -1; return m;",
        "    }",
        "    memcpy(&m.depth, m.base + 8, 4);",
        "    memcpy(&m.width, m.base + 12, 4);",
        "    m.tail_ptr = reinterpret_cast<uint64_t*>(m.base + 16);",
        "    m.head_ptr = reinterpret_cast<uint64_t*>(m.base + 24);",
        "    m.ring = m.base + 32;",
        "    return m;",
        "}",
        "",
        "static void shm_unmap(ShmMapping& m) {",
        "    if (m.base) munmap(m.base, m.len);",
        "    if (m.fd >= 0) close(m.fd);",
        "    m.base = nullptr; m.fd = -1;",
        "}",
        "",
    ]
    for arg in stream_args:
        n = arg.qualified_name
        lines.append(f"static ShmMapping shm_{n};")
        if arg.port.is_istream:
            lines.append(f"static bool last_empty_n_{n} = false;")
        elif arg.port.is_ostream:
            lines.append(f"static bool last_full_n_{n} = false;")
    lines.append("")
    return lines


def _cpp_main_body(  # noqa: PLR0913, PLR0917
    top_name: str,
    axi_list: list[AXI],
    args: Sequence[Arg],
    config: dict,
    reg_addrs: dict[str, list[str]],
    mode: str,
    top_level_peek: set[str] | None = None,
) -> list[str]:
    """Generate the main() function body."""
    stream_args = [a for a in args if a.is_stream]
    lines: list[str] = [
        "int main(int argc, char** argv) {",
        "    Verilated::commandArgs(argc, argv);",
        f"    dut = new V{top_name};",
        "",
        "    // Initialize",
        "    dut->ap_clk = 0;",
        "    dut->ap_rst_n = 0;",
    ]
    if mode == "vitis":
        lines.extend(
            [
                "    dut->s_axi_control_AWVALID = 0;",
                "    dut->s_axi_control_WVALID = 0;",
                "    dut->s_axi_control_ARVALID = 0;",
                "    dut->s_axi_control_BREADY = 0;",
                "    dut->s_axi_control_RREADY = 0;",
            ]
        )
    else:
        lines.append("    dut->ap_start = 0;")

    lines.extend(_cpp_axi_port_init(axi_list))
    lines.extend(_cpp_stream_init(stream_args, top_level_peek))

    lines.extend(
        [
            "",
            "    // Reset",
            "    for (int i = 0; i < 10; i++) tick();",
            "    dut->ap_rst_n = 1;",
            "    for (int i = 0; i < 5; i++) tick();",
            "",
        ]
    )

    lines.extend(_cpp_load_data(axi_list, stream_args, config))

    # Shadow variables hold the previous tick's DUT outputs so that
    # stream_service sees 1-cycle-delayed values (DUT→TB delay).
    # Staged variables hold the next TB→DUT values; they are applied to
    # the DUT at the start of the next iteration (TB→DUT delay).
    if stream_args:
        lines.extend(_cpp_stream_shadow_decls(stream_args))
        lines.extend(_cpp_stream_staged_decls(stream_args, top_level_peek))

    # Control register writes / port setup
    if mode == "vitis":
        lines.extend(_cpp_ctrl_writes(args, config, reg_addrs))
    else:
        lines.extend(_cpp_hls_port_setup(args, axi_list, config))

    # HLS port setup ends with tick() after ap_start=1.  Capture the DUT's
    # outputs from that tick so the main loop's first stream_service uses
    # correct prev_ values (otherwise the first read/write assertion is lost).
    if stream_args:
        lines.extend(_cpp_stream_shadow_save(stream_args))

    lines.extend(_cpp_sim_loop(stream_args, mode, top_level_peek))
    lines.extend(_cpp_dump_data(axi_list, stream_args, config))

    lines.extend(
        [
            "",
            '    printf("Simulation completed successfully\\n");',
            "    delete dut;",
            "    return 0;",
            "}",
            "",
        ]
    )
    return lines


def _cpp_axi_port_init(axi_list: list[AXI]) -> list[str]:
    """Generate AXI port initialization statements."""
    lines: list[str] = []
    for axi in axi_list:
        n = axi.name
        lines.append(f"    dut->m_axi_{n}_ARREADY = 0;")
        lines.append(f"    dut->m_axi_{n}_RVALID = 0;")
        # Wide data ports need memset, narrow ports can use = 0
        if axi.data_width > 64:
            lines.append(
                f"    memset(&dut->m_axi_{n}_RDATA, 0, sizeof(dut->m_axi_{n}_RDATA));"
            )
        else:
            lines.append(f"    dut->m_axi_{n}_RDATA = 0;")
        lines.extend(
            [
                (
                    f"    dut->m_axi_{n}_RLAST = 0;"
                    f" dut->m_axi_{n}_RID = 0;"
                    f" dut->m_axi_{n}_RRESP = 0;"
                ),
                f"    dut->m_axi_{n}_AWREADY = 1; dut->m_axi_{n}_WREADY = 1;",
                (
                    f"    dut->m_axi_{n}_BVALID = 0;"
                    f" dut->m_axi_{n}_BID = 0;"
                    f" dut->m_axi_{n}_BRESP = 0;"
                ),
            ]
        )
    return lines


def _cpp_stream_init(
    stream_args: Sequence[Arg], top_level_peek: set[str] | None = None
) -> list[str]:
    """Generate FIFO stream signal initialization statements."""
    lines: list[str] = []
    for arg in stream_args:
        n = arg.qualified_name
        pn = arg.peek_qualified_name
        # Only drive peek ports that exist at the top-level module
        if pn and top_level_peek is not None and pn not in top_level_peek:
            pn = None
        # Stream FIFO ports are data_width + 1 (EOT bit)
        port_width = arg.port.data_width + 1
        if arg.port.is_istream:
            if port_width > 64:
                lines.append(f"    memset(&dut->{n}_dout, 0, sizeof(dut->{n}_dout));")
                if pn:
                    lines.append(
                        f"    memset(&dut->{pn}_dout, 0, sizeof(dut->{pn}_dout));"
                    )
            else:
                lines.append(f"    dut->{n}_dout = 0;")
                if pn:
                    lines.append(f"    dut->{pn}_dout = 0;")
            lines.append(f"    dut->{n}_empty_n = 0;")
            if pn:
                lines.append(f"    dut->{pn}_empty_n = 0;")
        elif arg.port.is_ostream:
            lines.append(f"    dut->{n}_full_n = 0;")
    return lines


def _cpp_load_data(
    axi_list: list[AXI],
    stream_args: Sequence[Arg],
    config: dict,
) -> list[str]:
    """Generate binary and stream data loading statements."""
    lines: list[str] = []
    axi_to_data = config.get("axi_to_data_file", {})
    axi_to_size = config.get("axi_to_c_array_size", {})
    for idx, axi in enumerate(axi_list):
        data_path = axi_to_data.get(axi.name, "")
        c_size = axi_to_size.get(axi.name, "0")
        byte_size = f"{c_size} * {axi.data_width // 8}"
        base_addr = f"0x{idx + 1}0000000ULL"
        lines.append(f"    const uint64_t {axi.name}_BASE = {base_addr};")
        lines.append(f"    const size_t {axi.name}_SIZE = {byte_size};")
        if data_path:
            lines.append(
                f'    load_binary("{data_path}", {axi.name}_BASE, {axi.name}_SIZE);'
            )
    lines.append("")

    axis_to_data = config.get("axis_to_data_file", {})
    for arg in stream_args:
        data_path = axis_to_data.get(arg.qualified_name, "")
        if data_path:
            n = arg.qualified_name
            lines.extend(
                [
                    f'    shm_{n} = shm_map("{data_path}");',
                    f"    if (!shm_{n}.base) {{",
                    (
                        f'        fprintf(stderr, "ERROR: shm_map failed for'
                        f' {n} path=%s errno=%d\\n",'
                        f' "{data_path}", errno);'
                    ),
                    "        return 1;",
                    "    }",
                ]
            )
    if stream_args:
        lines.append("")
    return lines


def _cpp_stream_shadow_decls(stream_args: Sequence[Arg]) -> list[str]:
    """Generate shadow variable declarations for DUT stream output signals.

    Shadow variables store the previous tick's DUT outputs so that
    stream_service sees 1-cycle-delayed values, matching xsim's pre-NBA
    DPI read semantics.
    """
    lines: list[str] = []
    for arg in stream_args:
        n = arg.qualified_name
        dw = arg.port.data_width
        port_width = dw + 1
        if arg.port.is_istream:
            lines.append(f"    bool prev_{n}_read = false;")
        elif arg.port.is_ostream:
            lines.append(f"    bool prev_{n}_write = false;")
            if port_width > 64:
                nw = (port_width + 31) // 32
                lines.append(f"    uint32_t prev_{n}_din[{nw}] = {{}};")
            else:
                lines.append(f"    uint64_t prev_{n}_din = 0;")
    return lines


def _cpp_stream_staged_decls(
    stream_args: Sequence[Arg], top_level_peek: set[str] | None = None
) -> list[str]:
    """Generate staged variable declarations for TB→DUT stream signals.

    In xsim, DPI outputs use NBA (<=) so the DUT sees updates one cycle
    later.  Staged variables emulate this: stream_service writes to staged
    vars, and they are applied to the DUT at the start of the NEXT
    iteration.
    """
    lines: list[str] = []
    for arg in stream_args:
        n = arg.qualified_name
        dw = arg.port.data_width
        port_width = dw + 1
        pn = arg.peek_qualified_name
        if pn and top_level_peek is not None and pn not in top_level_peek:
            pn = None
        if arg.port.is_istream:
            lines.append(f"    uint8_t staged_{n}_empty_n = 0;")
            if port_width > 64:
                lines.append(
                    f"    alignas(4) uint8_t staged_{n}_dout"
                    f"[sizeof(dut->{n}_dout)] = {{}};"
                )
            else:
                lines.append(f"    uint64_t staged_{n}_dout = 0;")
            if pn:
                lines.append(f"    uint8_t staged_{pn}_empty_n = 0;")
                if port_width > 64:
                    lines.append(
                        f"    alignas(4) uint8_t staged_{pn}_dout"
                        f"[sizeof(dut->{pn}_dout)] = {{}};"
                    )
                else:
                    lines.append(f"    uint64_t staged_{pn}_dout = 0;")
        elif arg.port.is_ostream:
            lines.append(f"    uint8_t staged_{n}_full_n = 0;")
    return lines


def _cpp_stream_apply_staged(
    stream_args: Sequence[Arg], top_level_peek: set[str] | None = None
) -> list[str]:
    """Generate statements to apply staged TB→DUT values to the DUT.

    Also updates last_empty_n / last_full_n to reflect the applied state.
    """
    lines: list[str] = []
    for arg in stream_args:
        n = arg.qualified_name
        dw = arg.port.data_width
        port_width = dw + 1
        pn = arg.peek_qualified_name
        if pn and top_level_peek is not None and pn not in top_level_peek:
            pn = None
        if arg.port.is_istream:
            lines.append(f"        dut->{n}_empty_n = staged_{n}_empty_n;")
            if port_width > 64:
                lines.append(
                    f"        memcpy(&dut->{n}_dout, staged_{n}_dout,"
                    f" sizeof(dut->{n}_dout));"
                )
            else:
                lines.append(f"        dut->{n}_dout = staged_{n}_dout;")
            lines.append(f"        last_empty_n_{n} = (staged_{n}_empty_n != 0);")
            if pn:
                lines.append(f"        dut->{pn}_empty_n = staged_{pn}_empty_n;")
                if port_width > 64:
                    lines.append(
                        f"        memcpy(&dut->{pn}_dout, staged_{pn}_dout,"
                        f" sizeof(dut->{pn}_dout));"
                    )
                else:
                    lines.append(f"        dut->{pn}_dout = staged_{pn}_dout;")
        elif arg.port.is_ostream:
            lines.append(f"        dut->{n}_full_n = staged_{n}_full_n;")
            lines.append(f"        last_full_n_{n} = (staged_{n}_full_n != 0);")
    return lines


def _cpp_stream_shadow_save(stream_args: Sequence[Arg]) -> list[str]:
    """Generate statements to snapshot DUT stream outputs into shadow vars."""
    lines: list[str] = []
    for arg in stream_args:
        n = arg.qualified_name
        dw = arg.port.data_width
        port_width = dw + 1
        if arg.port.is_istream:
            lines.append(f"        prev_{n}_read = dut->{n}_read;")
        elif arg.port.is_ostream:
            lines.append(f"        prev_{n}_write = dut->{n}_write;")
            if port_width > 64:
                lines.append(
                    f"        memcpy(prev_{n}_din, &dut->{n}_din,"
                    f" sizeof(prev_{n}_din));"
                )
            else:
                lines.append(f"        prev_{n}_din = dut->{n}_din;")
    return lines


def _cpp_stream_service(
    stream_args: Sequence[Arg], top_level_peek: set[str] | None = None
) -> list[str]:
    """Generate FIFO stream servicing code using direct SharedMemoryQueue access.

    Mirrors the xsim DPI istream/ostream protocol: the testbench reads/writes
    the mmap'd ring buffer concurrently with the host process each cycle.

    DUT output signals (read, write, din) are read from ``prev_*`` shadow
    variables so that stream_service sees values from the *previous* tick,
    matching xsim's pre-NBA DPI read semantics (DUT→TB: 1 cycle).

    TB-to-DUT signals (empty_n, dout, full_n) are written to ``staged_*``
    variables instead of directly to the DUT.  The staged values are applied
    at the start of the *next* iteration by ``_cpp_stream_apply_staged``,
    giving a 1-cycle TB→DUT delay that matches xsim's NBA semantics.

    Combined with the ``apply_staged → stream_service → tick → shadow_save``
    ordering, the total round trip is 2 cycles, matching xsim exactly.
    """
    lines: list[str] = []
    for arg in stream_args:
        n = arg.qualified_name
        w = (arg.port.data_width + 7) // 8
        dw = arg.port.data_width
        port_width = dw + 1
        if arg.port.is_istream:
            pn = arg.peek_qualified_name
            if pn and top_level_peek is not None and pn not in top_level_peek:
                pn = None
            # EOT bit set code (bit data_width in staged dout)
            # For > 64 bits, staged_dout is a uint8_t array;
            # for <= 64 bits, it is a uint64_t scalar (needs &).
            staged_dout_ptr = (
                f"staged_{n}_dout" if port_width > 64 else f"&staged_{n}_dout"
            )
            if port_width > 64:
                eot_set = (
                    f"                    reinterpret_cast<uint32_t*>"
                    f"(staged_{n}_dout)[{dw // 32}]"
                    f" |= (1U << {dw % 32});"
                )
                zero_dout = (
                    f"                memset(staged_{n}_dout, 0,"
                    f" sizeof(staged_{n}_dout));"
                )
            else:
                eot_set = f"                    staged_{n}_dout |= (1ULL << {dw});"
                zero_dout = f"                staged_{n}_dout = 0;"
            lines.extend(
                [
                    f"        if (shm_{n}.base) {{",
                    f"            if (last_empty_n_{n} && prev_{n}_read) {{",
                    f"                __atomic_store_n(shm_{n}.tail_ptr,",
                    (
                        f"                    __atomic_load_n(shm_{n}.tail_ptr,"
                        " __ATOMIC_ACQUIRE) + 1,"
                    ),
                    "                    __ATOMIC_RELEASE);",
                    "            }",
                    (
                        f"            uint64_t h_{n} = __atomic_load_n("
                        f"shm_{n}.head_ptr, __ATOMIC_ACQUIRE);"
                    ),
                    (
                        f"            uint64_t t_{n} = __atomic_load_n("
                        f"shm_{n}.tail_ptr, __ATOMIC_ACQUIRE);"
                    ),
                    f"            if (h_{n} > t_{n}) {{",
                    (
                        f"                size_t off_{n} ="
                        f" size_t(t_{n} % shm_{n}.depth)"
                        f" * shm_{n}.width;"
                    ),
                    zero_dout,
                    (
                        f"                memcpy({staged_dout_ptr},"
                        f" shm_{n}.ring + off_{n}, {w});"
                    ),
                    (f"                if (shm_{n}.ring[off_{n} + {w}])"),
                    eot_set,
                    f"                staged_{n}_empty_n = 1;",
                    "            } else {",
                    f"                staged_{n}_empty_n = 0;",
                    "            }",
                    "        }",
                ]
            )
            # Mirror peek port from read port (TAPA_WHILE_NOT_EOT uses peek)
            if pn:
                if port_width > 64:
                    lines.append(
                        f"        memcpy(staged_{pn}_dout, staged_{n}_dout,"
                        f" sizeof(staged_{n}_dout));"
                    )
                else:
                    lines.append(f"        staged_{pn}_dout = staged_{n}_dout;")
                lines.append(f"        staged_{pn}_empty_n = staged_{n}_empty_n;")
        elif arg.port.is_ostream:
            # Extract EOT bit from din (bit data_width)
            if port_width > 64:
                eot_extract = (
                    f"            uint8_t eot_{n} ="
                    f" (reinterpret_cast<uint32_t*>"
                    f"(prev_{n}_din)[{dw // 32}]"
                    f" >> {dw % 32}) & 1;"
                )
                din_ptr = f"prev_{n}_din"
            else:
                eot_extract = (
                    f"            uint8_t eot_{n} = (prev_{n}_din >> {dw}) & 1;"
                )
                din_ptr = f"&prev_{n}_din"
            lines.extend(
                [
                    (
                        f"        if (last_full_n_{n} && prev_{n}_write"
                        f" && shm_{n}.base) {{"
                    ),
                    (
                        f"            uint64_t wh_{n} = __atomic_load_n("
                        f"shm_{n}.head_ptr, __ATOMIC_ACQUIRE);"
                    ),
                    (
                        f"            size_t woff_{n} ="
                        f" size_t(wh_{n} % shm_{n}.depth)"
                        f" * shm_{n}.width;"
                    ),
                    (f"            memset(shm_{n}.ring + woff_{n}, 0, shm_{n}.width);"),
                    (f"            memcpy(shm_{n}.ring + woff_{n}, {din_ptr}, {w});"),
                    eot_extract,
                    (f"            shm_{n}.ring[woff_{n} + {w}] = eot_{n};"),
                    (
                        f"            __atomic_store_n(shm_{n}.head_ptr,"
                        f" wh_{n} + 1, __ATOMIC_RELEASE);"
                    ),
                    "        }",
                    f"        if (shm_{n}.base) {{",
                    (
                        f"            uint64_t fh_{n} = __atomic_load_n("
                        f"shm_{n}.head_ptr, __ATOMIC_ACQUIRE);"
                    ),
                    (
                        f"            uint64_t ft_{n} = __atomic_load_n("
                        f"shm_{n}.tail_ptr, __ATOMIC_ACQUIRE);"
                    ),
                    (
                        f"            staged_{n}_full_n ="
                        f" (fh_{n} - ft_{n} < shm_{n}.depth) ? 1 : 0;"
                    ),
                    "        } else {",
                    f"            staged_{n}_full_n = 1;",
                    "        }",
                ]
            )
    return lines


def _cpp_stream_stall_cond(stream_args: Sequence[Arg]) -> str:
    """Return a C++ condition that is true when any stream is stalled."""
    parts: list[str] = []
    for a in stream_args:
        n = a.qualified_name
        if a.port.is_istream:
            parts.append(f"!staged_{n}_empty_n")
        elif a.port.is_ostream:
            parts.append(f"!staged_{n}_full_n")
    return " || ".join(parts)


def _cpp_sim_loop(
    stream_args: Sequence[Arg],
    mode: str,
    top_level_peek: set[str] | None = None,
) -> list[str]:
    """Generate the main simulation loop."""
    lines: list[str] = [
        '    printf("Kernel started, running simulation...\\n");',
        "",
    ]

    # Stall counter: only yield CPU after several consecutive stall cycles.
    # This avoids busy-spinning on transient empty/full
    # conditions while still yielding when the host process is slow.
    stall_cond = _cpp_stream_stall_cond(stream_args)
    stall_decl = "    int stall_count = 0;" if stall_cond else ""

    lines.extend(
        [
            "    bool done = false;",
            "    int timeout = 50000000;",
            stall_decl,
        ]
    )
    lines.extend(
        [
            "    for (int cycle = 0; cycle < timeout; cycle++) {",
            "        service_all_axi();",
        ]
    )

    # Ordering: apply_staged → stream_service(prev_) → tick → shadow_save.
    #
    # In xsim, DPI outputs use NBA (<=) so the DUT sees stream port
    # updates one cycle AFTER the DPI computes them.  To match:
    #   - apply_staged writes PREVIOUS iteration's service results to
    #     the DUT (1-cycle TB→DUT delay, matching NBA).
    #   - stream_service reads prev_ shadow variables from the previous
    #     tick (1-cycle DUT→TB delay, matching pre-NBA DPI reads).
    #   - stream_service writes to staged_ variables (applied next iter).
    #   - tick evaluates the DUT with the applied (old) values.
    #   - shadow_save captures DUT outputs for the next iteration.
    # Total round trip: 2 cycles, matching xsim exactly.
    lines.extend(_cpp_stream_apply_staged(stream_args, top_level_peek))
    lines.extend(_cpp_stream_service(stream_args, top_level_peek))

    lines.append("        tick();")
    lines.extend(_cpp_stream_shadow_save(stream_args))

    # Yield CPU when streams stay stalled for multiple consecutive cycles.
    if stall_cond:
        lines.extend(
            [
                f"        if ({stall_cond}) {{",
                "            if (++stall_count > 16) sched_yield();",
                "        } else {",
                "            stall_count = 0;",
                "        }",
            ]
        )

    if mode == "vitis":
        lines.extend(
            [
                "        if (dut->__SYM__interrupt) {",
                '            printf("Kernel done after %d cycles\\n", cycle);',
                "            done = true;",
                "            break;",
                "        }",
            ]
        )
    else:
        lines.extend(
            [
                "        if (dut->ap_done) {",
                '            printf("Kernel done after %d cycles\\n", cycle);',
                "            done = true;",
                "            break;",
                "        }",
                "        if (dut->ap_ready) {",
                "            dut->ap_start = 0;",
                "        }",
            ]
        )

    lines.append("    }")

    # After ap_done, continue ticking to drain any remaining pipeline
    # writes.  Same ordering: apply_staged → service → tick → shadow_save.
    if stream_args:
        lines.extend(
            [
                "    if (done) {",
                "        for (int drain = 0; drain < 100; drain++) {",
                "            service_all_axi();",
            ]
        )
        lines.extend(_cpp_stream_apply_staged(stream_args, top_level_peek))
        lines.extend(_cpp_stream_service(stream_args, top_level_peek))
        lines.append("            tick();")
        lines.extend(_cpp_stream_shadow_save(stream_args))
        if stall_cond:
            lines.extend(
                [
                    f"            if ({stall_cond}) {{",
                    "                if (++stall_count > 16) sched_yield();",
                    "            } else {",
                    "                stall_count = 0;",
                    "            }",
                ]
            )
        lines.extend(
            [
                "        }",
                "    }",
            ]
        )

    lines.extend(
        [
            "",
            "    if (!done) {",
            (
                '        printf("TIMEOUT: kernel did not complete'
                ' in %d cycles\\n", timeout);'
            ),
            "        delete dut;",
            "        return 1;",
            "    }",
            "",
        ]
    )
    return lines


def _cpp_dump_data(
    axi_list: list[AXI],
    stream_args: Sequence[Arg],
    config: dict,
) -> list[str]:
    """Generate binary and stream data dumping statements."""
    lines: list[str] = []
    axi_to_data = config.get("axi_to_data_file", {})
    for axi in axi_list:
        data_path = axi_to_data.get(axi.name, "")
        if data_path:
            # Output path convention: <base>_out.bin (strip .bin from input)
            out_path = _get_output_path(data_path)
            lines.append(
                f'    dump_binary("{out_path}", {axi.name}_BASE, {axi.name}_SIZE);'
            )

    lines.extend(f"    shm_unmap(shm_{arg.qualified_name});" for arg in stream_args)
    return lines


def _get_output_path(input_path: str) -> str:
    """Convert an input data path to the corresponding output path.

    Follows the convention used by tapa_fast_cosim_device:
    <base>.bin → <base>_out.bin
    """
    p = Path(input_path)
    return str(p.with_name(p.stem + "_out.bin"))


def _cpp_ctrl_writes(
    args: Sequence[Arg],
    config: dict,
    reg_addrs: dict[str, list[str]],
) -> list[str]:
    """Generate control register write statements for Vitis mode."""
    lines: list[str] = [
        "    // Enable interrupt for done detection",
        "    ctrl_write(0x04, 1);  // GIE",
        "    ctrl_write(0x08, 1);  // IER",
        "",
    ]
    for arg in args:
        # Vitis HLS names mmap registers as "<name>_offset"
        mmap_reg_name = arg.name
        if arg.is_mmap and mmap_reg_name not in reg_addrs:
            mmap_reg_name = f"{arg.name}_offset"
        if arg.is_mmap and mmap_reg_name in reg_addrs:
            addrs = reg_addrs[mmap_reg_name]
            base = f"{arg.name}_BASE"
            for i, addr in enumerate(addrs[:2]):
                a = addr.replace("'h", "0x")
                val = f"(uint32_t){base}" if i == 0 else f"(uint32_t)({base} >> 32)"
                lines.append(f"    ctrl_write({a}, {val});")

    scalar_to_val = config.get("scalar_to_val", {})
    for arg in args:
        if arg.is_scalar and arg.qualified_name in reg_addrs:
            addrs = reg_addrs[arg.qualified_name]
            val = scalar_to_val.get(arg.qualified_name, "'h0")
            hex_val = val.replace("'h", "0x")
            for i, addr in enumerate(addrs[:2]):
                a = addr.replace("'h", "0x")
                v = (
                    f"(uint32_t){hex_val}"
                    if i == 0
                    else f"(uint32_t)((uint64_t){hex_val} >> 32)"
                )
                lines.append(f"    ctrl_write({a}, {v});")

    lines.extend(["", "    // Start kernel", "    ctrl_write(0x00, 1);"])
    return lines


def _cpp_hls_port_setup(
    args: Sequence[Arg],
    axi_list: list[AXI],
    config: dict,
) -> list[str]:
    """Generate direct port assignments for HLS mode."""
    lines: list[str] = []
    scalar_to_val = config.get("scalar_to_val", {})

    # Set mmap offset ports
    for arg in args:
        if arg.is_mmap:
            # Find corresponding AXI base address
            found = False
            for idx, axi in enumerate(axi_list):
                if axi.name == arg.name:
                    base = f"0x{idx + 1}0000000ULL"
                    lines.append(f"    dut->{arg.name}_offset = {base};")
                    found = True
                    break
            if not found:
                _logger.warning("No AXI port found for mmap arg %s", arg.name)

    # Set scalar ports
    for arg in args:
        if arg.is_scalar:
            val = scalar_to_val.get(arg.qualified_name, "'h0")
            hex_val = val.replace("'h", "0x")
            lines.append(f"    dut->{arg.name} = {hex_val};")

    lines.extend(
        [
            "",
            "    // Start kernel",
            "    dut->ap_start = 1;",
            "    service_all_axi();",
            "    tick();",
        ]
    )
    return lines


def _cpp_axi_helpers(axi_list: list[AXI], mode: str) -> list[str]:
    """Generate AXI service functions and ctrl_write."""
    lines: list[str] = []

    # service_all_axi — generates inline read/write handling per AXI port
    lines.append("static void service_all_axi() {")
    for axi in axi_list:
        n = axi.name
        data_bytes = axi.data_width // 8
        lines.extend(_cpp_axi_read_service(n, data_bytes))
        lines.extend(_cpp_axi_write_service(n, data_bytes))
    lines.extend(["}", ""])

    if mode == "vitis":
        lines.extend(_CPP_CTRL_WRITE_FUNC)

    return lines


def _cpp_axi_read_service(name: str, data_bytes: int) -> list[str]:
    """Generate inline AXI read service code for one port."""
    n = name
    return [
        "    // AXI read service for " + n,
        f"    dut->m_axi_{n}_ARREADY = !rd_{n}.busy;",
        f"    if (dut->m_axi_{n}_ARVALID && !rd_{n}.busy) {{",
        f"        rd_{n}.busy = true;",
        f"        rd_{n}.addr = dut->m_axi_{n}_ARADDR;",
        f"        rd_{n}.len = dut->m_axi_{n}_ARLEN;",
        f"        rd_{n}.id = dut->m_axi_{n}_ARID;",
        f"        rd_{n}.beat = 0;",
        "    }",
        f"    if (rd_{n}.busy) {{",
        f"        dut->m_axi_{n}_RVALID = 1;",
        f"        mem_read(rd_{n}.addr + (uint64_t)rd_{n}.beat * {data_bytes},",
        f"                 &dut->m_axi_{n}_RDATA, {data_bytes});",
        f"        dut->m_axi_{n}_RLAST = (rd_{n}.beat == rd_{n}.len) ? 1 : 0;",
        f"        dut->m_axi_{n}_RID = rd_{n}.id;",
        f"        dut->m_axi_{n}_RRESP = 0;",
        f"        if (dut->m_axi_{n}_RREADY) {{",
        f"            if (rd_{n}.beat >= rd_{n}.len) rd_{n}.busy = false;",
        f"            else rd_{n}.beat++;",
        "        }",
        "    } else {",
        f"        dut->m_axi_{n}_RVALID = 0;",
        "    }",
    ]


def _cpp_axi_write_service(name: str, data_bytes: int) -> list[str]:
    """Generate inline AXI write service code for one port."""
    n = name
    return [
        "    // AXI write service for " + n,
        (
            f"    dut->m_axi_{n}_AWREADY ="
            f" (!wr_{n}.aw_got && !wr_{n}.b_pending) ? 1 : 0;"
        ),
        f"    if (dut->m_axi_{n}_AWVALID && dut->m_axi_{n}_AWREADY) {{",
        f"        wr_{n}.aw_got = true;",
        f"        wr_{n}.addr = dut->m_axi_{n}_AWADDR;",
        f"        wr_{n}.id = dut->m_axi_{n}_AWID;",
        f"        wr_{n}.beat = 0;",
        "    }",
        f"    dut->m_axi_{n}_WREADY = wr_{n}.aw_got ? 1 : 0;",
        f"    if (dut->m_axi_{n}_WVALID && dut->m_axi_{n}_WREADY) {{",
        f"        mem_write(wr_{n}.addr + (uint64_t)wr_{n}.beat * {data_bytes},",
        f"                  &dut->m_axi_{n}_WDATA, dut->m_axi_{n}_WSTRB,",
        f"                  {data_bytes});",
        f"        wr_{n}.beat++;",
        f"        if (dut->m_axi_{n}_WLAST) {{",
        f"            wr_{n}.aw_got = false;",
        f"            wr_{n}.b_pending = true;",
        "        }",
        "    }",
        f"    dut->m_axi_{n}_BVALID = wr_{n}.b_pending ? 1 : 0;",
        f"    dut->m_axi_{n}_BID = wr_{n}.id;",
        f"    dut->m_axi_{n}_BRESP = 0;",
        f"    if (wr_{n}.b_pending && dut->m_axi_{n}_BREADY)",
        f"        wr_{n}.b_pending = false;",
    ]


_CPP_CTRL_WRITE_FUNC = [
    "static void ctrl_write(uint8_t addr, uint32_t data) {",
    "    dut->s_axi_control_AWVALID = 1;",
    "    dut->s_axi_control_AWADDR = addr;",
    "    dut->s_axi_control_WVALID = 1;",
    "    dut->s_axi_control_WDATA = data;",
    "    dut->s_axi_control_WSTRB = 0xF;",
    "    dut->s_axi_control_BREADY = 1;",
    "    for (int i = 0; i < 20; i++) {",
    "        service_all_axi(); tick();",
    "        if (dut->s_axi_control_BVALID) break;",
    "    }",
    "    dut->s_axi_control_AWVALID = 0;",
    "    dut->s_axi_control_WVALID = 0;",
    "    dut->s_axi_control_BREADY = 0;",
    "    service_all_axi(); tick();",
    "}",
    "",
]


def _generate_build_script(
    top_name: str,
) -> str:
    """Generate a shell script that builds the Verilator simulation."""
    verilator_bin, verilator_root = _find_verilator()

    # Verilator warning suppressions for HLS-generated code.
    # Use -Wno-WIDTH (covers WIDTHEXPAND/WIDTHTRUNC/WIDTHXZEXPAND) and
    # -Wno-fatal for broad compatibility across Verilator 5.x versions.
    warn_flags = (
        "-Wno-fatal -Wno-PINMISSING -Wno-WIDTH"
        " -Wno-UNUSEDSIGNAL -Wno-UNDRIVEN -Wno-UNOPTFLAT"
        " -Wno-STMTDLY -Wno-CASEINCOMPLETE -Wno-SYMRSVDWORD"
        " -Wno-COMBDLY -Wno-TIMESCALEMOD -Wno-MULTIDRIVEN"
    )

    root_export = f'export VERILATOR_ROOT="{verilator_root}"' if verilator_root else ""

    return f"""\
#!/bin/bash
set -e
cd "$(dirname "$0")"
{root_export}

{verilator_bin} --cc --top-module {top_name} \\
  {warn_flags} \\
  --no-timing \\
  --exe tb.cpp dpi_support.cpp \\
  rtl/*.v 2>&1

# -lrt is needed on Linux for shm_open; not needed/available on macOS
RT_LIB=""
if [ "$(uname)" = "Linux" ]; then RT_LIB="-lrt"; fi

make -C obj_dir -f V{top_name}.mk V{top_name} \\
  VM_USER_LDLIBS="$RT_LIB" \\
  -j$(nproc 2>/dev/null || sysctl -n hw.ncpu 2>/dev/null || echo 4) 2>&1
"""


def _find_verilator() -> tuple[str, str | None]:
    """Find the Verilator binary and optionally its root directory.

    Returns a (binary_path, verilator_root) tuple. verilator_root is set
    when using a Bazel-built Verilator that needs VERILATOR_ROOT exported.
    """
    # Check VERILATOR_BIN env var first (set by Bazel test rules)
    env_bin = os.environ.get("VERILATOR_BIN")
    if env_bin:
        verilator_bin = str(Path(env_bin).resolve())
        if not Path(verilator_bin).is_file():
            _logger.error("VERILATOR_BIN=%s does not exist", env_bin)
            sys.exit(1)
        # For Bazel-built verilator, find VERILATOR_ROOT in runfiles
        verilator_root = _find_verilator_root(verilator_bin)
        _logger.info(
            "Using Bazel Verilator: %s (root: %s)", verilator_bin, verilator_root
        )
        return verilator_bin, verilator_root

    # Fall back to PATH and common install locations
    verilator_bin = shutil.which("verilator")
    if verilator_bin is None:
        for candidate in (
            "/opt/homebrew/bin/verilator",
            "/usr/local/bin/verilator",
            "/usr/bin/verilator",
        ):
            if Path(candidate).is_file():
                verilator_bin = candidate
                break
    if verilator_bin is None:
        _logger.error("verilator not found in PATH or common locations")
        sys.exit(1)
    return verilator_bin, None


def _find_verilator_root(verilator_bin: str) -> str | None:
    """Determine VERILATOR_ROOT for a Bazel-built Verilator binary.

    Searches runfiles directories and standard install layouts for the
    Verilator include directory containing verilated.h.
    """
    bin_path = Path(verilator_bin)

    # In Bazel tests, the include files are in the test's runfiles tree.
    # Check TEST_SRCDIR / RUNFILES_DIR for the verilator repo.
    for env_var in ("TEST_SRCDIR", "RUNFILES_DIR"):
        runfiles_dir = os.environ.get(env_var)
        if not runfiles_dir:
            continue
        # Look for verilator+/include/ (bzlmod repo name) or verilator/include/
        for repo_name in ("verilator+", "verilator"):
            candidate = Path(runfiles_dir) / repo_name
            if (candidate / "include" / "verilated.h").is_file():
                return str(candidate.resolve())

    # Check binary's own runfiles: <binary>.runfiles/<repo>/include/
    runfiles_dir = bin_path.parent / (bin_path.name + ".runfiles")
    if runfiles_dir.is_dir():
        for entry in runfiles_dir.iterdir():
            if (
                entry.name.startswith("verilator")
                and (entry / "include" / "verilated.h").is_file()
            ):
                return str(entry.resolve())

    # Check standard install layout: bin/verilator -> ../include/
    root_candidate = bin_path.parent.parent
    if (root_candidate / "include" / "verilated.h").is_file():
        return str(root_candidate)

    _logger.warning("Could not determine VERILATOR_ROOT for %s", verilator_bin)
    return None
