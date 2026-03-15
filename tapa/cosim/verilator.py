"""Verilator-based cosimulation backend for TAPA.

Generates a C++ testbench and builds/runs it with Verilator, providing an
open-source alternative to xsim that works on both Linux and macOS.
"""

import logging
import re
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

    # Copy RTL files to output directory for Verilator
    rtl_dir = Path(tb_output_dir) / "rtl"
    rtl_dir.mkdir(parents=True, exist_ok=True)
    for v_file in Path(verilog_path).glob("*.v"):
        target = rtl_dir / v_file.name
        target.write_bytes(v_file.read_bytes())
    for sv_file in Path(verilog_path).glob("*.sv"):
        target = rtl_dir / sv_file.name
        target.write_bytes(sv_file.read_bytes())

    # Detect and replace Xilinx IPs with behavioral models
    ip_replacements = _detect_xilinx_ips(rtl_dir)
    for ip_file in ip_replacements:
        _logger.info("   Generated behavioral replacement: %s", ip_file)

    # Parse control register addresses (Vitis mode)
    reg_addrs: dict[str, list[str]] = {}
    if mode == "vitis":
        ctrl_path = f"{verilog_path}/{top_name}_control_s_axi.v"
        reg_addrs = parse_register_addr(ctrl_path)

    # Generate the C++ testbench
    tb_cpp = _generate_cpp_testbench(top_name, axi_list, args, config, reg_addrs, mode)
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


