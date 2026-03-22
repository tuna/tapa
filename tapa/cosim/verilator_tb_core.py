"""C++ testbench generation for Verilator cosimulation."""

from __future__ import annotations

from pathlib import Path
from typing import TYPE_CHECKING

from tapa.cosim.verilator_tb_axi import (
    generate_axi_helpers,
    generate_ctrl_writes,
    generate_hls_port_setup,
)
from tapa.cosim.verilator_tb_stream import generate_stream_support

if TYPE_CHECKING:
    from collections.abc import Sequence

    from tapa.cosim.common import AXI, Arg


_WIDE_RDATA_WIDTH = 64


def generate_cpp_testbench(  # noqa: PLR0913, PLR0917
    top_name: str,
    axi_list: list[AXI],
    args: Sequence[Arg],
    config: dict,
    reg_addrs: dict[str, list[str]],
    mode: str,
) -> str:
    """Generate a C++ testbench for Verilator simulation."""
    lines: list[str] = []
    lines.extend(_cpp_preamble(top_name, axi_list, args, mode))
    lines.extend(_cpp_main_body(top_name, axi_list, args, config, reg_addrs, mode))
    lines.extend(generate_axi_helpers(axi_list, mode))
    return "\n".join(lines) + "\n"


def _cpp_preamble(
    top_name: str,
    axi_list: list[AXI],
    args: Sequence[Arg],
    mode: str,
) -> list[str]:
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

    lines.extend(generate_stream_support(args))

    lines.append("")
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
    lines.extend(_cpp_stream_init(stream_args))

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

    if mode == "vitis":
        lines.extend(generate_ctrl_writes(args, config, reg_addrs))
    else:
        lines.extend(generate_hls_port_setup(args, axi_list, config))

    lines.extend(_cpp_sim_loop(stream_args, mode))
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
        if axi.data_width > _WIDE_RDATA_WIDTH:
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


def _cpp_stream_init(stream_args: Sequence[Arg]) -> list[str]:
    """Generate FIFO stream signal initialization statements."""
    lines: list[str] = []
    for arg in stream_args:
        n = arg.qualified_name
        if arg.port.is_istream:
            lines.extend(
                [
                    f"    dut->{n}_dout = 0;",
                    f"    dut->{n}_empty_n = 0;",
                ]
            )
        elif arg.port.is_ostream:
            lines.append(f"    dut->{n}_full_n = 1;")
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
            lines.append(
                f'    load_stream(stream_{arg.qualified_name}, "{data_path}");'
            )
    if stream_args:
        lines.append("")
    return lines


def _cpp_stream_service(stream_args: Sequence[Arg]) -> list[str]:
    """Generate FIFO stream servicing code for the simulation loop."""
    lines: list[str] = []
    for arg in stream_args:
        n = arg.qualified_name
        w = (arg.port.data_width + 7) // 8
        if arg.port.is_istream:
            lines.extend(
                [
                    f"        if (dut->{n}_read && dut->{n}_empty_n)",
                    f"            stream_{n}.data.pop();",
                    f"        if (!stream_{n}.data.empty()) {{",
                    f"            dut->{n}_empty_n = 1;",
                    (
                        f"            memcpy(&dut->{n}_dout,"
                        f" stream_{n}.data.front().data(), {w});"
                    ),
                    f"        }} else dut->{n}_empty_n = 0;",
                ]
            )
        elif arg.port.is_ostream:
            lines.extend(
                [
                    f"        if (dut->{n}_write && dut->{n}_full_n) {{",
                    f"            std::vector<uint8_t> buf({w});",
                    f"            memcpy(buf.data(), &dut->{n}_din, {w});",
                    f"            stream_{n}.data.push(std::move(buf));",
                    "        }",
                    f"        dut->{n}_full_n = 1;",
                ]
            )
    return lines


def _cpp_sim_loop(stream_args: Sequence[Arg], mode: str) -> list[str]:
    """Generate the main simulation loop."""
    lines: list[str] = [
        '    printf("Kernel started, running simulation...\\n");',
        "",
        "    bool done = false;",
        "    int timeout = 50000000;",
        "    for (int cycle = 0; cycle < timeout; cycle++) {",
        "        service_all_axi();",
    ]

    lines.extend(_cpp_stream_service(stream_args))
    lines.append("        tick();")

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

    lines.extend(
        [
            "    }",
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
            out_path = _get_output_path(data_path)
            lines.append(
                f'    dump_binary("{out_path}", {axi.name}_BASE, {axi.name}_SIZE);'
            )

    axis_to_data = config.get("axis_to_data_file", {})
    for arg in stream_args:
        data_path = axis_to_data.get(arg.qualified_name, "")
        if data_path and arg.port.is_ostream:
            out_path = _get_output_path(data_path)
            lines.append(f'    dump_stream(stream_{arg.qualified_name}, "{out_path}");')
    return lines


def _get_output_path(input_path: str) -> str:
    """Convert an input data path to the corresponding output path."""
    p = Path(input_path)
    return str(p.with_name(p.stem + "_out.bin"))