def _detect_xilinx_ips(rtl_dir: Path) -> list[str]:
    """Detect Xilinx IP instantiations and generate behavioral replacements.

    Scans for DPI-C imports in Verilog files that reference Xilinx floating-
    point or other IPs, and generates behavioral C++ replacements.

    Returns a list of generated replacement file names.
    """
    replacements = []

    for v_file in sorted(rtl_dir.glob("*.v")):
        content = v_file.read_text(encoding="utf-8", errors="replace")
        # Look for DPI-C imports that reference fp operations
        dpi_imports = re.findall(
            r'import\s+"DPI-C"\s+function\s+(\w+)\s+(\w+)\s*\(([^)]*)\)', content
        )
        for ret_type, func_name, params in dpi_imports:
            if func_name.startswith("fp"):
                # Already have a behavioral model for this — generate it
                _logger.info(
                    "   Found DPI-C function: %s in %s", func_name, v_file.name
                )

    # Check for encrypted Xilinx IP modules (pragma protect)
    for v_file in sorted(rtl_dir.glob("*.v")):
        content = v_file.read_text(encoding="utf-8", errors="replace")
        if "`pragma protect" in content:
            _logger.warning(
                "   Encrypted Xilinx IP found: %s — needs behavioral replacement",
                v_file.name,
            )

    return replacements


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

}  // extern "C"
"""


def _generate_cpp_testbench(  # noqa: PLR0913, PLR0917
    top_name: str,
    axi_list: list[AXI],
    args: Sequence[Arg],
    config: dict,
    reg_addrs: dict[str, list[str]],
    mode: str,
) -> str:
    """Generate a C++ testbench for Verilator simulation."""
    lines: list[str] = []
    lines.extend(_cpp_preamble(top_name, axi_list, mode))
    lines.extend(_cpp_main_body(top_name, axi_list, args, config, reg_addrs, mode))
    lines.extend(_cpp_axi_helpers(axi_list, mode))
    return "\n".join(lines) + "\n"


def _cpp_preamble(top_name: str, axi_list: list[AXI], mode: str) -> list[str]:
    """Generate C++ includes, types, and global declarations."""
    lines: list[str] = [
        "#include <cstdio>",
        "#include <cstdlib>",
        "#include <cstring>",
        "#include <fstream>",
        "#include <map>",
        "#include <vector>",
        f'#include "V{top_name}.h"',
        '#include "verilated.h"',
        "",
        "static std::map<uint64_t, uint8_t> memory;",
        "",
        "static uint32_t mem_read32(uint64_t addr) {",
        "    addr &= ~3ULL;",
        "    uint32_t val = 0;",
        "    for (int i = 0; i < 4; i++)",
        "        val |= (uint32_t)memory[addr + i] << (i * 8);",
        "    return val;",
        "}",
        "",
        "static void mem_write32(uint64_t addr, uint32_t data, uint8_t strb) {",
        "    addr &= ~3ULL;",
        "    for (int i = 0; i < 4; i++)",
        "        if (strb & (1 << i))",
        "            memory[addr + i] = (data >> (i * 8)) & 0xFF;",
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
        "    uint8_t len = 0, beat = 0;",
        "};",
        "",
        "struct AxiWritePort {",
        "    bool aw_got = false, b_pending = false;",
        "    uint64_t addr = 0;",
        "    uint8_t beat = 0;",
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
    if mode == "vitis":
        lines.extend(
            [
                "static void ctrl_write(uint8_t addr, uint32_t data);",
                "static void service_all_axi();",
                "",
            ]
        )
    return lines


def _cpp_main_body(  # noqa: PLR0913, PLR0917
    top_name: str,
    axi_list: list[AXI],
    args: Sequence[Arg],
    config: dict,
    reg_addrs: dict[str, list[str]],
    mode: str,
) -> list[str]:
    """Generate the main() function body."""
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

    for axi in axi_list:
        n = axi.name
        lines.extend(
            [
                f"    dut->m_axi_{n}_ARREADY = 0;",
                f"    dut->m_axi_{n}_RVALID = 0; dut->m_axi_{n}_RDATA = 0;",
                f"    dut->m_axi_{n}_RLAST = 0;"
                f" dut->m_axi_{n}_RID = 0;"
                f" dut->m_axi_{n}_RRESP = 0;",
                f"    dut->m_axi_{n}_AWREADY = 1; dut->m_axi_{n}_WREADY = 1;",
                f"    dut->m_axi_{n}_BVALID = 0;"
                f" dut->m_axi_{n}_BID = 0;"
                f" dut->m_axi_{n}_BRESP = 0;",
            ]
        )

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

    # Load binary data
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

    # Control register writes (Vitis mode)
    if mode == "vitis":
        lines.extend(_cpp_ctrl_writes(args, config, reg_addrs))

    lines.extend(
        [
            '    printf("Kernel started, running simulation...\\n");',
            "",
            "    bool done = false;",
            "    int timeout = 1000000;",
            "    for (int cycle = 0; cycle < timeout; cycle++) {",
            "        service_all_axi();",
            "        tick();",
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

    lines.extend(
        [
            "    }",
            "",
            "    if (!done) {",
            '        printf("TIMEOUT: kernel did not complete'
            ' in %d cycles\\n", timeout);',
            "        delete dut;",
            "        return 1;",
            "    }",
            "",
        ]
    )

    for axi in axi_list:
        data_path = axi_to_data.get(axi.name, "")
        if data_path:
            lines.append(
                f'    dump_binary("{data_path}_out.bin",'
                f" {axi.name}_BASE, {axi.name}_SIZE);"
            )

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
        if arg.is_mmap and arg.name in reg_addrs:
            addrs = reg_addrs[arg.name]
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


def _cpp_axi_helpers(axi_list: list[AXI], mode: str) -> list[str]:
    """Generate AXI service functions and ctrl_write."""
    lines: list[str] = _CPP_AXI_SERVICE_FUNCTIONS[:]

    # service_all_axi
    lines.append("static void service_all_axi() {")
    for axi in axi_list:
        n = axi.name
        lines.extend(
            [
                "    { uint8_t ar, rv, rl, ri, rr; uint32_t rd;",
                f"      service_axi_read(rd_{n},",
                f"        dut->m_axi_{n}_ARVALID,",
                f"        dut->m_axi_{n}_ARADDR,",
                f"        dut->m_axi_{n}_ARLEN,",
                f"        dut->m_axi_{n}_RREADY,",
                "        ar, rv, rd, rl, ri, rr);",
                f"      dut->m_axi_{n}_ARREADY = ar;",
                f"      dut->m_axi_{n}_RVALID = rv;",
                f"      dut->m_axi_{n}_RDATA = rd;",
                f"      dut->m_axi_{n}_RLAST = rl;",
                f"      dut->m_axi_{n}_RID = ri;",
                f"      dut->m_axi_{n}_RRESP = rr; }}",
                "    { uint8_t aw, wr, bv, bi, br;",
                f"      service_axi_write(wr_{n},",
                f"        dut->m_axi_{n}_AWVALID,",
                f"        dut->m_axi_{n}_AWADDR,",
                f"        dut->m_axi_{n}_AWLEN,",
                f"        dut->m_axi_{n}_WVALID,",
                f"        dut->m_axi_{n}_WDATA,",
                f"        dut->m_axi_{n}_WSTRB,",
                f"        dut->m_axi_{n}_WLAST,",
                f"        dut->m_axi_{n}_BREADY,",
                "        aw, wr, bv, bi, br);",
                f"      dut->m_axi_{n}_AWREADY = aw;",
                f"      dut->m_axi_{n}_WREADY = wr;",
                f"      dut->m_axi_{n}_BVALID = bv;",
                f"      dut->m_axi_{n}_BID = bi;",
                f"      dut->m_axi_{n}_BRESP = br; }}",
            ]
        )
    lines.extend(["}", ""])

    if mode == "vitis":
        lines.extend(_CPP_CTRL_WRITE_FUNC)

    return lines


_CPP_AXI_SERVICE_FUNCTIONS = [
    "static void service_axi_read(AxiReadPort& p,",
    "    uint8_t arvalid, uint64_t araddr,",
    "    uint8_t arlen, uint8_t rready,",
    "    uint8_t& arready, uint8_t& rvalid,",
    "    uint32_t& rdata,",
    "    uint8_t& rlast, uint8_t& rid,",
    "    uint8_t& rresp) {",
    "    arready = !p.busy;",
    "    if (arvalid && arready) {",
    "        p.busy = true; p.addr = araddr;",
    "        p.len = arlen; p.beat = 0;",
    "    }",
    "    if (p.busy) {",
    "        rvalid = 1;",
    "        rdata = mem_read32(",
    "            p.addr + (uint64_t)p.beat * 4);",
    "        rlast = (p.beat == p.len) ? 1 : 0;",
    "        rid = 0; rresp = 0;",
    "        if (rready) {",
    "            if (p.beat >= p.len) p.busy = false;",
    "            else p.beat++;",
    "        }",
    "    } else {",
    "        rvalid = 0; rdata = 0; rlast = 0;",
    "        rid = 0; rresp = 0;",
    "    }",
    "}",
    "",
    "static void service_axi_write(AxiWritePort& p,",
    "    uint8_t awvalid, uint64_t awaddr,",
    "    uint8_t awlen,",
    "    uint8_t wvalid, uint32_t wdata,",
    "    uint8_t wstrb, uint8_t wlast,",
    "    uint8_t bready,",
    "    uint8_t& awready, uint8_t& wready,",
    "    uint8_t& bvalid, uint8_t& bid,",
    "    uint8_t& bresp) {",
    "    awready = (!p.aw_got && !p.b_pending) ? 1 : 0;",
    "    if (awvalid && awready) {",
    "        p.aw_got = true; p.addr = awaddr;",
    "        p.beat = 0;",
    "    }",
    "    wready = p.aw_got ? 1 : 0;",
    "    if (wvalid && wready) {",
    "        mem_write32(",
    "            p.addr + (uint64_t)p.beat * 4,",
    "            wdata, wstrb);",
    "        p.beat++;",
    "        if (wlast) {",
    "            p.aw_got = false;",
    "            p.b_pending = true;",
    "        }",
    "    }",
    "    bvalid = p.b_pending ? 1 : 0;",
    "    bid = 0; bresp = 0;",
    "    if (p.b_pending && bready)",
    "        p.b_pending = false;",
    "}",
    "",
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
    # Verilator warning suppressions for HLS-generated code
    warn_flags = (
        "-Wno-PINMISSING -Wno-WIDTHEXPAND -Wno-WIDTHTRUNC"
        " -Wno-UNUSEDSIGNAL -Wno-UNDRIVEN -Wno-UNOPTFLAT"
        " -Wno-STMTDLY -Wno-WIDTHXZEXPAND"
        " -Wno-CASEINCOMPLETE -Wno-SYMRSVDWORD -Wno-COMBDLY"
    )

    return f"""\
#!/bin/bash
set -e
cd "$(dirname "$0")"

verilator --cc --top-module {top_name} \\
  {warn_flags} \\
  --no-timing \\
  --exe tb.cpp dpi_support.cpp \\
  rtl/*.v 2>&1

make -C obj_dir -f V{top_name}.mk V{top_name} \\
  -j$(nproc 2>/dev/null || sysctl -n hw.ncpu 2>/dev/null || echo 4) 2>&1
"""
